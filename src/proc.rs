//! Child processes: Git queries, the environment probe, and the check runner.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use std::sync::mpsc;

use crate::error::{os_error_name, PmError, Res};
use crate::py::{repr_str, strip, universal_newlines, utf8_error_text};
use crate::time::{now, seconds_between, stamp};
use crate::TOOL;

/// Whether a process with this id exists (a non-int or non-positive id does not).
pub fn alive(pid: Option<&Value>) -> bool {
    let Some(Value::Number(n)) = pid else {
        return false;
    };
    let Some(pid) = n.as_i64() else {
        return false;
    };
    if pid <= 0 || pid > i64::from(i32::MAX) {
        return false;
    }
    // SAFETY: kill with signal 0 only checks for the process; it sends nothing.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Host name as `uname().nodename`.
pub fn node_name() -> String {
    // SAFETY: uname fills the zeroed struct; the fields are NUL-terminated C strings.
    unsafe {
        let mut info: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut info) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(info.nodename.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

pub enum Captured {
    Done {
        status: ExitStatus,
        stdout: String,
        stderr: String,
    },
    Timeout,
    Failed(std::io::Error),
}

/// What [`capture_bytes`] saw: the undecoded output.
pub enum CapturedBytes {
    Done {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Timeout,
    Failed(std::io::Error),
}

fn read_all(
    mut source: impl Read + Send + 'static,
    done: mpsc::Sender<()>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = source.read_to_end(&mut buf);
        let _ = done.send(());
        buf
    })
}

/// The moment `seconds` from now, as the baseline's `monotonic() + seconds`
/// limit behaved: none for NaN, for infinity and past what an `Instant` holds
/// (the limit is never reached), now for zero and below.
pub fn deadline_after(seconds: f64) -> Option<Instant> {
    let start = Instant::now();
    if seconds.is_nan() {
        return None;
    }
    if seconds <= 0.0 {
        return Some(start);
    }
    Duration::try_from_secs_f64(seconds)
        .ok()
        .and_then(|d| start.checked_add(d))
}

fn wait_until(child: &mut Child, deadline: Option<Instant>) -> std::io::Result<Option<ExitStatus>> {
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if let Some(limit) = deadline {
            if Instant::now() >= limit {
                return Ok(None);
            }
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// Run a command with stdout and stderr captured; on timeout kill the child (not its group).
pub fn capture(command: Command, timeout: Duration) -> Captured {
    capture_input(command, None, timeout)
}

/// [`capture`] with `input` written to the child's stdin (then closed) when given.
/// Output is decoded leniently (invalid UTF-8 becomes U+FFFD).
pub fn capture_input(command: Command, input: Option<&str>, timeout: Duration) -> Captured {
    match capture_bytes(command, input, timeout) {
        CapturedBytes::Done {
            status,
            stdout,
            stderr,
        } => {
            let text = |b: &[u8]| universal_newlines(&String::from_utf8_lossy(b));
            Captured::Done {
                status,
                stdout: text(&stdout),
                stderr: text(&stderr),
            }
        }
        CapturedBytes::Timeout => Captured::Timeout,
        CapturedBytes::Failed(err) => Captured::Failed(err),
    }
}

/// `subprocess.run(..., capture_output=True, timeout=...)`: the time limit covers
/// the child's exit and the end of both pipes (a background grandchild holding a
/// pipe open runs into it). On timeout the child is killed and reaped; the
/// grandchildren are left alone, as the baseline left them.
pub fn capture_bytes(
    mut command: Command,
    input: Option<&str>,
    timeout: Duration,
) -> CapturedBytes {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let deadline = Instant::now().checked_add(timeout);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return CapturedBytes::Failed(err),
    };
    let feeder = match (child.stdin.take(), input) {
        (Some(mut stdin), Some(text)) => {
            let bytes = text.as_bytes().to_vec();
            Some(thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            }))
        }
        _ => None,
    };
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().map(|s| read_all(s, tx.clone()));
    let err = child.stderr.take().map(|s| read_all(s, tx.clone()));
    drop(tx);
    let mut open_pipes = usize::from(out.is_some()) + usize::from(err.is_some());
    let mut timed_out = false;
    while open_pipes > 0 {
        let waited = match deadline {
            Some(limit) => rx.recv_timeout(limit.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match waited {
            Ok(()) => open_pipes -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let waited = if timed_out {
        Ok(None)
    } else {
        wait_until(&mut child, deadline)
    };
    if let Some(feeder) = feeder {
        if !matches!(waited, Ok(Some(_))) {
            let _ = child.kill();
        }
        if !timed_out {
            let _ = feeder.join();
        }
    }
    match waited {
        Ok(Some(status)) => {
            let bytes = |h: Option<thread::JoinHandle<Vec<u8>>>| {
                h.and_then(|h| h.join().ok()).unwrap_or_default()
            };
            CapturedBytes::Done {
                status,
                stdout: bytes(out),
                stderr: bytes(err),
            }
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            CapturedBytes::Timeout // the reader threads end with the last writer
        }
        Err(err) => CapturedBytes::Failed(err),
    }
}

/// `text=True` decoding of captured output: strict UTF-8 with universal newlines,
/// stdout first; a bad byte is the baseline's uncaught `UnicodeDecodeError`.
fn decode_strict(bytes: &[u8]) -> Res<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(universal_newlines(text)),
        Err(err) => Err(PmError::Internal(format!(
            "UnicodeDecodeError: {}",
            utf8_error_text(bytes, err)
        ))),
    }
}

/// `str(OSError)` as the baseline wrote it for a failed spawn: `[Errno N] text: 'program'`.
fn spawn_error_text(err: &std::io::Error, program: &str) -> String {
    match err.raw_os_error() {
        Some(code) => {
            // SAFETY: strerror returns a NUL-terminated static or thread-local message.
            let text = unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)) }
                .to_string_lossy()
                .into_owned();
            format!("[Errno {code}] {text}: {}", repr_str(program))
        }
        None => err.to_string(),
    }
}

