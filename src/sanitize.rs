//! Distribution hygiene scan: site names, personal home paths, private IPs, e-mail
//! addresses and the identifiers of the projects this package was extracted from.
//!
//! The five patterns are matched by hand (no pattern crate). Digits and case folding
//! are ASCII; the line is otherwise read as Unicode text.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::py::splitlines;

/// Folders whose contents are never scanned (any path part with one of these names).
pub const EXCLUDED_DIRS: [&str; 5] = [".git", "__pycache__", ".cache", "node_modules", "target"];

/// Pattern names in report order.
pub const NAMES: [&str; 5] = [
    "site_name",
    "home_path",
    "private_ip",
    "email",
    "project_identifier",
];

// Written in pieces so this file does not match itself.
const SITE_NAMES: [&[&str]; 2] = [
    &["\u{acbd}\u{b9ac}", "\u{b098}\u{b77c}"],
    &["kyung", "rinara"],
];
/// `head` + one of `_` or `-` + `tail`, case-insensitive.
const PROJECT_JOINED: [(&[&str], &str); 3] = [
    (&["easy"], "vllm"),
    (&["vllm"], "simulator"),
    (&["her", "mes"], "subagent"),
];
const PROJECT_PLAIN: [&str; 2] = ["val", "_agent"];

/// One hit: path relative to the root (with `/`), 1-based line number, pattern name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rel: String,
    pub line: usize,
    pub name: &'static str,
}

/// The names of the patterns that occur in `line`, in report order.
pub fn line_hits(line: &str) -> Vec<&'static str> {
    let chars: Vec<char> = line.chars().collect();
    let tests: [fn(&[char]) -> bool; 5] =
        [site_name, home_path, private_ip, email, project_identifier];
    NAMES
        .iter()
        .zip(tests)
        .filter(|(_, test)| test(&chars))
        .map(|(name, _)| *name)
        .collect()
}

fn pieces(parts: &[&str]) -> Vec<char> {
    parts.concat().chars().collect()
}

/// Whether `needle` occurs in `hay` with ASCII letters compared case-insensitively.
fn contains_fold(hay: &[char], needle: &[char]) -> bool {
    !needle.is_empty()
        && hay.len() >= needle.len()
        && hay
            .windows(needle.len())
            .any(|w| w.iter().zip(needle).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

fn site_name(line: &[char]) -> bool {
    SITE_NAMES
        .iter()
        .any(|parts| contains_fold(line, &pieces(parts)))
}

fn project_identifier(line: &[char]) -> bool {
    contains_fold(line, &pieces(&PROJECT_PLAIN))
        || PROJECT_JOINED.iter().any(|(head, tail)| {
            ["_", "-"].iter().any(|sep| {
                let joined = [head.concat().as_str(), sep, tail].concat();
                contains_fold(line, &joined.chars().collect::<Vec<_>>())
            })
        })
}

fn starts_with(line: &[char], at: usize, text: &str) -> bool {
    (at..)
        .zip(text.chars())
        .all(|(i, ch)| line.get(i) == Some(&ch))
}

fn run_len(line: &[char], at: usize, class: impl Fn(char) -> bool) -> usize {
    line[at.min(line.len())..]
        .iter()
        .take_while(|c| class(**c))
        .count()
}

/// `/home/[a-z_][a-z0-9_-]*/`
fn home_path(line: &[char]) -> bool {
    let prefix = ["/ho", "me/"].concat();
    (0..line.len()).any(|i| {
        if !starts_with(line, i, &prefix) {
            return false;
        }
        let start = i + prefix.len();
        let first = line.get(start);
        if !first.is_some_and(|c| c.is_ascii_lowercase() || *c == '_') {
            return false;
        }
        let n = run_len(line, start, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
        });
        line.get(start + n) == Some(&'/')
    })
}

/// `\d{1,3}` then `.` at `at`; returns the position after the dot.
fn octet_dot(line: &[char], at: usize) -> Option<usize> {
    let n = run_len(line, at, |c| c.is_ascii_digit());
    ((1..=3).contains(&n) && line.get(at + n) == Some(&'.')).then_some(at + n + 1)
}

/// `(?<![0-9.])(?:10\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])|192\.168)\.\d{1,3}\.\d{1,3}(?![0-9])`
fn private_ip(line: &[char]) -> bool {
    let digit = |c: Option<&char>| c.is_some_and(|c| c.is_ascii_digit());
    (0..line.len()).any(|i| {
        if i > 0 && (line[i - 1].is_ascii_digit() || line[i - 1] == '.') {
            return false;
        }
        let after_head = if starts_with(line, i, "10.") {
            octet_dot(line, i + 3)
        } else if starts_with(line, i, "172.") {
            let (a, b) = (line.get(i + 4), line.get(i + 5));
            let second = match a {
                Some('1') => b.is_some_and(|c| ('6'..='9').contains(c)),
                Some('2') => digit(b),
                Some('3') => matches!(b, Some('0') | Some('1')),
                _ => false,
            };
            (second && line.get(i + 6) == Some(&'.')).then_some(i + 7)
        } else if starts_with(line, i, "192.168.") {
            Some(i + 8)
        } else {
            None
        };
        let Some(third) = after_head.and_then(|at| octet_dot(line, at)) else {
            return false;
        };
        let n = run_len(line, third, |c| c.is_ascii_digit());
        (1..=3).contains(&n)
    })
}

fn local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._%+-".contains(c)
}

