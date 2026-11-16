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
        Value::Number(n) => number_f64(n) != 0.0,
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
            Value::Number(n) => match int_text(n) {
                Some(text) => Some(text.parse().map_or_else(
                    |_| Num::Float(number_f64(n)), // beyond i128: compared as a float
                    Num::Int,
                )),
                None => Some(Num::Float(number_f64(n))),
            },
            _ => None,
        }
    }

    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    /// Ordering as the baseline compared numbers: int with float exactly,
    /// None when a NaN is involved.
    pub fn compare(self, other: Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(&b)),
            (Num::Int(a), Num::Float(b)) => int_float_cmp(a, b),
            (Num::Float(a), Num::Int(b)) => int_float_cmp(b, a).map(Ordering::reverse),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(&b),
        }
    }

    /// `self >= other`; false when a NaN is involved.
    pub fn ge(self, other: Num) -> bool {
        matches!(
            self.compare(other),
            Some(Ordering::Greater | Ordering::Equal)
        )
    }

    /// `self > other`; false when a NaN is involved.
    pub fn gt(self, other: Num) -> bool {
        self.compare(other) == Some(Ordering::Greater)
    }

    /// `self + other`. An int sum outside i128 goes on as a float (intended change 8).
    pub fn plus(self, other: Num) -> Num {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => a
                .checked_add(b)
                .map_or_else(|| Num::Float(a as f64 + b as f64), Num::Int),
            (a, b) => Num::Float(a.as_f64() + b.as_f64()),
        }
    }

    pub fn to_value(self) -> Value {
        match self {
            Num::Int(i) => Value::Number(Number::from_string_unchecked(i.to_string())),
            Num::Float(f) => float_value(f),
        }
    }
}

/// `list.sort` with a `<` that may fail or be inconsistent (NaN): the
/// baseline's own algorithm step for step, so the order (or the error) is the
/// baseline's for any comparison.
pub fn py_sort<T>(items: &mut [T], lt: impl FnMut(&T, &T) -> Res<bool>) -> Res<()> {
    crate::sort::list_sort(items, lt)
}

/// Exact comparison of an int with a float (no rounding of the int).
fn int_float_cmp(a: i128, b: f64) -> Option<Ordering> {
    const LIMIT: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0; // 2**127
    if b.is_nan() {
        return None;
    }
    if b >= LIMIT {
        return Some(Ordering::Less);
    }
    if b < -LIMIT {
        return Some(Ordering::Greater);
    }
    let whole = b.trunc();
    let by_whole = a.cmp(&(whole as i128));
    Some(match by_whole {
        Ordering::Equal => 0.0f64.partial_cmp(&(b - whole)).unwrap_or(Ordering::Equal),
        other => other,
    })
}

/// `max(values)`: the first of the largest, compared with `>` as the baseline did.
pub fn py_max(values: &[Value]) -> Res<Option<Value>> {
    let mut best: Option<(&Value, Num)> = None;
    for value in values {
        let n = num(value)?;
        match best {
            Some((_, b)) if !n.gt(b) => {}
            _ => best = Some((value, n)),
        }
    }
    Ok(best.map(|(v, _)| v.clone()))
}

/// The baseline's type name of a value (`int`, `float`, `str`, `list`, `dict`, ...).
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if int_text(n).is_none() => "float",
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

/// `a >= b` for a value and a number (false when a NaN is involved).
pub fn ge_num(a: &Value, b: Num) -> Res<bool> {
    Ok(num(a)?.ge(b))
}

/// An int that is not a bool.
pub fn is_int(value: &Value) -> bool {
    matches!(value, Value::Number(n) if int_text(n).is_some())
}

/// Loose equality: 1 == 1.0 == true, dicts compare without order. Items inside
/// a list or dict compare as [`same_or_eq`], as the baseline's containers did.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (num_only(a), num_only(b)) {
        return x.compare(y) == Some(Ordering::Equal);
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_or_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_or_eq(v, w)))
        }
        _ => false,
    }
}

/// Equality as the baseline's containers, dict keys and `in` saw it: identity
/// first, then `==`. Every JSON `NaN` the baseline read was the decoder's one
/// NaN object, so two NaN values match here (and not under a bare `==`).
pub fn same_or_eq(a: &Value, b: &Value) -> bool {
    let nan = |v: &Value| {
        matches!(v, Value::Number(_)) && Num::of(v).is_some_and(|n| n.as_f64().is_nan())
    };
    (nan(a) && nan(b)) || py_eq(a, b)
}

