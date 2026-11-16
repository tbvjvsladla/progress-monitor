//! Read-only form check of a campaign instance or of the campaign template.
//!
//! The check reads one point in time and never runs a declared `proof.check`
//! command. Rules, modes and exit codes are listed in [`cli::HELP`]. The executor
//! (the `pm` program in the same build folder) writes the files; this check only
//! reads them.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::jsonio::{self, JsonError, Unit};
use crate::py;

pub mod cli;
pub mod control;
pub mod declaration;
pub mod rules;

pub use rules::check;

pub const PLACEHOLDER: &str = "<<FILL>>";
pub const SKELETON_CELL: &str = "_cell";
pub const STATES: [&str; 4] = ["pending", "running", "done", "failed"];
pub const CELL_STATES: [&str; 5] = ["open", "closed", "cancelled", "stopped", "waiting"];
pub const ORIGINS: [&str; 2] = ["plan", "auto"];
pub const POINTER_KEYS: [&str; 4] = ["kind", "path", "cell", "note"];
pub const DECLARATION_TEXT: [&str; 5] = ["id", "goal", "plan_ref", "checklist_ref", "declared"];
pub const DEFAULT_AUTO_CELLS: i64 = 5;
pub const RULES: [&str; 18] = [
    "proof-blank",
    "source-missing",
    "state-invalid",
    "done-not-ok",
    "undeclared",
    "cell-summary-incomplete",
    "journey-invalid",
    "pointer-invalid",
    "declared-missing",
    "placeholder-left",
    "no-cells",
    "done-unrecorded",
    "closed-without-gate",
    "structure-unlogged",
    "auto-cap-exceeded",
    "check-duplicate",
    "ref-invalid",
    "gate-not-proven",
];

/// Why a check could not finish.
#[derive(Debug)]
pub enum LintError {
    /// The input cannot be interpreted at all (exit 2, reported as JSON).
    Unreadable(String),
    /// An input the baseline crashed on, or an operating-system error (exit 1, stderr only).
    Internal(String),
}

pub type LintResult<T> = Result<T, LintError>;

impl LintError {
    pub fn io(path: &Path, err: &std::io::Error) -> LintError {
        LintError::Internal(format!(
            "{}: {}: {err}",
            crate::error::os_error_name(err),
            path.display()
        ))
    }
}

/// One rule failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub path: String,
    pub message: String,
}

impl Finding {
    pub fn to_value(&self) -> Value {
        Value::Object(py::obj([
            ("rule", Value::from(self.rule)),
            ("path", Value::from(self.path.clone())),
            ("message", Value::from(self.message.clone())),
        ]))
    }
}

/// Findings in the order the rules met them.
#[derive(Debug, Default)]
pub struct Findings(pub Vec<Finding>);

impl Findings {
    pub fn add(&mut self, rule: &'static str, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(Finding {
            rule,
            path: path.into(),
            message: message.into(),
        });
    }
}

/// The findings as a JSON list (`[{rule, path, message}, ...]`).
pub fn findings_value(findings: &[Finding]) -> Value {
    Value::Array(findings.iter().map(Finding::to_value).collect())
}

/// Not a string, or only whitespace.
pub fn blank(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => py::strip(s).is_empty(),
        _ => true,
    }
}

/// A non-blank string value.
pub fn text_of(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(s)) if !py::strip(s).is_empty() => Some(s),
        _ => None,
    }
}

/// `value in (names...)` for a tuple of strings.
pub fn one_of(value: Option<&Value>, names: &[&str]) -> bool {
    matches!(value, Some(Value::String(s)) if names.contains(&s.as_str()))
}

/// File contents read as text with newline translation; `Err(reason)` when not UTF-8.
pub fn read_text(path: &Path) -> LintResult<Result<String, String>> {
    let bytes = fs::read(path).map_err(|e| LintError::io(path, &e))?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => Ok(py::universal_newlines(&text)),
        Err(err) => Err(err.to_string()),
    })
}

/// Text that must decode; a decoding failure stops the run like the baseline's crash.
pub fn read_utf8(path: &Path) -> LintResult<String> {
    read_text(path)?.map_err(|reason| {
        LintError::Internal(format!("UnicodeDecodeError: {}: {reason}", path.display()))
    })
}

/// A JSON file: missing or malformed is unreadable.
pub fn load_json(path: &Path) -> LintResult<Value> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(LintError::Unreadable(format!(
                "{}: missing",
                path.display()
            )));
        }
        Err(err) => return Err(LintError::io(path, &err)),
    };
    let not_json =
        |reason: String| LintError::Unreadable(format!("{}: not JSON ({reason})", path.display()));
    let text = String::from_utf8(bytes).map_err(|e| not_json(e.to_string()))?;
    match jsonio::parse(&py::universal_newlines(&text)) {
        Ok(value) => Ok(value),
        Err(JsonError::Decode(reason)) => Err(not_json(reason)),
        Err(JsonError::Other(text)) => Err(LintError::Internal(text)),
    }
}

/// `json.loads` where the baseline caught only `JSONDecodeError`: `None` for one;
/// any other exception the decoder raised (`RecursionError`, the integer-digit
/// `ValueError`) ends the command.
pub fn parse_json(text: &str) -> LintResult<Option<Value>> {
    match jsonio::parse(text) {
        Ok(value) => Ok(Some(value)),
        Err(JsonError::Decode(_)) => Ok(None),
        Err(JsonError::Other(text)) => Err(LintError::Internal(text)),
    }
}

