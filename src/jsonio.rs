//! JSON text exactly as the baseline read and wrote it, and the instance's file I/O.
//!
//! Reading follows `json.loads`: `NaN`, `Infinity` and `-Infinity` are numbers,
//! integers keep every digit (up to the 4300-digit conversion limit), floats are
//! read with correct rounding, `-0` is the integer 0, and a lone surrogate escape
//! such as `\ud800` is kept (as [`MARK`] and a stand-in character, see [`Unit`]).
//! Numbers are held as their output text (serde_json `arbitrary_precision`).
//!
//! Three layouts: pretty (indent 2, file and stdout), compact (`", "` and `": "`,
//! journey lines and the Stop hook) and sorted compact (keys sorted recursively,
//! the input of every hash). Strings keep non-ASCII text; `"`, `\`, and control
//! characters are escaped with lowercase `\u00xx`. Text holding a lone surrogate
//! cannot be encoded as UTF-8: writing it fails as the baseline's
//! `UnicodeEncodeError` did ([`encode`]).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::{Map, Number, Value};

use crate::error::{PmError, Res};
use crate::py::{decode_error, number_text};

/// First code point of the stand-ins for the lone surrogates U+D800..U+DFFF
/// (plane 16 private use, U+10F000..U+10F7FF). A stand-in is a surrogate only
/// right after [`MARK`]; alone it is the real character.
const SURROGATE_BASE: u32 = 0x10_F000;

/// Leads a surrogate's stand-in. A real U+10FFFF is written as two marks only
/// where it would otherwise lead (before a mark or a stand-in); elsewhere it is
/// itself, so text without surrogates is held exactly as its characters.
pub const MARK: char = '\u{10FFFF}';

/// Nesting depth at which the baseline's decoder raised `RecursionError`.
const MAX_DEPTH: usize = 9_997;

/// The baseline's limit on the digits of an integer read from text.
const INT_MAX_STR_DIGITS: usize = 4_300;

/// One element of the baseline's strings: a character or a lone surrogate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Char(char),
    Surrogate(u32),
}

impl Unit {
    /// The code point (a surrogate's own value).
    pub fn code(self) -> u32 {
        match self {
            Unit::Char(c) => c as u32,
            Unit::Surrogate(code) => code,
        }
    }
}

fn is_stand_in(ch: char) -> bool {
    (SURROGATE_BASE..SURROGATE_BASE + 0x800).contains(&(ch as u32))
}

fn stand_in(code: u32) -> char {
    char::from_u32(SURROGATE_BASE + (code - 0xD800)).expect("plane 16 is valid")
}

/// The held form of a sequence of units (see [`MARK`]).
pub fn from_units(units: &[Unit]) -> String {
    let mut out = String::with_capacity(units.len());
    for (k, unit) in units.iter().enumerate() {
        match *unit {
            Unit::Surrogate(code) => {
                out.push(MARK);
                out.push(stand_in(code));
            }
            Unit::Char(MARK) => {
                let leads = match units.get(k + 1) {
                    Some(Unit::Char(next)) => *next == MARK || is_stand_in(*next),
                    Some(Unit::Surrogate(_)) => true,
                    None => false,
                };
                out.push(MARK);
                if leads {
                    out.push(MARK);
                }
            }
            Unit::Char(c) => out.push(c),
        }
    }
    out
}

/// Real text (from an argument, a variable, a path) in the held form.
pub fn mark(text: &str) -> String {
    if !text.contains(MARK) {
        return text.to_string();
    }
    from_units(&text.chars().map(Unit::Char).collect::<Vec<_>>())
}

/// [`mark`] of a path's text.
pub fn path_text(path: &Path) -> String {
    mark(&path.display().to_string())
}

/// The units of a held string.
pub fn units(text: &str) -> impl Iterator<Item = Unit> + '_ {
    let mut chars = text.chars().peekable();
    std::iter::from_fn(move || {
        let ch = chars.next()?;
        if ch != MARK {
            return Some(Unit::Char(ch));
        }
        Some(match chars.peek() {
            Some(&MARK) => {
                chars.next();
                Unit::Char(MARK)
            }
            Some(&next) if is_stand_in(next) => {
                chars.next();
                Unit::Surrogate(next as u32 - SURROGATE_BASE + 0xD800)
            }
            _ => Unit::Char(MARK),
        })
    })
}

/// The code points of a held string, a surrogate as its own value.
pub fn code_points(text: &str) -> impl Iterator<Item = u32> + '_ {
    units(text).map(Unit::code)
}

