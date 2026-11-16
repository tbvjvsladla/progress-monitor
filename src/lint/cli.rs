//! `check_campaign` command line: arguments, the negative control, the report.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::control::negative_control;
use super::{check, findings_value, is_dir, os_path, py_text, LintError, RULES};
use crate::error::PmError;
use crate::jsonio::{encode, pretty};
use crate::paths::{normalise, resolve, Paths};
use crate::py::obj;

pub const HELP: &str = "\
usage: check_campaign [-h] [--template] [--root ROOT] [--list-control] [path]

Check a campaign instance or the campaign template (see campaigns/README.md).

The check is read-only. It reads one point in time and never runs a declared
proof.check command.

positional arguments:
  path            campaign folder (default: campaigns/_template)

options:
  -h, --help      show this help message and exit
  --template      check the target as a template
  --root ROOT     repository root for relative pointer paths
  --list-control  print the negative control outcome per rule

Rules (name, what fails):
  proof-blank              proof.check or proof.expect is empty
  source-missing           state is done/failed but proof.source is empty
  state-invalid            state is not pending/running/done/failed
  done-not-ok              state is done but proof.ok is not true
  undeclared               a phase file or cell folder the declaration does not name
  cell-summary-incomplete  cell.md lacks one of its six sections, or a section is empty
  journey-invalid          a journey line is not a JSON object, lacks utc/event,
                           or its time goes backward
  pointer-invalid          an evidence pointer has a key outside kind/path/cell/note,
                           lacks kind/path, or its path does not exist
  declared-missing         a declared cell lacks its folder, cell.md or a phase file
  placeholder-left         (instance only) a placeholder or the skeleton cell remains
  no-cells                 (instance only) the declaration has no cells
  done-unrecorded          (instance only) a phase is done but the executor record
                           (by) is absent or its log file is missing
  closed-without-gate      (instance only) a cell is closed but its group's gate is
                           not passed or does not list the cell as a member
  structure-unlogged       (instance only) replaying campaign-open, cell-add,
                           cell-cancel and regroup events does not give the present
                           cell/group layout (skipped when campaign-open is absent)
  auto-cap-exceeded        (instance only) more auto cells than budgets.auto_cells
                           plus the allowed auto-cell-cap answers
  check-duplicate          (instance only) two checks share an id, or share
                           cmd, cwd and env
  ref-invalid              (instance only) a group, check, covers, parent or needs
                           reference names something the declaration lacks
  gate-not-proven          (instance only) a gate is passed but a check result is
                           not PASS/reused/covered or its receipt file is missing

The declaration must be schema_version 2; anything else is unreadable (exit 2).

Template mode applies when the target is campaigns/_template or --template is
given; every other target is checked as an instance. The mode never depends on
file contents, so a copied template is checked as an instance.

Before checking the target the program runs a negative control: it builds a valid
instance from the real template in a temporary directory, requires it to pass,
and requires each single-defect copy to fail with exactly its own rule.

Exit codes: 0 pass, 1 rule failure (or failed control), 2 usage or unreadable input.
Not checked: edits to existing journey lines, whether a proof.check can really
fail, whether a receipt is truthful, and whether review rounds stayed within the
cap. The executor (the pm program beside this one) writes these files; this
check only reads them.
";

/// The parsed command line.
#[derive(Debug, Default, PartialEq)]
struct Args {
    path: Option<String>,
    template: bool,
    root: Option<String>,
    list_control: bool,
}

enum ParseEnd {
    Help,
    Error(String),
}

fn is_negative_number(arg: &str) -> bool {
    let body = &arg[1..];
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    digits(body)
        || body
            .split_once('.')
            .is_some_and(|(a, b)| a.chars().all(|c| c.is_ascii_digit()) && digits(b))
}

/// Whether an argument is read as an option rather than a value.
fn looks_optional(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1 && !is_negative_number(arg) && !arg.contains(' ')
}

fn parse(argv: &[String]) -> Result<Args, ParseEnd> {
    let mut args = Args::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    let mut only_positional = false;
    while i < argv.len() {
        let arg = &argv[i];
        if only_positional || !looks_optional(arg) {
            positional.push(arg.clone());
        } else if arg == "--" {
            only_positional = true;
        } else {
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v)),
                _ => (arg.as_str(), None),
            };
            match (name, inline) {
                ("-h" | "--help", None) => return Err(ParseEnd::Help),
                ("--template", None) => args.template = true,
                ("--list-control", None) => args.list_control = true,
                ("--root", Some(value)) => args.root = Some(value.to_string()),
                ("--root", None) => match argv.get(i + 1) {
                    Some(next) if !looks_optional(next) => {
                        args.root = Some(next.clone());
                        i += 1;
                    }
                    _ => {
                        return Err(ParseEnd::Error(
                            "argument --root: expected one argument".into(),
                        ))
                    }
                },
                (_, Some(_)) if ["--template", "--list-control", "--help"].contains(&name) => {
                    return Err(ParseEnd::Error(format!(
                        "argument {name}: ignored explicit argument '{}'",
                        inline.unwrap_or_default()
                    )));
                }
                _ => return Err(ParseEnd::Error(format!("unrecognized arguments: {arg}"))),
            }
        }
        i += 1;
    }
    if positional.len() > 1 {
        return Err(ParseEnd::Error(format!(
            "unrecognized arguments: {}",
            positional[1..].join(" ")
        )));
    }
    args.path = positional.pop();
    Ok(args)
}

