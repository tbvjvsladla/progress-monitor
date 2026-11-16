//! Command line: a small parser with the baseline's grammar (global options before the
//! command, `--opt value` and `--opt=value`, the last repeat wins, `--` ends options,
//! no abbreviations), and the entry point that prints one JSON result.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

use crate::campaign::Campaign;
use crate::commands as c;
use crate::error::PmError;
use crate::jsonio::pretty;
use crate::paths::{normalise, resolve, Paths};
use crate::py::Obj;

type Handler = fn(&mut Campaign, &Args) -> c::Out;

#[derive(Clone, Copy)]
enum Kind {
    Value,
    Flag,
}

#[derive(Clone, Copy)]
struct Opt {
    name: &'static str,
    kind: Kind,
    required: bool,
    choices: Option<&'static [&'static str]>,
    float: bool,
}

const fn value(name: &'static str) -> Opt {
    Opt {
        name,
        kind: Kind::Value,
        required: false,
        choices: None,
        float: false,
    }
}

const fn required(name: &'static str) -> Opt {
    Opt {
        name,
        kind: Kind::Value,
        required: true,
        choices: None,
        float: false,
    }
}

const fn flag(name: &'static str) -> Opt {
    Opt {
        name,
        kind: Kind::Flag,
        required: false,
        choices: None,
        float: false,
    }
}

const fn choice(name: &'static str, choices: &'static [&'static str], required: bool) -> Opt {
    Opt {
        name,
        kind: Kind::Value,
        required,
        choices: Some(choices),
        float: false,
    }
}

const fn float(name: &'static str) -> Opt {
    Opt {
        name,
        kind: Kind::Value,
        required: false,
        choices: None,
        float: true,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Special {
    None,
    StopHook,
    Init,
}

struct Leaf {
    path: &'static [&'static str],
    positionals: &'static [&'static str],
    options: &'static [Opt],
    handler: Option<Handler>,
    special: Special,
}

const fn leaf(
    path: &'static [&'static str],
    positionals: &'static [&'static str],
    options: &'static [Opt],
    handler: Handler,
) -> Leaf {
    Leaf {
        path,
        positionals,
        options,
        handler: Some(handler),
        special: Special::None,
    }
}

const BRIEFING: &[&str] = &["true", "false"];
const HOOK_HARNESS: &[&str] = &["claude", "codex"];
const INIT_HARNESS: &[&str] = &["claude", "codex", "both"];

static LEAVES: &[Leaf] = &[
    leaf(
        &["open"],
        &[],
        &[
            choice("--briefing-on-phase", BRIEFING, false),
            float("--briefing-every-hours"),
        ],
        c::cmd_open,
    ),
    leaf(&["status"], &[], &[], crate::status_cmd),
    leaf(&["metrics"], &[], &[], crate::metrics_cmd),
    leaf(&["brief"], &[], &[value("--done")], c::cmd_brief),
    leaf(&["event"], &["json"], &[], c::cmd_event),
    leaf(
        &["phase", "start"],
        &["cell", "phase"],
        &[],
        c::cmd_phase_start,
    ),
    leaf(
        &["phase", "close"],
        &["cell", "phase"],
        &[],
        c::cmd_phase_close,
    ),
    leaf(
        &["phase", "retry"],
        &["cell", "phase"],
        &[],
        c::cmd_phase_retry,
    ),
    leaf(
        &["gate", "run"],
        &["group"],
        &[value("--force"), value("--reason")],
        c::cmd_gate_run,
    ),
    leaf(
        &["gate", "reopen"],
        &["group"],
        &[required("--reason")],
        c::cmd_gate_reopen,
    ),
    leaf(
        &["final", "run"],
        &[],
        &[value("--force"), value("--reason")],
        c::cmd_final_run,
    ),
    leaf(
        &["check", "invalidate"],
        &["check"],
        &[required("--reason")],
        c::cmd_check_invalidate,
    ),
    leaf(
        &["check", "run"],
        &["check"],
        &[required("--explore"), required("--target")],
        c::cmd_check_run,
    ),
    leaf(
        &["explore", "open"],
        &[],
        &[required("--cell"), required("--reason")],
        c::cmd_explore_open,
    ),
    leaf(
        &["explore", "close"],
        &["id"],
        &[required("--selected"), required("--ref"), value("--note")],
        c::cmd_explore_close,
    ),
    leaf(&["cell", "add"], &["spec"], &[], c::cmd_cell_add),
    leaf(
        &["cell", "cancel"],
        &["cell"],
        &[required("--reason")],
        c::cmd_cell_cancel,
    ),
    leaf(
        &["cell", "move"],
        &["cell", "group"],
        &[required("--reason"), value("--checks")],
        c::cmd_cell_move,
    ),
    leaf(
        &["cell", "reopen"],
        &["cell"],
        &[required("--reason")],
        c::cmd_cell_reopen,
    ),
    leaf(
        &["cell", "stop"],
        &["cell"],
        &[required("--reason")],
        c::cmd_cell_stop,
    ),
    leaf(
        &["cell", "resume"],
        &["cell"],
        &[value("--ref")],
        c::cmd_cell_resume,
    ),
    leaf(&["cell", "close"], &["cell"], &[], c::cmd_cell_close),
    leaf(
        &["review", "record"],
        &["cell", "review"],
        &[],
        c::cmd_review_record,
    ),
    leaf(
        &["hitl", "open"],
        &[],
        &[
            required("--kind"),
            required("--question"),
            value("--cell"),
            value("--group"),
            flag("--final"),
        ],
        c::cmd_hitl_open,
    ),
    leaf(
        &["hitl", "answer"],
        &["id"],
        &[value("--ref"), flag("--allow")],
        c::cmd_hitl_answer,
    ),
    leaf(&["monitor", "reset"], &[], &[], c::cmd_monitor_reset),
    Leaf {
        path: &["stop-hook"],
        positionals: &[],
        options: &[choice("--harness", HOOK_HARNESS, true)],
        handler: None,
        special: Special::StopHook,
    },
    Leaf {
        path: &["init"],
        positionals: &[],
        options: &[choice("--harness", INIT_HARNESS, false), flag("--apply")],
        handler: None,
        special: Special::Init,
    },
];

