//! Value semantics of the baseline executor: truthiness, loose equality, number
//! arithmetic, `str`/`repr` text, line splitting and whitespace stripping.
//!
//! State is held as `serde_json::Value` so unknown keys and key order survive a
//! read-modify-write round trip. These helpers give that value the meaning the
//! baseline gave it.

use std::cmp::Ordering;

use serde_json::{Map, Number, Value};

use crate::error::{PmError, Res};

pub type Obj = Map<String, Value>;

/// Truth value: null, false, 0, 0.0, "", [] and {} are false.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn opt_truthy(value: Option<&Value>) -> bool {
    value.is_some_and(truthy)
}

/// A number as the baseline sees it: bool counts as 0/1.
#[derive(Clone, Copy, Debug)]
pub enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    pub fn of(value: &Value) -> Option<Num> {
        match value {
            Value::Bool(b) => Some(Num::Int(i128::from(*b))),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(Num::Int(i128::from(i)))
                } else if let Some(u) = n.as_u64() {
                    Some(Num::Int(i128::from(u)))
                } else {
                    n.as_f64().map(Num::Float)
                }
            }
            _ => None,
        }
    }

    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    pub fn compare(self, other: Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(&b)),
            (a, b) => a.as_f64().partial_cmp(&b.as_f64()),
        }
    }

    pub fn plus(self, other: Num) -> Num {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Num::Int(a + b),
            (a, b) => Num::Float(a.as_f64() + b.as_f64()),
        }
    }

    pub fn to_value(self) -> Value {
        match self {
            Num::Int(i) => {
                if let Ok(v) = i64::try_from(i) {
                    Value::from(v)
                } else if let Ok(v) = u64::try_from(i) {
                    Value::from(v)
                } else {
                    Value::from(i as f64)
                }
            }
            Num::Float(f) => Number::from_f64(f).map_or(Value::Null, Value::Number),
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// A number or the baseline's TypeError.
pub fn num(value: &Value) -> Res<Num> {
    Num::of(value).ok_or_else(|| {
        PmError::Internal(format!(
            "TypeError: unsupported operand type: '{}'",
            type_name(value)
        ))
    })
}

/// `value + 1` as the baseline computed it.
pub fn plus_one(value: &Value) -> Res<Value> {
    Ok(num(value)?.plus(Num::Int(1)).to_value())
}

/// Ordering of two numbers; a non-number is the baseline's TypeError.
pub fn num_cmp(a: &Value, b: &Value) -> Res<Ordering> {
    let (x, y) = (num(a)?, num(b)?);
    Ok(x.compare(y).unwrap_or(Ordering::Equal))
}

/// `a >= b` where b is an integer budget.
pub fn ge_int(a: &Value, b: i64) -> Res<bool> {
    Ok(num_cmp(a, &Value::from(b))? != Ordering::Less)
}

/// An int that is not a bool.
pub fn is_int(value: &Value) -> bool {
    matches!(value, Value::Number(n) if !n.is_f64())
}

/// Loose equality: 1 == 1.0 == true, dicts compare without order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (num_only(a), num_only(b)) {
        return x.compare(y) == Some(Ordering::Equal);
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => false,
    }
}

fn num_only(value: &Value) -> Option<Num> {
    match value {
        Value::Bool(_) | Value::Number(_) => Num::of(value),
        _ => None,
    }
}

pub fn eq_str(value: Option<&Value>, text: &str) -> bool {
    matches!(value, Some(Value::String(s)) if s == text)
}

/// The text `str(value)` gives.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// The text `repr(value)` gives.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => number_text(n),
        Value::String(s) => repr_str(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), py_repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// `repr` of a string: single quotes unless the text holds a single quote and no double quote.
pub fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Text of a JSON number as the baseline printed it.
pub fn number_text(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        float_repr(n.as_f64().unwrap_or(0.0))
    }
}