/// Print the report; a text holding a name that is not UTF-8 cannot be written,
/// which ends the command as the baseline's `UnicodeEncodeError` did (exit 1).
fn print(value: Value) -> Result<(), LintError> {
    let text = match encode(&pretty(&value)) {
        Ok(text) => text,
        Err(PmError::Internal(why) | PmError::Usage(why)) => return Err(LintError::Internal(why)),
        Err(other) => return Err(LintError::Internal(format!("{other:?}"))),
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
    Ok(())
}

fn internal(text: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "check_campaign: error: {text}");
    1
}

/// The project's `campaigns/_template` when init laid it, otherwise the package copy.
fn default_template(paths: &Paths, package: &Path) -> Result<PathBuf, String> {
    let installed = paths.repo.join("campaigns").join("_template");
    resolve(if is_dir(&installed) {
        &installed
    } else {
        package
    })
}

/// Run the check with these arguments (without the program name). Returns the exit code.
pub fn main(argv: &[String]) -> i32 {
    let args = match parse(argv) {
        Ok(args) => args,
        Err(ParseEnd::Help) => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(HELP.as_bytes());
            return 0;
        }
        Err(ParseEnd::Error(text)) => {
            let _ = writeln!(
                std::io::stderr(),
                "check_campaign: error: {text}\n(see check_campaign --help)"
            );
            return 2;
        }
    };
    let paths = match Paths::discover() {
        Ok(paths) => paths,
        Err(text) => return internal(&text),
    };
    let package = paths
        .skill_dir
        .join("assets")
        .join("campaigns")
        .join("_template");
    let setup = || -> Result<(PathBuf, PathBuf, PathBuf, bool), String> {
        let root = match &args.root {
            Some(text) => resolve(&os_path(&normalise(text)))?,
            None => paths.repo.clone(),
        };
        let template_dir = default_template(&paths, &package)?;
        let target = match &args.path {
            Some(text) => resolve(&os_path(&normalise(text)))?,
            None => template_dir.clone(),
        };
        let template = args.template || target == template_dir || target == resolve(&package)?;
        Ok((root, template_dir, target, template))
    };
    let (root, template_dir, target, template) = match setup() {
        Ok(found) => found,
        Err(text) => return internal(&text),
    };
    match run(&args, &root, &template_dir, &target, template) {
        Ok(code) => code,
        Err(LintError::Unreadable(text)) => {
            match print(Value::Object(obj([
                ("status", Value::from("ERROR")),
                ("error", Value::from(text)),
            ]))) {
                Ok(()) => 2,
                Err(LintError::Internal(text) | LintError::Unreadable(text)) => internal(&text),
            }
        }
        Err(LintError::Internal(text)) => internal(&text),
    }
}

fn run(
    args: &Args,
    root: &Path,
    template_dir: &Path,
    target: &Path,
    template: bool,
) -> Result<i32, LintError> {
    let (report, problems) = negative_control(template_dir)?;
    if args.list_control {
        let mut per_rule = serde_json::Map::new();
        let mut all_detected = true;
        for rule in RULES {
            let outcomes: Vec<&Value> = report
                .cases
                .values()
                .filter(|case| case.get("expected").and_then(Value::as_str) == Some(rule))
                .filter_map(|case| case.get("outcome"))
                .collect();
            let detected =
                !outcomes.is_empty() && outcomes.iter().all(|o| o.as_str() == Some("detected"));
            all_detected &= detected;
            per_rule.insert(
                rule.to_string(),
                Value::from(if detected { "detected" } else { "missed" }),
            );
        }
        let ok = problems.is_empty() && all_detected;
        print(Value::Object(obj([
            ("status", Value::from(if ok { "PASS" } else { "FAIL" })),
            (
                "valid_instance_states",
                Value::Array(report.valid_instance_states),
            ),
            ("valid_instance", Value::Object(report.valid_instance)),
            ("rules", Value::Object(per_rule)),
            ("cases", Value::Object(report.cases)),
            ("problems", Value::from(problems)),
        ])))?;
        return Ok(if ok { 0 } else { 1 });
    }
    if !problems.is_empty() {
        print(Value::Object(obj([
            ("status", Value::from("FAIL")),
            ("negative_control", Value::from(problems)),
        ])))?;
        return Ok(1);
    }
    let findings = check(target, root, template)?;
    let shown = match target.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => py_text(rel.as_os_str()),
        Err(_) => py_text(target.as_os_str()),
    };
    print(Value::Object(obj([
        (
            "status",
            Value::from(if findings.is_empty() { "PASS" } else { "FAIL" }),
        ),
        ("negative_control", Value::from("PASS")),
        (
            "mode",
            Value::from(if template { "template" } else { "instance" }),
        ),
        ("target", Value::from(shown)),
        ("findings", findings_value(&findings)),
    ])))?;
    Ok(if findings.is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_and_path() {
        let args = parse(&argv(&["x", "--root=/r", "--template"]))
            .ok()
            .unwrap();
        assert_eq!(args.path.as_deref(), Some("x"));
        assert_eq!(args.root.as_deref(), Some("/r"));
        assert!(args.template && !args.list_control);
        assert!(matches!(parse(&argv(&["a", "b"])), Err(ParseEnd::Error(_))));
        assert!(matches!(parse(&argv(&["--root"])), Err(ParseEnd::Error(_))));
        assert!(matches!(parse(&argv(&["--temp"])), Err(ParseEnd::Error(_))));
        assert!(matches!(parse(&argv(&["--help"])), Err(ParseEnd::Help)));
        let args = parse(&argv(&["--", "--template"])).ok().unwrap();
        assert_eq!(args.path.as_deref(), Some("--template"));
    }
}
