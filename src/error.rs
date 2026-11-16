//! The three ways a command ends without success.

use serde_json::Value;

use crate::py::Obj;

#[derive(Debug)]
pub enum PmError {
    /// The request is understood but not allowed now (exit 1). Holds the whole payload.
    Refused(Obj),
    /// Bad arguments or unreadable input (exit 2).
    Usage(String),
    /// A malformed file met an operation it cannot take (exit 2), named like the baseline's exception.
    Internal(String),
}

pub type Res<T> = Result<T, PmError>;

impl PmError {
    /// `{"ok": false, "refused": name, **detail}`.
    pub fn refused(name: impl Into<String>, detail: Vec<(&str, Value)>) -> PmError {
        let mut payload = Obj::new();
        payload.insert("ok".into(), Value::Bool(false));
        payload.insert("refused".into(), Value::String(name.into()));
        for (key, value) in detail {
            payload.insert(key.to_string(), value);
        }
        PmError::Refused(payload)
    }

    pub fn usage(text: impl Into<String>) -> PmError {
        PmError::Usage(text.into())
    }

    /// An operating-system error the baseline did not catch.
    pub fn os(err: &std::io::Error) -> PmError {
        PmError::Internal(format!("{}: {err}", os_error_name(err)))
    }

    pub fn is_usage(&self) -> bool {
        matches!(self, PmError::Usage(_))
    }
}

/// The baseline's exception class name for an I/O error kind.
pub fn os_error_name(err: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match err.kind() {
        K::NotFound => "FileNotFoundError",
        K::PermissionDenied => "PermissionError",
        K::AlreadyExists => "FileExistsError",
        K::NotADirectory => "NotADirectoryError",
        K::IsADirectory => "IsADirectoryError",
        K::TimedOut => "TimeoutError",
        K::BrokenPipe => "BrokenPipeError",
        K::Interrupted => "InterruptedError",
        _ => "OSError",
    }
}