/// A value that can be a dict key or set member, else the baseline's TypeError.
pub fn hashable(value: &Value) -> Res<&Value> {
    match value {
        Value::Array(_) | Value::Object(_) => Err(PmError::Internal(format!(
            "TypeError: unhashable type: '{}'",
            type_name(value)
        ))),
        _ => Ok(value),
    }
}

/// `a < b` for two JSON values: numbers (bool included) by value, strings by
/// code points, lists item by item (the first unequal pair decides, else the
/// shorter list is less). Any other pair is the baseline's TypeError.
pub fn py_lt(a: &Value, b: &Value) -> Res<bool> {
    if let (Some(x), Some(y)) = (num_only(a), num_only(b)) {
        return Ok(x.compare(y) == Some(Ordering::Less));
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Ok(crate::jsonio::py_cmp(x, y) == Ordering::Less),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !same_or_eq(p, q) {
                    return py_lt(p, q);
                }
            }
            Ok(x.len() < y.len())
        }
        _ => Err(PmError::Internal(format!(
            "TypeError: '<' not supported between instances of '{}' and '{}'",
            type_name(a),
            type_name(b)
        ))),
    }
}

/// `monotonic() + value`'s reading of a time limit in seconds: a number as a
/// float; an int beyond a float and a non-number are the baseline's errors.
pub fn seconds_of(value: &Value) -> Res<f64> {
    match value {
        Value::Number(n) if is_int(value) && number_f64(n).is_infinite() => Err(PmError::Internal(
            "OverflowError: int too large to convert to float".into(),
        )),
        Value::Number(_) | Value::Bool(_) => Ok(num(value)?.as_f64()),
        other => Err(PmError::Internal(format!(
            "TypeError: unsupported operand type(s) for +: 'float' and '{}'",
            type_name(other)
        ))),
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
        Value::Number(n) => match int_text(n) {
            Some(text) => text.to_string(),
            None => float_str(number_f64(n)),
        },
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
    for unit in crate::jsonio::units(text) {
        let ch = match unit {
            crate::jsonio::Unit::Char(c) => c,
            crate::jsonio::Unit::Surrogate(code) => {
                out.push_str(&format!("\\u{code:04x}"));
                continue;
            }
        };
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
            c if !crate::printable::is_printable(c) => {
                let code = c as u32;
                out.push_str(&if code < 0x100 {
                    format!("\\x{code:02x}")
                } else if code < 0x1_0000 {
                    format!("\\u{code:04x}")
                } else {
                    format!("\\U{code:08x}")
                });
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// The text of an int number (`-?digits`); None for a float.
pub fn int_text(n: &Number) -> Option<&str> {
    let text = n.as_str();
    let digits = text.strip_prefix('-').unwrap_or(text);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(text)
}

/// The value of a number as a float (`NaN` and the infinities included).
pub fn number_f64(n: &Number) -> f64 {
    n.as_str().parse().unwrap_or(f64::NAN)
}

/// A float number held as its `repr` text, `NaN` and `Infinity` included.
pub fn float_value(x: f64) -> Value {
    Value::Number(Number::from_string_unchecked(float_repr(x)))
}

/// Text of a JSON number as the baseline printed it.
pub fn number_text(n: &Number) -> String {
    match int_text(n) {
        Some(text) => text.to_string(),
        None => float_repr(number_f64(n)),
    }
}

/// The message of a UTF-8 `UnicodeDecodeError` for these bytes.
pub fn decode_error(err: &std::string::FromUtf8Error) -> String {
    utf8_error_text(err.as_bytes(), err.utf8_error())
}

/// [`decode_error`] from the bytes and the error.
pub fn utf8_error_text(bytes: &[u8], err: std::str::Utf8Error) -> String {
    let at = err.valid_up_to();
    let lead = bytes[at];
    let (len, reason) = match err.error_len() {
        Some(n) => {
            let start_ok = matches!(lead, 0xC2..=0xF4);
            (
                n,
                if start_ok {
                    "invalid continuation byte"
                } else {
                    "invalid start byte"
                },
            )
        }
        None => (bytes.len() - at, "unexpected end of data"),
    };
    if len == 1 {
        format!("'utf-8' codec can't decode byte 0x{lead:02x} in position {at}: {reason}")
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {at}-{}: {reason}",
            at + len - 1
        )
    }
}

/// `repr`/`str` of a float: `nan`, `inf`, `-inf`, else [`float_repr`].
pub fn float_str(x: f64) -> String {
    if x.is_nan() {
        "nan".to_string()
    } else if x.is_infinite() {
        if x > 0.0 { "inf" } else { "-inf" }.to_string()
    } else {
        float_repr(x)
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

/// The `{:g}` text of a float: six significant digits, trailing zeros dropped,
/// exponent form below 1e-4 and from 1e6 on (`1e-07`, `1.23457e+06`, `inf`, `nan`).
pub fn g_format(x: f64) -> String {
    if !x.is_finite() {
        return float_str(x);
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |t: &str| {
        if t.contains('.') {
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            t.to_string()
        }
    };
    if (-4..6).contains(&exp) {
        trim(&format!("{x:.*}", (5 - exp) as usize))
    } else {
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{}e{esign}{:02}", trim(mantissa), exp.abs())
    }
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
    fn less_than_and_seconds_follow_the_baseline() {
        let v = |t: &str| crate::jsonio::parse(t).unwrap();
        assert!(py_lt(&v("\"a\""), &v("\"b\"")).unwrap());
        assert!(py_lt(&v("[NaN, 1]"), &v("[NaN, 2]")).unwrap());
        assert!(py_lt(&v("[1]"), &v("[1, 0]")).unwrap());
        assert!(!py_lt(&v("NaN"), &v("1")).unwrap());
        assert!(py_lt(&v("{}"), &v("{}")).is_err());
        assert!(py_lt(&v("\"a\""), &v("1")).is_err());
        assert!(same_or_eq(&v("NaN"), &v("NaN")) && !py_eq(&v("NaN"), &v("NaN")));
        assert!(py_eq(&v("[NaN]"), &v("[NaN]")));
        let big = format!("1{}", "0".repeat(400));
        assert!(seconds_of(&v(&big)).is_err());
        assert_eq!(seconds_of(&v("true")).unwrap(), 1.0);
        assert!(seconds_of(&v("Infinity")).unwrap().is_infinite());
        assert!(seconds_of(&v("\"5\"")).is_err());
    }

    #[test]
    fn names_and_repr() {
        assert!(is_plain_name("c10-pm"));
        assert!(!is_plain_name("_x"));
        assert!(!is_plain_name("a/b"));
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a"), "'a'");
        assert_eq!(
            repr_str("\u{85}\u{a0}\u{ad}\u{200b}\u{e000}\u{10f000}\u{10ffff}é"),
            "'\\x85\\xa0\\xad\\u200b\\ue000\\U0010f000\\U0010ffff\u{e9}'"
        );
        assert_eq!(g_format(30.0), "30");
        assert_eq!(g_format(0.5), "0.5");
    }
}

/// First code points of the Unicode decimal digit runs (`0` of each run of ten),
/// from the Unicode 15.0 database of the baseline interpreter (3.12).
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
    0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
    0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
    0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650,
    0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60, 0x16ac0,
    0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950,
    0x1fbf0,
];

fn decimal_value(ch: char) -> Option<u32> {
    let c = ch as u32;
    DECIMAL_ZEROS
        .iter()
        .find(|zero| (**zero..**zero + 10).contains(&c))
        .map(|zero| c - zero)
}

/// `float(text)` for a str: Unicode digits and spaces, underscores between digits,
/// `inf`/`infinity`/`nan` in any case. None where the baseline raised ValueError.
pub fn py_float(text: &str) -> Option<f64> {
    // _PyUnicode_TransformDecimalAndSpaceToASCII
    let ascii: String = text
        .chars()
        .map(|ch| {
            if (ch as u32) < 127 {
                ch
            } else if is_space(ch) {
                ' '
            } else if let Some(d) = decimal_value(ch) {
                char::from_digit(d, 10).unwrap_or('?')
            } else {
                '?'
            }
        })
        .collect();
    let body =
        ascii.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let body = if body.contains('_') {
        let mut out = String::with_capacity(body.len());
        let mut prev = '\0';
        for ch in body.chars() {
            if ch == '_' {
                if !prev.is_ascii_digit() {
                    return None;
                }
            } else {
                if prev == '_' && !ch.is_ascii_digit() {
                    return None;
                }
                out.push(ch);
            }
            prev = ch;
        }
        if prev == '_' {
            return None;
        }
        out
    } else {
        body.to_string()
    };
    let unsigned = body.strip_prefix(['+', '-']).unwrap_or(&body);
    let word = unsigned.to_ascii_lowercase();
    if matches!(word.as_str(), "inf" | "infinity" | "nan") {
        return body.parse().ok();
    }
    // digits with at most one point and an optional exponent; no other spelling
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let point_ok = mantissa.matches('.').count() <= 1
        && mantissa.chars().all(|c| c.is_ascii_digit() || c == '.')
        && mantissa.chars().any(|c| c.is_ascii_digit());
    let exp_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && e.chars().all(|c| c.is_ascii_digit())
    });
    if !(point_ok && exp_ok) {
        return None;
    }
    body.parse().ok()
}

/// `int(value)` for a value read from JSON. An int outside i128 (and the int of
/// a float outside it) goes on as a float (intended change 8).
pub fn py_int(value: &Value) -> Res<Num> {
    match value {
        Value::Bool(b) => Ok(Num::Int(i128::from(*b))),
        Value::Number(n) => match Num::of(value) {
            Some(whole) if is_int(value) => Ok(whole), // outside i128 it stays a float
            _ => float_to_int(number_f64(n)),
        },
        Value::String(s) => int_of_text(s).ok_or_else(|| {
            let shown: String = repr_str(s).chars().take(200).collect();
            PmError::Internal(format!(
                "ValueError: invalid literal for int() with base 10: {shown}"
            ))
        })?,
        other => Err(PmError::Internal(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            type_name(other)
        ))),
    }
}

fn float_to_int(x: f64) -> Res<Num> {
    const LIMIT: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0; // 2**127
    if x.is_nan() {
        return Err(PmError::Internal(
            "ValueError: cannot convert float NaN to integer".into(),
        ));
    }
    if x.is_infinite() {
        return Err(PmError::Internal(
            "OverflowError: cannot convert float infinity to integer".into(),
        ));
    }
    let whole = x.trunc();
    Ok(if (-LIMIT..LIMIT).contains(&whole) {
        Num::Int(whole as i128)
    } else {
        Num::Float(whole)
    })
}

/// `int(text)` in base 10: Unicode digits and spaces, a sign, underscores
/// between digits. None where the baseline raised `invalid literal`; the
/// digit-count limit is the baseline's error too.
fn int_of_text(text: &str) -> Option<Res<Num>> {
    let mut ascii = String::with_capacity(text.len());
    for ch in text.chars() {
        if (ch as u32) < 127 {
            ascii.push(ch);
        } else if is_space(ch) {
            ascii.push(' ');
        } else {
            ascii.push(char::from_digit(decimal_value(ch)?, 10)?);
        }
    }
    let body =
        ascii.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let (negative, unsigned) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    let mut digits = String::with_capacity(unsigned.len());
    let mut prev = '_';
    for ch in unsigned.chars() {
        if ch == '_' {
            if prev == '_' {
                return None;
            }
        } else if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            return None;
        }
        prev = ch;
    }
    if digits.is_empty() || prev == '_' {
        return None;
    }
    if digits.len() > 4300 {
        return Some(Err(PmError::Internal(format!(
            "ValueError: Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
            digits.len()
        ))));
    }
    let signed = if negative {
        format!("-{digits}")
    } else {
        digits
    };
    Some(Ok(signed.parse::<i128>().map_or_else(
        |_| Num::Float(signed.parse().unwrap_or(f64::NAN)),
        Num::Int,
    )))
}

#[cfg(test)]
mod float_tests {
    use super::*;

    #[test]
    fn float_of_text_as_baseline() {
        assert_eq!(py_float("1_0"), Some(10.0));
        assert_eq!(py_float(" 1 \n"), Some(1.0));
        assert_eq!(py_float("-INF"), Some(f64::NEG_INFINITY));
        assert!(py_float("nan").is_some_and(f64::is_nan));
        assert_eq!(py_float("Infinity"), Some(f64::INFINITY));
        assert_eq!(py_float("+5"), Some(5.0));
        assert_eq!(py_float("\u{0661}\u{0662}"), Some(12.0));
        assert_eq!(py_float("1_0.5_5e1_0"), Some(10.55e10));
        for bad in [
            "0x1", "1__0", "_1", "1_", "", "1e", ".", "1.2.3", "inf_", "é",
        ] {
            assert_eq!(py_float(bad), None, "{bad}");
        }
    }

    #[test]
    fn number_edges_as_baseline() {
        // r2-5: an int sum past i128 goes on as a float, never wraps
        let top = Num::Int(i128::MAX);
        assert!(matches!(top.plus(Num::Int(1)), Num::Float(x) if x > 1.7e38));
        assert!(matches!(Num::Int(i128::MIN).plus(Num::Int(-1)), Num::Float(x) if x < -1.7e38));
        // ints compare with floats exactly; NaN compares false both ways
        let big = Num::Int(9_007_199_254_740_993);
        assert!(big.gt(Num::Float(9_007_199_254_740_992.0)));
        assert!(Num::Int(2).gt(Num::Float(1.5)) && !Num::Int(1).gt(Num::Float(1.5)));
        assert!(!Num::Float(f64::NAN).ge(Num::Int(0)) && !Num::Int(0).ge(Num::Float(f64::NAN)));
        assert!(Num::Float(1e300).gt(top) && Num::Int(i128::MIN).gt(Num::Float(-1e300)));
        // r2-8: ASCII below 127 is kept as it is, so \x1c-\x1f are not spaces for float()
        for bad in ["\x1c5", "5\x1f", "\x7f5"] {
            assert_eq!(py_float(bad), None, "{bad:?}");
        }
        assert_eq!(py_float("\u{2000}5\u{3000}"), Some(5.0));
        // `{:g}`
        assert_eq!(g_format(-1e20), "-1e+20");
        assert_eq!(g_format(-1234567.0), "-1.23457e+06");
        assert_eq!(g_format(-1e-7), "-1e-07");
        assert_eq!(g_format(123456.0), "123456");
        assert_eq!(g_format(0.0001), "0.0001");
        assert_eq!(g_format(-0.0), "-0");
        assert_eq!(g_format(f64::NAN), "nan");
        // int()
        let int = |v: &str| py_int(&crate::jsonio::parse(v).unwrap()).map(|n| n.to_value());
        assert_eq!(int("1e300").unwrap(), float_value(1e300));
        assert_eq!(int("-1.5").unwrap(), Value::from(-1));
        assert_eq!(int("\" +1_0 \"").unwrap(), Value::from(10));
        assert!(int("NaN").is_err() && int("\"1__0\"").is_err() && int("[]").is_err());
        // max() keeps the first of the largest
        let max = |v: &str| py_max(crate::jsonio::parse(v).unwrap().as_array().unwrap()).unwrap();
        assert_eq!(max("[0.0, 0, NaN]"), Some(float_value(0.0)));
        assert_eq!(max("[0.0, 0, NaN, 5]"), Some(Value::from(5)));
        assert!(max("[NaN, 5]").is_some_and(|v| v.as_number().is_some_and(|n| n.as_str() == "NaN")));
        // repr of non-finite floats
        assert_eq!(py_repr(&float_value(f64::NEG_INFINITY)), "-inf");
    }

    #[test]
    fn sort_follows_the_baseline_with_nan() {
        let mut items = vec![3.0, f64::NAN, 1.0, 2.0];
        py_sort(&mut items, |a, b| Ok(a < b)).unwrap();
        // the baseline: sorted([3.0, nan, 1.0, 2.0]) == [3.0, nan, 1.0, 2.0]
        assert_eq!(format!("{items:?}"), "[3.0, NaN, 1.0, 2.0]");
        let mut items = vec![5, 1, 4, 1, 3];
        py_sort(&mut items, |a, b| Ok(a < b)).unwrap();
        assert_eq!(items, [1, 1, 3, 4, 5]);
    }

    #[test]
    fn numbers_keep_baseline_text() {
        let v = crate::jsonio::parse(
            "[1e400, -0, -0.0, 1E5, 100000000000000000000, NaN, -Infinity, -943305.0469559873]",
        )
        .unwrap();
        assert_eq!(
            crate::jsonio::compact(&v),
            "[Infinity, 0, -0.0, 100000.0, 100000000000000000000, NaN, -Infinity, -943305.0469559873]"
        );
        assert!(is_int(&v[4]) && !is_int(&v[5]));
        assert!(truthy(&v[5]) && !truthy(&v[1]) && !truthy(&v[2]));
    }
}