fn domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-'
}

/// A word character of a Unicode text pattern: letters, numbers and `_`.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `example\.(?:invalid|com|org)\b` at `at`.
fn allowed_domain(line: &[char], at: usize) -> bool {
    let base = "example.";
    if !starts_with(line, at, base) {
        return false;
    }
    let at = at + base.len();
    ["invalid", "com", "org"].iter().any(|tld| {
        starts_with(line, at, tld) && !line.get(at + tld.len()).is_some_and(|c| word_char(*c))
    })
}

/// `[A-Za-z0-9._%+-]+@(?!example\.(?:invalid|com|org)\b)[A-Za-z0-9.-]+\.[A-Za-z]{2,}`
fn email(line: &[char]) -> bool {
    (1..line.len()).any(|at| {
        if line[at] != '@' || !local_char(line[at - 1]) || allowed_domain(line, at + 1) {
            return false;
        }
        let start = at + 1;
        let n = run_len(line, start, domain_char);
        (1..n).any(|i| {
            line[start + i] == '.'
                && line
                    .get(start + i + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && line
                    .get(start + i + 2)
                    .is_some_and(|c| c.is_ascii_alphabetic())
        })
    })
}

/// Every path under `dir`, sorted by parts; directory links are listed but not entered,
/// and excluded folders are not entered.
fn walk(
    dir: &Path,
    prefix: &mut Vec<OsString>,
    out: &mut Vec<Vec<OsString>>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if EXCLUDED_DIRS.iter().any(|x| name == *x) {
            continue;
        }
        prefix.push(name);
        out.push(prefix.clone());
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), prefix, out)?;
        }
        prefix.pop();
    }
    Ok(())
}