/// A JSON object read from a file (control fixtures only).
pub fn parse_object(path: &Path) -> LintResult<Value> {
    let text = read_utf8(path)?;
    jsonio::parse(&text)
        .map_err(|e| LintError::Internal(format!("JSONDecodeError: {}: {e}", path.display())))
}

/// `is_file()`/`is_dir()`/`exists()`: symbolic links are followed, errors read as false.
pub fn is_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file())
}

pub fn is_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

pub fn exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

/// A file name as the baseline's text: UTF-8 with each undecodable byte held as
/// the lone surrogate U+DC80..U+DCFF (`surrogateescape`), held as a
/// [`jsonio::Unit::Surrogate`]. Printing such a name fails like the baseline's
/// `UnicodeEncodeError` (see [`jsonio::encode`]).
pub fn py_text(name: &OsStr) -> String {
    let mut units = Vec::new();
    for chunk in name.as_bytes().utf8_chunks() {
        units.extend(chunk.valid().chars().map(Unit::Char));
        for byte in chunk.invalid() {
            units.push(Unit::Surrogate(0xDC00 + u32::from(*byte)));
        }
    }
    jsonio::from_units(&units)
}

/// The file name [`py_text`] read: escaped bytes back to themselves.
pub fn os_name(text: &str) -> OsString {
    let mut out = Vec::new();
    for unit in jsonio::units(text) {
        match unit {
            Unit::Surrogate(code @ 0xDC80..=0xDCFF) => out.push((code - 0xDC00) as u8),
            Unit::Surrogate(_) => {
                out.extend_from_slice(jsonio::os_text(&jsonio::from_units(&[unit])).as_bytes());
            }
            Unit::Char(ch) => out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    OsString::from_vec(out)
}

/// `path` with each component read back through [`os_name`].
pub fn os_path(path: &Path) -> PathBuf {
    PathBuf::from(os_name(&path.to_string_lossy()))
}

/// [`crate::paths::join`] for a name read from an instance file: the escaped bytes
/// of [`py_text`] go back to themselves, as the baseline's path objects did.
pub fn join_text(base: &Path, rel: &str) -> PathBuf {
    let lexical = os_path(&crate::paths::join(Path::new(""), rel));
    if lexical.is_absolute() {
        lexical
    } else if lexical == Path::new(".") {
        crate::paths::join(base, "")
    } else {
        base.join(lexical)
    }
}

/// The last component of `path` as the baseline's text.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(py_text).unwrap_or_default()
}

/// Code point order of the baseline's strings: a surrogate sorts as its value.
pub fn py_order(a: &str, b: &str) -> Ordering {
    jsonio::py_cmp(a, b)
}

/// The entries of a folder, sorted by name (code point order).
pub fn sorted_entries(dir: &Path) -> LintResult<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| LintError::io(dir, &e))? {
        entries.push(entry.map_err(|e| LintError::io(dir, &e))?.path());
    }
    entries.sort_by(|a, b| py_order(&file_name(a), &file_name(b)));
    Ok(entries)
}

/// Every file below `root` as relative parts, hidden ones included. Folders reached
/// through a symbolic link are not entered; a linked file counts. Sorted part by part,
/// so `a/x` comes before `a-b/x`.
pub fn files_below(root: &Path) -> LintResult<Vec<Vec<String>>> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), Vec::<String>::new())];
    while let Some((dir, parts)) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| LintError::io(&dir, &e))? {
            let entry = entry.map_err(|e| LintError::io(&dir, &e))?;
            let path = entry.path();
            let mut rel = parts.clone();
            rel.push(py_text(&entry.file_name()));
            let real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if real_dir {
                pending.push((path.clone(), rel.clone()));
            }
            if is_file(&path) {
                found.push(rel);
            }
        }
    }
    found.sort_by(|a, b| compare_parts(a, b));
    Ok(found)
}

fn compare_parts(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        match py_order(x, y) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    a.len().cmp(&b.len())
}

/// `Path.suffix`: the last dotted part of a name that does not start or end with it.
pub fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_like_path_objects() {
        assert_eq!(suffix("a.json"), ".json");
        assert_eq!(suffix(".json"), "");
        assert_eq!(suffix("a."), "");
        assert_eq!(suffix("a.b.json"), ".json");
    }

    #[test]
    fn blank_values() {
        assert!(blank(None));
        assert!(blank(Some(&Value::from(" \n"))));
        assert!(blank(Some(&Value::from(3))));
        assert!(!blank(Some(&Value::from("x"))));
    }

    #[test]
    fn undecodable_name_bytes_round_trip_as_escapes() {
        let raw = OsString::from_vec(b"c\xfe-\xc3\xa9\xed\xa0\x80".to_vec());
        let text = py_text(&raw);
        let codes: Vec<u32> = jsonio::code_points(&text).collect();
        assert_eq!(codes, [0x63, 0xDCFE, 0x2D, 0xE9, 0xDCED, 0xDCA0, 0xDC80]);
        assert_eq!(os_name(&text), raw);
        assert_eq!(py_order(&text, "c\u{e000}"), Ordering::Less);
        assert_eq!(
            join_text(Path::new("/r"), "a/./b/"),
            PathBuf::from("/r/a/b")
        );
        assert_eq!(join_text(Path::new("/r"), "/x"), PathBuf::from("/x"));
        assert_eq!(join_text(Path::new("/r"), ""), PathBuf::from("/r"));
    }

    #[test]
    fn part_order_differs_from_text_order() {
        let mut v = [
            vec!["a-b".to_string(), "x".to_string()],
            vec!["a".to_string(), "x".to_string()],
        ];
        v.sort_by(|a, b| compare_parts(a, b));
        assert_eq!(v[0][0], "a");
    }
}