const USAGE: &str = "\
progress-monitor executor for a two-layer campaign.

Usage: pm [--campaigns <dir>] [--executor <who>] <command> ...
       pm init [--harness claude|codex|both] [--apply]   lay campaigns/ and the Stop hook

Commands:
  open [--briefing-on-phase true|false] [--briefing-every-hours H]
  status | metrics | brief [--done <path>] | event <json>
  phase start|close|retry <cell> <phase>
  gate run <group> [--force <check> --reason <why>] | gate reopen <group> --reason <why>
  final run [--force <check> --reason <why>]
  check invalidate <check> --reason <why> | check run <check> --explore <id> --target <dir>
  explore open --cell <cell> --reason <why> | explore close <id> --selected <x> --ref <r> [--note <n>]
  cell add <spec.json> | cell cancel|reopen|stop <cell> --reason <why>
  cell move <cell> <group> --reason <why> [--checks a,b] | cell resume <cell> --ref <r> | cell close <cell>
  review record <cell> <review.json>
  hitl open --kind <k> --question <q> (--cell <c> | --group <g> | --final)
  hitl answer <id> --ref <r> [--allow]
  monitor reset
  stop-hook --harness claude|codex
  init [--harness claude|codex|both] [--apply]

Exit codes: 0 ok, 1 a check failed or the request was refused, 2 usage or
unreadable input. `stop-hook` always exits 0.
";

/// Parsed arguments of one leaf command.
#[derive(Default, Debug)]
pub struct Args {
    positionals: Vec<String>,
    options: HashMap<String, String>,
    flags: HashSet<String>,
}

impl Args {
    pub fn pos(&self, i: usize) -> &str {
        self.positionals.get(i).map_or("", String::as_str)
    }