/// Scan every regular file (links to files included) under `root`.
/// A file with a NUL byte is skipped; other bytes are read as UTF-8 with replacement.
pub fn scan(root: &Path) -> Result<Vec<Hit>, String> {
    let mut found = Vec::new();
    walk(root, &mut Vec::new(), &mut found).map_err(|e| format!("{}: {e}", root.display()))?;
    found.sort();
    let mut hits = Vec::new();
    for parts in found {
        let rel: PathBuf = parts.iter().collect();
        let path = root.join(&rel);
        if !path.is_file() {
            continue;
        }
        let data = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if data.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&data);
        let shown = parts
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        for (index, line) in splitlines(&text).into_iter().enumerate() {
            for name in line_hits(line) {
                hits.push(Hit {
                    rel: shown.clone(),
                    line: index + 1,
                    name,
                });
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::line_hits;

    /// Cases are written with `~` inside the token so this file stays clean; the
    /// expected names were taken from the baseline patterns on the same lines.
    const CASES: [(&str, &[&str]); 64] = [
        ("10~.1.2.3", &["private_ip"]),
        ("110~.1.2.3", &[]),
        ("a10~.1.2.3", &["private_ip"]),
        (".10~.1.2.3", &[]),
        ("10~.1.2.3.", &["private_ip"]),
        ("10~.1.2.3456", &[]),
        ("10~.1234.2.3", &[]),
        ("10~.1.2", &[]),
        ("x10~.0.0.1y", &["private_ip"]),
        ("1.10~.0.0.1", &[]),
        ("172~.15.0.1", &[]),
        ("172~.16.0.1", &["private_ip"]),
        ("172~.31.255.255", &["private_ip"]),
        ("172~.32.0.1", &[]),
        ("172~.20.0.1", &["private_ip"]),
        ("172~.2.0.1", &[]),
        ("192~.168.0.1", &["private_ip"]),
        ("192~.169.0.1", &[]),
        ("192~.168.1", &[]),
        ("/ho~me/someone/x", &["home_path"]),
        ("/ho~me/_x/", &["home_path"]),
        ("/ho~me/1x/", &[]),
        ("/ho~me/Bob/", &[]),
        ("/ho~me/a-b_c9/", &["home_path"]),
        ("/ho~me/ab", &[]),
        ("/ho~me//", &[]),
        ("/ho~me/ab /", &[]),
        ("x/ho~me/ab/", &["home_path"]),
        ("me~@corp.io", &["email"]),
        ("me~@example.com", &[]),
        ("me~@example.org.", &[]),
        ("me~@example.comx.io", &["email"]),
        ("me~@example.invalid", &[]),
        ("me~@corp.i", &[]),
        ("~@corp.io", &[]),
        ("me~@.io", &[]),
        ("me~@a..io", &["email"]),
        ("me~@example.com_x.io", &["email"]),
        ("me~@example.coma", &["email"]),
        ("me~@example.com\u{e9}", &["email"]),
        ("me~@example.com-x", &[]),
        ("me~@example.com.x", &[]),
        ("a+b~@corp.io", &["email"]),
        ("me~@EXAMPLE.com", &["email"]),
        ("a b~@corp.io", &["email"]),
        ("me~@corp.io and x~@example.com", &["email"]),
        ("kyung~rinara", &["site_name"]),
        ("KYUNG~RINARA", &["site_name"]),
        ("Kyung~Rinara", &["site_name"]),
        ("\u{acbd}\u{b9ac}~\u{b098}\u{b77c}", &["site_name"]),
        ("val~_agent", &["project_identifier"]),
        ("VAL~_AGENT", &["project_identifier"]),
        ("val~-agent", &[]),
        ("easy~-vllm", &["project_identifier"]),
        ("easy~_vllm", &["project_identifier"]),
        ("easy~vllm", &[]),
        ("vllm~_simulator", &["project_identifier"]),
        ("VLLM~-SIMULATOR", &["project_identifier"]),
        ("her~mes-subagent", &["project_identifier"]),
        ("Her~mes_Subagent", &["project_identifier"]),
        ("her~mes.subagent", &[]),
        ("nothing here", &[]),
        ("me~@example.com\u{301}", &[]),
        ("10~.1.2.3\u{663}", &["private_ip"]),
    ];

    #[test]
    fn patterns_match_like_the_baseline() {
        for (raw, want) in CASES {
            let line = raw.replace('~', "");
            assert_eq!(line_hits(&line), want.to_vec(), "line {raw:?}");
        }
    }

    #[test]
    fn digits_and_case_folding_are_ascii() {
        // Outside the supported input: the baseline also matched these (Unicode digit,
        // Kelvin sign as k). This port keeps both ASCII; see PORTING.md.
        assert!(line_hits(&"10~.\u{663}.2.3".replace('~', "")).is_empty());
        assert!(line_hits(&"\u{212a}yung~rinara".replace('~', "")).is_empty());
        assert!(line_hits(&"ea\u{17f}y-vl~lm".replace('~', "")).is_empty());
    }
}
