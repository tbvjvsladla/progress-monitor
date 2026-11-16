//! Child processes: Git queries, the environment probe, and the check runner.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::os_error_name;
use crate::py::{strip, universal_newlines};
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

fn read_all(mut source: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = source.read_to_end(&mut buf);
        buf
    })
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
pub fn capture_input(mut command: Command, input: Option<&str>, timeout: Duration) -> Captured {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return Captured::Failed(err),
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
    let out = child.stdout.take().map(read_all);
    let err = child.stderr.take().map(read_all);
    let waited = wait_until(&mut child, Some(Instant::now() + timeout));
    if let Some(feeder) = feeder {
        if !matches!(waited, Ok(Some(_))) {
            let _ = child.kill();
        }
        let _ = feeder.join();
    }
    match waited {
        Ok(Some(status)) => {
            let text = |h: Option<thread::JoinHandle<Vec<u8>>>| {
                h.and_then(|h| h.join().ok())
                    .map(|b| universal_newlines(&String::from_utf8_lossy(&b)))
                    .unwrap_or_default()
            };
            Captured::Done {
                status,
                stdout: text(out),
                stderr: text(err),
            }
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            Captured::Timeout
        }
        Err(err) => Captured::Failed(err),
    }
}

/// Exit code as the baseline reported it: a signal N is -N.
pub fn exit_code(status: ExitStatus) -> i64 {
    match status.code() {
        Some(code) => i64::from(code),
        None => -i64::from(status.signal().unwrap_or(0)),
    }
}

/// `git -C <root> <args>`: (exit code, stripped stdout); a failure to run is (1, message).
pub fn git(root: &Path, args: &[&str]) -> (i64, String) {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args).stdin(Stdio::null());
    match capture(command, Duration::from_secs(120)) {
        Captured::Done { status, stdout, .. } => (exit_code(status), strip(&stdout).to_string()),
        Captured::Timeout => (1, "git timed out after 120 seconds".to_string()),
        Captured::Failed(err) => (1, err.to_string()),
    }
}

/// One environment probe line: its output, `exit <code>: <output>`, or `error: <name>`.
pub fn probe(cmd: &str, cwd: &Path) -> String {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null());
    match capture(command, Duration::from_secs(60)) {
        Captured::Done {
            status,
            stdout,
            stderr,
        } => {
            let both = format!("{stdout}{stderr}");
            let both = strip(&both);
            if status.success() {
                both.to_string()
            } else {
                format!("exit {}: {}", exit_code(status), both)
            }
        }
        Captured::Timeout => "error: TimeoutExpired".to_string(),
        Captured::Failed(err) => format!("error: {}", os_error_name(&err)),
    }
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
        .arg(cmd)
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
            let deadline = Duration::try_from_secs_f64(timeout_s.max(0.0))
                .ok()
                .map(|d| Instant::now() + d);
            match wait_until(&mut child, deadline)? {
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