/// Code point order of the baseline's strings.
pub fn py_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    code_points(a).cmp(code_points(b))
}

/// A held string as the text handed to the system (a path, a command, a
/// variable name): marks dropped, a surrogate left as its stand-in character.
pub fn os_text(text: &str) -> String {
    if !text.contains(MARK) {
        return text.to_string();
    }
    units(text)
        .map(|u| match u {
            Unit::Char(c) => c,
            Unit::Surrogate(code) => stand_in(code),
        })
        .collect()
}

/// Why a text did not decode.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonError {
    /// `json.JSONDecodeError`: the message with line, column and char.
    Decode(String),
    /// Another exception the decoder raised, as `Name: message`
    /// (`RecursionError`, the integer `ValueError`).
    Other(String),
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonError::Decode(text) | JsonError::Other(text) => f.write_str(text),
        }
    }
}

impl JsonError {
    /// The baseline caught only `JSONDecodeError`; anything else ended the command.
    pub fn into_uncaught(self) -> Option<PmError> {
        match self {
            JsonError::Decode(_) => None,
            JsonError::Other(text) => Some(PmError::Internal(text)),
        }
    }
}

struct Parser<'a> {
    chars: Vec<char>,
    text: &'a str,
    depth: usize,
}

fn is_ws(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    fn fail(&self, msg: &str, pos: usize) -> JsonError {
        let line = self.chars[..pos].iter().filter(|c| **c == '\n').count() + 1;
        let col = match self.chars[..pos].iter().rposition(|c| *c == '\n') {
            Some(nl) => pos - nl,
            None => pos + 1,
        };
        let _ = self.text;
        JsonError::Decode(format!("{msg}: line {line} column {col} (char {pos})"))
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(is_ws) {
            i += 1;
        }
        i
    }

    fn starts(&self, i: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }

    fn value(&mut self, i: usize) -> Result<(Value, usize), JsonError> {
        let Some(ch) = self.at(i) else {
            return Err(self.fail("Expecting value", i));
        };
        match ch {
            '"' => {
                let (s, end) = self.string(i + 1)?;
                Ok((Value::String(s), end))
            }
            '{' => self.nested(i + 1, true),
            '[' => self.nested(i + 1, false),
            'n' if self.starts(i, "null") => Ok((Value::Null, i + 4)),
            't' if self.starts(i, "true") => Ok((Value::Bool(true), i + 4)),
            'f' if self.starts(i, "false") => Ok((Value::Bool(false), i + 5)),
            'N' if self.starts(i, "NaN") => Ok((special("NaN"), i + 3)),
            'I' if self.starts(i, "Infinity") => Ok((special("Infinity"), i + 8)),
            '-' if self.starts(i, "-Infinity") => Ok((special("-Infinity"), i + 9)),
            _ => self.number(i),
        }
    }

    fn nested(&mut self, i: usize, object: bool) -> Result<(Value, usize), JsonError> {
        if self.depth >= MAX_DEPTH {
            let kind = if object { "object" } else { "array" };
            return Err(JsonError::Other(format!(
                "RecursionError: maximum recursion depth exceeded while decoding a JSON {kind} from a unicode string"
            )));
        }
        self.depth += 1;
        let out = if object {
            self.object(i)
        } else {
            self.array(i)
        };
        self.depth -= 1;
        out
    }

    fn object(&mut self, start: usize) -> Result<(Value, usize), JsonError> {
        let mut map = Map::new();
        let mut i = self.skip_ws(start);
        if self.at(i) == Some('}') {
            return Ok((Value::Object(map), i + 1));
        }
        loop {
            if self.at(i) != Some('"') {
                return Err(self.fail("Expecting property name enclosed in double quotes", i));
            }
            let (key, end) = self.string(i + 1)?;
            i = self.skip_ws(end);
            if self.at(i) != Some(':') {
                return Err(self.fail("Expecting ':' delimiter", i));
            }
            i = self.skip_ws(i + 1);
            let (value, end) = self.value(i)?;
            map.insert(key, value);
            i = self.skip_ws(end);
            match self.at(i) {
                Some('}') => return Ok((Value::Object(map), i + 1)),
                Some(',') => {}
                _ => return Err(self.fail("Expecting ',' delimiter", i)),
            }
            i = self.skip_ws(i + 1);
        }
    }

    fn array(&mut self, start: usize) -> Result<(Value, usize), JsonError> {
        let mut items = Vec::new();
        let mut i = self.skip_ws(start);
        if self.at(i) == Some(']') {
            return Ok((Value::Array(items), i + 1));
        }
        loop {
            let (value, end) = self.value(i)?;
            items.push(value);
            i = self.skip_ws(end);
            match self.at(i) {
                Some(']') => return Ok((Value::Array(items), i + 1)),
                Some(',') => {}
                _ => return Err(self.fail("Expecting ',' delimiter", i)),
            }
            i = self.skip_ws(i + 1);
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let mut code = 0;
        for k in 0..4 {
            code = code * 16 + self.at(at + k)?.to_digit(16)?;
        }
        Some(code)
    }

    /// The string whose body starts at `start` (after the opening quote).
    fn string(&self, start: usize) -> Result<(String, usize), JsonError> {
        let mut out: Vec<Unit> = Vec::new();
        let mut i = start;
        loop {
            let Some(ch) = self.at(i) else {
                return Err(self.fail("Unterminated string starting at", start - 1));
            };
            match ch {
                '"' => return Ok((from_units(&out), i + 1)),
                '\\' => {
                    let Some(esc) = self.at(i + 1) else {
                        return Err(self.fail("Unterminated string starting at", start - 1));
                    };
                    let simple = match esc {
                        '"' => Some('"'),
                        '\\' => Some('\\'),
                        '/' => Some('/'),
                        'b' => Some('\u{8}'),
                        'f' => Some('\u{c}'),
                        'n' => Some('\n'),
                        'r' => Some('\r'),
                        't' => Some('\t'),
                        'u' => None,
                        _ => return Err(self.fail("Invalid \\escape", i)),
                    };
                    if let Some(c) = simple {
                        out.push(Unit::Char(c));
                        i += 2;
                        continue;
                    }
                    // the baseline wanted one more character after the four digits
                    let code = match self.hex4(i + 2) {
                        Some(code) if i + 6 < self.chars.len() => code,
                        _ => return Err(self.fail("Invalid \\uXXXX escape", i + 1)),
                    };
                    i += 6;
                    if (0xD800..0xDC00).contains(&code)
                        && i + 6 < self.chars.len()
                        && self.at(i) == Some('\\')
                        && self.at(i + 1) == Some('u')
                    {
                        let Some(low) = self.hex4(i + 2) else {
                            return Err(self.fail("Invalid \\uXXXX escape", i + 1));
                        };
                        if (0xDC00..0xE000).contains(&low) {
                            let joined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            out.push(Unit::Char(
                                char::from_u32(joined).expect("a joined pair is a scalar"),
                            ));
                            i += 6;
                            continue;
                        }
                    }
                    if (0xD800..0xE000).contains(&code) {
                        out.push(Unit::Surrogate(code));
                    } else {
                        out.push(Unit::Char(char::from_u32(code).expect("not a surrogate")));
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(self.fail("Invalid control character at", i));
                }
                c => {
                    out.push(Unit::Char(c));
                    i += 1;
                }
            }
        }
    }

    fn digits_from(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        i
    }

    fn number(&self, start: usize) -> Result<(Value, usize), JsonError> {
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('1'..='9') => i = self.digits_from(i + 1),
            Some('0') => i += 1,
            _ => return Err(self.fail("Expecting value", start)),
        }
        let int_end = i;
        let mut is_float = false;
        if self.at(i) == Some('.') && self.at(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            i = self.digits_from(i + 2);
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i + 1;
            if matches!(self.at(j), Some('+' | '-')) {
                j += 1;
            }
            let k = self.digits_from(j);
            if k > j {
                is_float = true;
                i = k;
            }
        }
        if !is_float {
            i = int_end;
        }
        let token: String = self.chars[start..i].iter().collect();
        let number = if is_float {
            let x: f64 = token.parse().expect("a JSON float token parses");
            crate::py::float_repr(x)
        } else {
            let digits = token.trim_start_matches('-');
            if digits.len() > INT_MAX_STR_DIGITS {
                return Err(JsonError::Other(format!(
                    "ValueError: Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                    digits.len()
                )));
            }
            if digits == "0" {
                "0".to_string()
            } else {
                token
            }
        };
        Ok((Value::Number(Number::from_string_unchecked(number)), i))
    }
}

fn special(text: &str) -> Value {
    Value::Number(Number::from_string_unchecked(text.to_string()))
}

/// `json.loads(text)`.
pub fn parse(text: &str) -> Result<Value, JsonError> {
    let mut parser = Parser {
        chars: text.chars().collect(),
        text,
        depth: 0,
    };
    if parser.at(0) == Some('\u{feff}') {
        return Err(parser.fail("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = parser.skip_ws(0);
    let (value, end) = parser.value(start)?;
    let end = parser.skip_ws(end);
    if end != parser.chars.len() {
        return Err(parser.fail("Extra data", end));
    }
    Ok(value)
}

/// `json.loads(text)` where the baseline skipped a `JSONDecodeError`: Ok(None) for
/// one, an error for any other exception.
pub fn parse_or_skip(text: &str) -> Res<Option<Value>> {
    match parse(text) {
        Ok(value) => Ok(Some(value)),
        Err(JsonError::Decode(_)) => Ok(None),
        Err(JsonError::Other(text)) => Err(PmError::Internal(text)),
    }
}

/// `str.encode("utf-8")` of a held text: the real text, or the baseline's
/// `UnicodeEncodeError` when it holds a lone surrogate (positions count units).
pub fn encode(text: &str) -> Res<String> {
    if !text.contains(MARK) {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut all = units(text).enumerate().peekable();
    while let Some((pos, unit)) = all.next() {
        let code = match unit {
            Unit::Char(c) => {
                out.push(c);
                continue;
            }
            Unit::Surrogate(code) => code,
        };
        let mut end = pos + 1;
        while all
            .peek()
            .is_some_and(|(_, u)| matches!(u, Unit::Surrogate(_)))
        {
            all.next();
            end += 1;
        }
        let what = if end - pos == 1 {
            format!("character '\\u{code:04x}' in position {pos}")
        } else {
            format!("characters in position {pos}-{}", end - 1)
        };
        return Err(PmError::Internal(format!(
            "UnicodeEncodeError: 'utf-8' codec can't encode {what}: surrogates not allowed"
        )));
    }
    Ok(out)
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
                entries.sort_by(|a, b| py_cmp(a.0, b.0));
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
pub fn digest(value: &Value) -> Res<String> {
    use sha2::{Digest, Sha256};
    let text = encode(&sorted_compact(value))?;
    let hash = Sha256::digest(text.as_bytes());
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
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
    let text = String::from_utf8(bytes).map_err(|e| {
        PmError::usage(format!(
            "{}: not JSON ({})",
            path.display(),
            decode_error(&e)
        ))
    })?;
    parse(&text).map_err(|e| match e {
        JsonError::Decode(text) => PmError::usage(format!("{}: not JSON ({text})", path.display())),
        JsonError::Other(text) => PmError::Internal(text),
    })
}

/// Write pretty JSON plus a newline through `<name>.tmp` and a rename.
pub fn write_json(path: &Path, value: &Value) -> Res<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PmError::os(&e))?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let temp = path.with_file_name(name);
    let text = match encode(&(pretty(value) + "\n")) {
        Ok(text) => text,
        Err(err) => {
            // the baseline opened (emptied) the temporary file before the encoding failed
            fs::write(&temp, "").map_err(|e| PmError::os(&e))?;
            return Err(err);
        }
    };
    fs::write(&temp, text).map_err(|e| PmError::os(&e))?;
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
            digest(&json!({"a": 1})).unwrap(),
            "f9d86028c6e0d64e225186f96acb69338b2c59764df79162107f5c4bb34d1310"
        );
    }

    /// `json.loads` then `json.dumps` of the baseline (r1-K1, r1-K2).
    #[test]
    fn numbers_and_tokens_round_trip_as_baseline() {
        let cases = [
            ("-943305.0469559873", "-943305.0469559873"),
            ("100000000000000000000", "100000000000000000000"),
            ("-0", "0"),
            ("-0.0", "-0.0"),
            ("1e400", "Infinity"),
            ("[NaN, Infinity, -Infinity]", "[NaN, Infinity, -Infinity]"),
            ("1E5", "100000.0"),
            ("1e16", "1e+16"),
            ("0.0001", "0.0001"),
            ("0.00001", "1e-05"),
            ("5e-324", "5e-324"),
        ];
        for (text, want) in cases {
            assert_eq!(compact(&parse(text).unwrap()), want, "{text}");
        }
        let big = "1".repeat(4301);
        assert!(
            matches!(parse(&big), Err(JsonError::Other(m)) if m.starts_with("ValueError: Exceeds the limit (4300 digits)"))
        );
        assert_eq!(
            compact(&parse(&"9".repeat(4300)).unwrap()),
            "9".repeat(4300)
        );
    }

    /// Lone surrogates are kept and fail only when encoded (r1-K2).
    #[test]
    fn lone_surrogates_are_kept_until_encoded() {
        let v = parse(r#"["\ud800", "\ud83d\ude00", "\udc00\ud800x"]"#).unwrap();
        assert_eq!(
            units(v[0].as_str().unwrap()).collect::<Vec<_>>(),
            [Unit::Surrogate(0xD800)]
        );
        assert_eq!(v[1].as_str(), Some("\u{1F600}"));
        let line = compact(&v);
        assert_eq!(
            encode(&line).unwrap_err().to_string_for_test(),
            "UnicodeEncodeError: 'utf-8' codec can't encode character '\\ud800' in position 2: surrogates not allowed"
        );
        assert!(digest(&v).is_err());
        let two = compact(&v[2]);
        assert_eq!(
            encode(&two).unwrap_err().to_string_for_test(),
            "UnicodeEncodeError: 'utf-8' codec can't encode characters in position 1-2: surrogates not allowed"
        );
    }

    /// Real plane 16 characters (U+10F000..U+10F7FF, U+10FFFF) next to lone
    /// surrogates read and write as the baseline did (r2-4).
    #[test]
    fn real_plane_16_characters_beside_lone_surrogates() {
        let real = "\u{10F000}\u{10F7FF}\u{10FFFF}\u{10FFFF}\u{10F000}\u{10FFFF}";
        // the same text as literal characters and as escapes
        let text = format!(
            "[\"{real}\", \"\\udbfc\\udc00\\udbfd\\udfff\\udbff\\udfff\\udbff\\udfff\\udbfc\\udc00\\udbff\\udfff\"]"
        );
        let v = parse(&text).unwrap();
        assert_eq!(v[0], v[1]);
        let want: Vec<u32> = real.chars().map(|c| c as u32).collect();
        assert_eq!(
            code_points(v[0].as_str().unwrap()).collect::<Vec<_>>(),
            want
        );
        assert_eq!(
            encode(&compact(&v)).unwrap(),
            format!("[\"{real}\", \"{real}\"]")
        );
        assert_eq!(os_text(v[0].as_str().unwrap()), real);
        assert_eq!(mark(real), v[0].as_str().unwrap());

        // mixed: U+10F000, lone U+D800, U+10FFFF, lone U+DFFF, U+10FFFF at the end
        let mixed = parse("\"\u{10F000}\\ud800\u{10FFFF}\\udfff\u{10FFFF}\"").unwrap();
        let held = mixed.as_str().unwrap();
        assert_eq!(
            units(held).collect::<Vec<_>>(),
            [
                Unit::Char('\u{10F000}'),
                Unit::Surrogate(0xD800),
                Unit::Char(MARK),
                Unit::Surrogate(0xDFFF),
                Unit::Char(MARK),
            ]
        );
        assert_eq!(
            encode(&compact(&mixed)).unwrap_err().to_string_for_test(),
            "UnicodeEncodeError: 'utf-8' codec can't encode character '\\ud800' in position 2: surrogates not allowed"
        );
        // sort_keys orders a lone surrogate before U+E000 and real U+10F000 after it
        let keys = parse("{\"\u{10F000}\": 1, \"\u{e000}\": 2, \"\\ud800\": 3}").unwrap();
        let sorted = sorted_compact(&keys);
        let order: Vec<u32> = code_points(&sorted).filter(|c| *c > 0x7f).collect();
        assert_eq!(order, [0xD800, 0xE000, 0x10F000]);
    }

    /// Decoder messages and the errors that are not `JSONDecodeError`.
    #[test]
    fn decoder_errors_as_baseline() {
        let cases = [
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{\"a\":1,}",
                "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)",
            ),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            (
                "\"\\ud800",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            ("1 2", "Extra data: line 1 column 3 (char 2)"),
            ("-NaN", "Expecting value: line 1 column 1 (char 0)"),
            (
                "\u{feff}[]",
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
            ),
        ];
        for (text, want) in cases {
            assert_eq!(
                parse(text),
                Err(JsonError::Decode(want.to_string())),
                "{text}"
            );
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        let out = std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(move || parse(&deep).map(|_| ()))
            .unwrap()
            .join()
            .unwrap();
        assert!(matches!(out, Err(JsonError::Other(m)) if m.starts_with("RecursionError")));
    }
}
