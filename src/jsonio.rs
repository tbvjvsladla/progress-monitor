//! JSON text exactly as the baseline wrote it, and the instance's file I/O.
//!
//! Three layouts: pretty (indent 2, file and stdout), compact (`", "` and `": "`,
//! journey lines and the Stop hook) and sorted compact (keys sorted recursively,
//! the input of every hash). Strings keep non-ASCII text; `"`, `\`, and control
//! characters are escaped with lowercase `\u00xx`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;

use crate::error::{PmError, Res};
use crate::py::number_text;

pub fn parse(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

fn escape(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn scalar(out: &mut String, value: &Value) -> bool {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&number_text(n)),
        Value::String(s) => escape(out, s),
        _ => return false,
    }
    true
}

fn write_pretty(out: &mut String, value: &Value, depth: usize) {
    if scalar(out, value) {
        return;
    }
    let pad = |out: &mut String, d: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(d));
    };
    match value {
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_pretty(out, item, depth + 1);
            }
            pad(out, depth);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                escape(out, key);
                out.push_str(": ");
                write_pretty(out, item, depth + 1);
            }
            pad(out, depth);
            out.push('}');
        }
        _ => {}
    }
}

fn write_compact(out: &mut String, value: &Value, sort: bool) {
    if scalar(out, value) {
        return;
    }
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_compact(out, item, sort);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if sort {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                escape(out, key);
                out.push_str(": ");
                write_compact(out, item, sort);
            }
            out.push('}');
        }
        _ => {}
    }
}

/// Indent 2, no trailing newline.
pub fn pretty(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, value, 0);
    out
}

/// One line with `", "` and `": "`.
pub fn compact(value: &Value) -> String {
    let mut out = String::new();
    write_compact(&mut out, value, false);
    out
}

/// One line, keys sorted at every level: the hash input.
pub fn sorted_compact(value: &Value) -> String {
    let mut out = String::new();
    write_compact(&mut out, value, true);
    out
}

/// SHA-256 hex of the sorted compact text.
pub fn digest(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(sorted_compact(value).as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read a JSON file. Missing: `default` when given, else a usage error.
pub fn read_json(path: &Path, default: Option<&Value>) -> Res<Value> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return match default {
                Some(v) => Ok(v.clone()),
                None => Err(PmError::usage(format!("{}: missing", path.display()))),
            };
        }
        Err(err) => return Err(PmError::os(&err)),
    };
    let text = String::from_utf8(bytes)
        .map_err(|e| PmError::usage(format!("{}: not JSON ({e})", path.display())))?;
    parse(&text).map_err(|e| PmError::usage(format!("{}: not JSON ({e})", path.display())))
}

/// Write pretty JSON plus a newline through `<name>.tmp` and a rename.
pub fn write_json(path: &Path, value: &Value) -> Res<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PmError::os(&e))?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let temp = path.with_file_name(name);
    fs::write(&temp, pretty(value) + "\n").map_err(|e| PmError::os(&e))?;
    fs::rename(&temp, path).map_err(|e| PmError::os(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layouts_match_baseline_bytes() {
        let v = json!({"b": [1, 2.0, {}], "a": "é\n\u{1}", "c": []});
        assert_eq!(
            pretty(&v),
            "{\n  \"b\": [\n    1,\n    2.0,\n    {}\n  ],\n  \"a\": \"é\\n\\u0001\",\n  \"c\": []\n}"
        );
        assert_eq!(
            compact(&v),
            "{\"b\": [1, 2.0, {}], \"a\": \"é\\n\\u0001\", \"c\": []}"
        );
        assert_eq!(
            sorted_compact(&json!({"z": {"y": 1, "x": null}, "a": true})),
            "{\"a\": true, \"z\": {\"x\": null, \"y\": 1}}"
        );
    }

    #[test]
    fn digest_of_known_text() {
        // sha256 of the text {"a": 1}
        assert_eq!(
            digest(&json!({"a": 1})),
            "f9d86028c6e0d64e225186f96acb69338b2c59764df79162107f5c4bb34d1310"
        );
    }
}
