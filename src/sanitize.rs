//! Distribution hygiene scan: site names, personal home paths, private IPs, e-mail
//! addresses and the identifiers of the projects this package was extracted from.
//!
//! The five patterns are the baseline pattern strings compiled with `fancy-regex`
//! (Unicode `\d`, simple case folding). Two spots are adapted so the matches stay the
//! baseline's: see `baseline_compat`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fancy_regex::Regex;

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

/// The baseline pattern strings, in report order, and whether each one ignores case.
/// Site and project names are written in pieces so this file does not match itself.
fn baseline_patterns() -> [(String, bool); 5] {
    let site_names = [
        "\u{acbd}\u{b9ac}\u{b098}\u{b77c}".to_string(),
        ["kyung", "rinara"].concat(),
    ];
    let project_names = [
        ["val", "_agent"].concat(),
        ["easy", "[_-]vllm"].concat(),
        ["vllm", "[_-]simulator"].concat(),
        ["her", "mes[_-]subagent"].concat(),
    ];
    [
        (site_names.join("|"), true),
        (r"/home/[a-z_][a-z0-9_-]*/".to_string(), false),
        (
            r"(?<![0-9.])(?:10\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])|192\.168)\.\d{1,3}\.\d{1,3}(?![0-9])"
                .to_string(),
            false,
        ),
        (
            r"[A-Za-z0-9._%+-]+@(?!example\.(?:invalid|com|org)\b)[A-Za-z0-9.-]+\.[A-Za-z]{2,}"
                .to_string(),
            false,
        ),
        (project_names.join("|"), true),
    ]
}

/// The baseline pattern rewritten where the Rust engine reads it differently.
///
/// - `\b`: the baseline word characters are `str.isalnum()` or `_` (letters and every
///   numeric character, not combining marks); the engine's `\w` adds marks and leaves
///   out `No`/`Nl` numbers. Every `\b` here follows a letter, so it is "no word
///   character next", written with the baseline's word set.
/// - Ignoring case, the baseline also matches `i` with U+0130 and U+0131, which simple
///   case folding does not; `i` only occurs in the literal names.
fn baseline_compat(pattern: &str, ignore_case: bool) -> String {
    let pattern = pattern.replace(r"\b", r"(?![\p{L}\p{N}_])");
    if ignore_case {
        format!("(?i){}", pattern.replace('i', "[i\u{130}\u{131}]"))
    } else {
        pattern
    }
}

fn compiled() -> &'static [Regex; 5] {
    static PATTERNS: OnceLock<[Regex; 5]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        baseline_patterns().map(|(pattern, ignore_case)| {
            Regex::new(&baseline_compat(&pattern, ignore_case))
                .expect("the sanitize patterns are valid")
        })
    })
}

/// One hit: path relative to the root (with `/`), 1-based line number, pattern name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rel: String,
    pub line: usize,
    pub name: &'static str,
}

/// The names of the patterns that occur in `line`, in report order.
pub fn line_hits(line: &str) -> Vec<&'static str> {
    NAMES
        .iter()
        .zip(compiled())
        // The patterns have no backreferences or deep nesting; a backtrack-limit error
        // cannot occur, and counting it as no hit would hide a finding, so it panics.
        .filter(|(_, re)| re.is_match(line).expect("sanitize pattern ran"))
        .map(|(name, _)| *name)
        .collect()
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
    fn unicode_digits_folding_and_word_boundary_follow_the_baseline() {
        // Expected names taken from the baseline patterns on the same lines.
        let cases: [(&str, &[&str]); 12] = [
            ("10~.\u{663}.2.3", &["private_ip"]),
            ("10~.1.2.\u{1d7d1}", &["private_ip"]),
            ("10~.1.2.\u{b2}", &[]),
            ("\u{212a}yung~rinara", &["site_name"]),
            ("kyungr\u{131}~nara", &["site_name"]),
            ("KYUNGR\u{130}~NARA", &["site_name"]),
            ("ea\u{17f}y-vl~lm", &["project_identifier"]),
            ("vllm_s\u{130}mul~ator", &["project_identifier"]),
            ("me~@example.com\u{b2}", &["email"]),
            ("me~@example.org\u{2160}.io", &["email"]),
            ("me~@example.com\u{903}", &[]),
            ("me~@example.invalid\u{5d0}", &["email"]),
        ];
        for (raw, want) in cases {
            let line = raw.replace('~', "");
            assert_eq!(line_hits(&line), want.to_vec(), "line {raw:?}");
        }
    }
}