    pub fn opt(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

enum ParseEnd {
    Help,
    Error(String),
}

struct Parsed {
    campaigns: Option<String>,
    executor: Option<String>,
    leaf: &'static Leaf,
    args: Args,
}

fn is_negative_number(arg: &str) -> bool {
    let body = &arg[1..];
    if body.is_empty() {
        return false;
    }
    if body.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }
    match body.split_once('.') {
        Some((a, b)) => {
            a.chars().all(|ch| ch.is_ascii_digit())
                && !b.is_empty()
                && b.chars().all(|ch| ch.is_ascii_digit())
        }
        None => false,
    }
}

/// Whether an argument is read as an option (else it is a value).
fn looks_optional(arg: &str, known: &[&str]) -> bool {
    if !arg.starts_with('-') || arg.len() == 1 {
        return false;
    }
    let name = arg.split_once('=').map_or(arg, |(n, _)| n);
    if known.contains(&arg) || (arg.starts_with("--") && known.contains(&name)) {
        return true;
    }
    if is_negative_number(arg) || arg.contains(' ') {
        return false;
    }
    true
}

fn take_value(
    args: &[String],
    i: &mut usize,
    name: &str,
    inline: Option<&str>,
    known: &[&str],
) -> Result<String, ParseEnd> {
    if let Some(v) = inline {
        return Ok(v.to_string());
    }
    match args.get(*i + 1) {
        Some(next) if !looks_optional(next, known) => {
            *i += 1;
            Ok(next.clone())
        }
        _ => Err(ParseEnd::Error(format!(
            "argument {name}: expected one argument"
        ))),
    }
}

fn parse(argv: &[String]) -> Result<Parsed, ParseEnd> {
    let global = ["--campaigns", "--executor", "-h", "--help"];
    let (mut campaigns, mut executor) = (None, None);
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        if !looks_optional(arg, &global) {
            break;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if arg.starts_with("--") => (n, Some(v)),
            _ => (arg.as_str(), None),
        };
        match name {
            "-h" | "--help" => return Err(ParseEnd::Help),
            "--campaigns" => campaigns = Some(take_value(argv, &mut i, name, inline, &global)?),
            "--executor" => executor = Some(take_value(argv, &mut i, name, inline, &global)?),
            _ => return Err(ParseEnd::Error(format!("unrecognized arguments: {arg}"))),
        }
        i += 1;
    }
    let Some(command) = argv.get(i) else {
        return Err(ParseEnd::Error(
            "the following arguments are required: command".into(),
        ));
    };
    let candidates: Vec<&Leaf> = LEAVES.iter().filter(|l| l.path[0] == command).collect();
    if candidates.is_empty() {
        return Err(ParseEnd::Error(format!("invalid choice: {command}")));
    }
    i += 1;
    let leaf: &'static Leaf = if candidates[0].path.len() == 1 {
        candidates[0]
    } else {
        match argv.get(i).map(String::as_str) {
            Some("-h" | "--help") => return Err(ParseEnd::Help),
            Some(action) if !looks_optional(action, &[]) => {
                i += 1;
                LEAVES
                    .iter()
                    .find(|l| l.path.len() == 2 && l.path[0] == command && l.path[1] == action)
                    .ok_or_else(|| ParseEnd::Error(format!("invalid choice: {action}")))?
            }
            Some(other) => return Err(ParseEnd::Error(format!("unrecognized arguments: {other}"))),
            None => {
                return Err(ParseEnd::Error(
                    "the following arguments are required: action".into(),
                ))
            }
        }
    };
    let rest = &argv[i..];
    let mut known: Vec<&str> = leaf.options.iter().map(|o| o.name).collect();
    known.extend(["-h", "--help"]);
    let mut args = Args::default();
    let mut only_positional = false;
    let mut extra = Vec::new();
    let mut j = 0;
    while j < rest.len() {
        let arg = &rest[j];
        if only_positional || !looks_optional(arg, &known) {
            if leaf.positionals.len() > args.positionals.len() {
                args.positionals.push(arg.clone());
            } else {
                extra.push(arg.clone());
            }
            j += 1;
            continue;
        }
        if arg == "--" {
            only_positional = true;
            j += 1;
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if arg.starts_with("--") => (n, Some(v)),
            _ => (arg.as_str(), None),
        };
        if name == "-h" || name == "--help" {
            return Err(ParseEnd::Help);
        }
        let Some(opt) = leaf.options.iter().find(|o| o.name == name) else {
            extra.push(arg.clone());
            j += 1;
            continue;
        };
        match opt.kind {
            Kind::Flag => {
                if inline.is_some() {
                    return Err(ParseEnd::Error(format!(
                        "argument {name}: ignored explicit argument"
                    )));
                }
                args.flags.insert(name.to_string());
            }
            Kind::Value => {
                let value = take_value(rest, &mut j, name, inline, &known)?;
                if let Some(choices) = opt.choices {
                    if !choices.contains(&value.as_str()) {
                        return Err(ParseEnd::Error(format!(
                            "argument {name}: invalid choice: {value}"
                        )));
                    }
                }
                if opt.float && c::parse_float(&value).is_err() {
                    return Err(ParseEnd::Error(format!(
                        "argument {name}: invalid float value: {value}"
                    )));
                }
                args.options.insert(name.to_string(), value);
            }
        }
        j += 1;
    }
    if args.positionals.len() < leaf.positionals.len() {
        let missing = leaf.positionals[args.positionals.len()..].join(", ");
        return Err(ParseEnd::Error(format!(
            "the following arguments are required: {missing}"
        )));
    }
    let missing: Vec<&str> = leaf
        .options
        .iter()
        .filter(|o| o.required && !args.options.contains_key(o.name))
        .map(|o| o.name)
        .collect();
    if !missing.is_empty() {
        return Err(ParseEnd::Error(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        )));
    }
    if !extra.is_empty() {
        return Err(ParseEnd::Error(format!(
            "unrecognized arguments: {}",
            extra.join(" ")
        )));
    }
    if leaf.path == ["hitl", "open"] {
        let given: Vec<&str> = ["--cell", "--group"]
            .into_iter()
            .filter(|n| args.options.contains_key(*n))
            .chain(args.flags.contains("--final").then_some("--final"))
            .collect();
        match given.len() {
            0 => {
                return Err(ParseEnd::Error(
                    "one of the arguments --cell --group --final is required".into(),
                ))
            }
            1 => {}
            _ => {
                return Err(ParseEnd::Error(format!(
                    "argument {}: not allowed with argument {}",
                    given[1], given[0]
                )))
            }
        }
    }
    Ok(Parsed {
        campaigns,
        executor,
        leaf,
        args,
    })
}