/// Exit code as the baseline reported it: a signal N is -N.
pub fn exit_code(status: ExitStatus) -> i64 {
    match status.code() {
        Some(code) => i64::from(code),
        None => -i64::from(status.signal().unwrap_or(0)),
    }
}

/// `git -C <root> <args>`: (exit code, stripped stdout); a failure to run or a
/// timeout is (1, the exception text). Output that is not UTF-8 is an error.
pub fn git(root: &Path, args: &[&str]) -> Res<(i64, String)> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args.iter().map(|a| crate::jsonio::os_text(a)));
    match capture_bytes(command, None, Duration::from_secs(120)) {
        CapturedBytes::Done {
            status,
            stdout,
            stderr,
        } => {
            let out = decode_strict(&stdout)?;
            decode_strict(&stderr)?;
            Ok((exit_code(status), strip(&out).to_string()))
        }
        CapturedBytes::Timeout => {
            let root = root.display().to_string();
            let argv: Vec<String> = ["git", "-C", root.as_str()]
                .into_iter()
                .chain(args.iter().copied())
                .map(repr_str)
                .collect();
            Ok((
                1,
                format!(
                    "Command '[{}]' timed out after 120 seconds",
                    argv.join(", ")
                ),
            ))
        }
        CapturedBytes::Failed(err) => Ok((1, spawn_error_text(&err, "git"))),
    }
}

/// One environment probe line: its output, `exit <code>: <output>`, or `error: <name>`.
/// Output that is not UTF-8 is the baseline's uncaught `UnicodeDecodeError`.
pub fn probe(cmd: &str, cwd: &Path) -> Res<String> {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(crate::jsonio::os_text(cmd))
        .current_dir(cwd)
        .stdin(Stdio::null());
    Ok(
        match capture_bytes(command, None, Duration::from_secs(60)) {
            CapturedBytes::Done {
                status,
                stdout,
                stderr,
            } => {
                let both = decode_strict(&stdout)? + &decode_strict(&stderr)?;
                let both = strip(&both);
                if status.success() {
                    both.to_string()
                } else {
                    format!("exit {}: {}", exit_code(status), both)
                }
            }
            CapturedBytes::Timeout => "error: TimeoutExpired".to_string(),
            CapturedBytes::Failed(err) => format!("error: {}", os_error_name(&err)),
        },
    )
}

pub struct Executed {
    pub exit_code: Option<i64>,
    pub error: Option<&'static str>,
    pub output: String,
    pub started: String,
    pub ended: String,
    pub seconds: f64,
}

fn spawn_in_session(cmd: &str, cwd: &Path, log: &File) -> std::io::Result<Child> {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(crate::jsonio::os_text(cmd))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?));
    // SAFETY: runs in the forked child before exec; setsid is async-signal-safe and touches no
    // memory of the parent.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()
}

/// Run one shell command in its own session, keep its output in `log_path`, and classify the end.
pub fn execute(
    cmd: &str,
    cwd: &Path,
    timeout_s: f64,
    log_path: &Path,
) -> std::io::Result<Executed> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let started = now();
    let mut code = None;
    let mut error = None;
    let log = File::create(log_path)?;
    match spawn_in_session(cmd, cwd, &log) {
        Ok(mut child) => {
            match wait_until(&mut child, deadline_after(timeout_s))? {
                Some(status) => code = Some(exit_code(status)),
                None => {
                    // SAFETY: the child leads its own process group (setsid); kill only signals it.
                    unsafe {
                        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    error = Some("timeout");
                }
            }
        }
        Err(err) => {
            drop(log);
            error = Some("spawn");
            let mut log = OpenOptions::new()
                .append(true)
                .create(true)
                .open(log_path)?;
            writeln!(log, "[{TOOL}] could not start the command: {err}")?;
        }
    }
    let ended = now();
    if error.is_none() {
        if let Some(c) = code {
            if c < 0 || (128 < c && c <= 128 + 64) {
                error = Some("signal"); // the shell reports 128+N for a command killed by signal N
            } else if c == 126 || c == 127 {
                error = Some("not-found");
            }
        }
    }
    let output = fs::read(log_path)
        .map(|b| universal_newlines(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    Ok(Executed {
        exit_code: if error == Some("timeout") { None } else { code },
        error,
        output,
        started: stamp(started),
        ended: stamp(ended),
        seconds: seconds_between(started, ended),
    })
}
