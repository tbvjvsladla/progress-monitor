//! The instance lock: `flock` on `<instance>/.lock`, retried every 50 ms until a deadline.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{PmError, Res};
use crate::py::g_format;

/// Held while state is read and written; released when dropped.
pub struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // SAFETY: the descriptor is owned by `self.file` and still open.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Seconds to wait: the argument, else `PM_LOCK_WAIT_S` (default 30) read as
/// the baseline's `float()` reads a string.
pub fn wait_seconds(wait: Option<f64>) -> Res<f64> {
    if let Some(w) = wait {
        return Ok(w);
    }
    let text = std::env::var("PM_LOCK_WAIT_S").unwrap_or_else(|_| "30".to_string());
    crate::py::py_float(&text).ok_or_else(|| {
        PmError::Internal(format!(
            "ValueError: could not convert string to float: {}",
            crate::py::repr_str(&text)
        ))
    })
}

pub fn take(dir: &Path, wait: Option<f64>) -> Res<LockGuard> {
    let wait = wait_seconds(wait)?;
    let path = dir.join(".lock");
    let file = OpenOptions::new()
        .append(true)
        .read(true)
        .create(true)
        .open(&path)
        .map_err(|e| PmError::os(&e))?;
    let deadline = Duration::try_from_secs_f64(wait.max(0.0))
        .ok()
        .and_then(|d| Instant::now().checked_add(d));
    loop {
        // SAFETY: the descriptor belongs to `file`, which outlives this call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(LockGuard { file });
        }
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(PmError::usage(format!(
                "{}: could not take the lock in {}s",
                path.display(),
                g_format(wait)
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}