/// Shortest round-trip float text with the baseline's exponent rule
/// (fixed for 1e-4 <= |x| < 1e16, otherwise `1e-05`, `1.5e+16`).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            let (a, b) = digits.split_at(point as usize);
            format!("{a}.{b}")
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let mant = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mant}e{esign}{:02}", exp.abs())
    }
}

/// `round(x, 3)`.
pub fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

/// The `{:g}` text of a float (six significant digits, no trailing zeros).
pub fn g_format(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 {
        return format!("{}", x as i64);
    }
    let text = format!("{x:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_string()
}

/// Whitespace as `str.isspace` sees it.
pub fn is_space(ch: char) -> bool {
    ch.is_whitespace() || ('\x1c'..='\x1f').contains(&ch)
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

fn is_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines()`: every Unicode line boundary, `\r\n` as one.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if is_line_break(ch) {
            lines.push(&text[start..i]);
            let mut end = i + ch.len_utf8();
            if ch == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Text-mode newline translation: `\r\n` and `\r` become `\n`.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]*$` (a trailing newline also matched in the baseline's `$`).
pub fn is_plain_name(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut chars = body.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `str.isdigit()` for file stems.
pub fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_numeric)
}

/// Build a JSON object from key/value pairs in the given order.
pub fn obj<I, K>(pairs: I) -> Obj
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

/// `dict[key]`: a missing key is the baseline's KeyError.
pub fn need<'a>(map: &'a Obj, key: &str) -> Res<&'a Value> {
    map.get(key)
        .ok_or_else(|| PmError::Internal(format!("KeyError: {}", repr_str(key))))
}

/// The object inside a value, or the baseline's TypeError/AttributeError.
pub fn as_obj(value: &Value) -> Res<&Obj> {
    value.as_object().ok_or_else(|| {
        PmError::Internal(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            type_name(value)
        ))
    })
}

pub fn as_obj_mut(value: &mut Value) -> Res<&mut Obj> {
    let name = type_name(value);
    value.as_object_mut().ok_or_else(|| {
        PmError::Internal(format!(
            "AttributeError: '{name}' object has no attribute 'get'"
        ))
    })
}

/// A list value, or the baseline's TypeError.
pub fn as_list(value: &Value) -> Res<&Vec<Value>> {
    value.as_array().ok_or_else(|| {
        PmError::Internal(format!(
            "TypeError: '{}' object is not a list",
            type_name(value)
        ))
    })
}

/// `value or []` as a list of values.
pub fn list_or_empty(value: Option<&Value>) -> Res<Vec<Value>> {
    match value {
        Some(v) if truthy(v) => match v {
            Value::Array(items) => Ok(items.clone()),
            Value::String(s) => Ok(s.chars().map(|c| Value::from(c.to_string())).collect()),
            Value::Object(m) => Ok(m.keys().map(|k| Value::from(k.clone())).collect()),
            other => Err(PmError::Internal(format!(
                "TypeError: '{}' object is not iterable",
                type_name(other)
            ))),
        },
        _ => Ok(Vec::new()),
    }
}

/// Membership `needle in haystack` for a list value.
pub fn contains(haystack: &[Value], needle: &Value) -> bool {
    haystack.iter().any(|item| py_eq(item, needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_text_matches_baseline_repr() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(0.012), "0.012");
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1.5e-5), "1.5e-05");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(123456.789), "123456.789");
    }

    #[test]
    fn splitlines_breaks_on_unicode_boundaries() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\u{2028}e\n"),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
    }

    #[test]
    fn loose_equality() {
        assert!(py_eq(&Value::from(1), &Value::from(1.0)));
        assert!(py_eq(&Value::from(true), &Value::from(1)));
        assert!(!py_eq(&Value::from("1"), &Value::from(1)));
    }

    #[test]
    fn names_and_repr() {
        assert!(is_plain_name("c10-pm"));
        assert!(!is_plain_name("_x"));
        assert!(!is_plain_name("a/b"));
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a"), "'a'");
        assert_eq!(g_format(30.0), "30");
        assert_eq!(g_format(0.5), "0.5");
    }
}