/// Print one result as indented JSON.
fn print_json(result: &Obj) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", pretty(&Value::Object(result.clone())));
    let _ = out.flush();
}

fn failure(text: String) -> Obj {
    crate::py::obj([("ok", Value::Bool(false)), ("error", Value::from(text))])
}

/// Run the executor with these arguments (without the program name). Returns the exit code.
pub fn main(argv: &[String]) -> i32 {
    let hook_named = argv.iter().any(|a| a == "stop-hook");
    let parsed = match parse(argv) {
        Ok(p) => p,
        Err(ParseEnd::Help) => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(USAGE.as_bytes());
            return 0;
        }
        Err(ParseEnd::Error(text)) => {
            let _ = writeln!(std::io::stderr(), "pm: error: {text}\n(see pm --help)");
            return if hook_named { 0 } else { 2 };
        }
    };
    let is_hook = parsed.leaf.special == Special::StopHook;
    let executor = parsed
        .executor
        .clone()
        .filter(|e| !e.is_empty())
        .or_else(|| std::env::var("PM_EXECUTOR").ok().filter(|e| !e.is_empty()))
        .unwrap_or_else(|| "unknown".to_string());
    let paths = match Paths::discover() {
        Ok(p) => p,
        Err(text) => {
            if !is_hook {
                print_json(&failure(format!("RuntimeError: {text}")));
            }
            return if is_hook { 0 } else { 2 };
        }
    };
    let base_path: PathBuf = match &parsed.campaigns {
        Some(text) => normalise(text),
        None => match std::env::var("PM_CAMPAIGNS_DIR")
            .ok()
            .filter(|e| !e.is_empty())
        {
            Some(env) => normalise(&env),
            None => paths.repo.join("campaigns"),
        },
    };
    let base = match resolve(&base_path) {
        Ok(b) => b,
        Err(text) => {
            if !is_hook {
                print_json(&failure(format!("campaigns directory: {text}")));
            }
            return if is_hook { 0 } else { 2 };
        }
    };
    match parsed.leaf.special {
        Special::StopHook => {
            crate::hook::stop_hook(
                &base,
                parsed.args.opt("--harness").unwrap_or("claude"),
                &executor,
                &paths,
            );
            return 0;
        }
        Special::Init => {
            let harness = parsed.args.opt("--harness").unwrap_or("both");
            let (result, code) =
                match crate::init::cmd_init(&base, &paths, harness, parsed.args.flag("--apply")) {
                    Ok(r) => (r, 0),
                    Err(e) => (error_payload(e).0, 2),
                };
            let shown: Obj = result.into_iter().filter(|(_, v)| !v.is_null()).collect();
            print_json(&shown);
            return code;
        }
        Special::None => {}
    }
    let handler = parsed.leaf.handler.expect("every plain leaf has a handler");
    let outcome = Campaign::open(&base, &executor, &paths)
        .and_then(|mut camp| handler(&mut camp, &parsed.args));
    let (result, code) = match outcome {
        Ok(r) => (r, 0),
        Err(e) => error_payload(e),
    };
    let shown: Obj = result
        .into_iter()
        .filter(|(k, v)| !v.is_null() || k == "hitl" || k == "note")
        .collect();
    print_json(&shown);
    code
}

fn error_payload(err: PmError) -> (Obj, i32) {
    match err {
        PmError::Refused(payload) => (payload, 1),
        PmError::Usage(text) => (failure(text), 2),
        PmError::Internal(text) => (failure(text), 2),
    }
}
