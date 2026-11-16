//! `smoke_install`: install the package into an empty temporary project and run the
//! synthetic campaign end to end.
//!
//! Steps: copy the skill folder to <tmp>/.claude/skills/progress-monitor (and link it from
//! .agents/skills for Codex) with the executables next to this one in its target/release/,
//! `init` dry-run then `--apply` then again (nothing is overwritten), lint the laid
//! template, run the demo campaign from assets/examples through prepare -> change ->
//! gate -> final, lint the instance, and call the Stop hook with and without
//! campaigns/ACTIVE.
//!
//! Prints SMOKE-PASS and exits 0 only when every step held. Needs git.
//! Usage: target/release/smoke_install [--keep]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use progress_monitor::install::{copy_binaries, copy_tree, make_temp_dir, SKIP};
use progress_monitor::jsonio::{compact, parse, pretty};
use progress_monitor::paths::Paths;
use progress_monitor::proc::{capture_input, exit_code, Captured};
use progress_monitor::py::{py_str, strip, truthy};

const GIT: [&str; 6] = [
    "-c",
    "user.name=smoke",
    "-c",
    "user.email=smoke@example.invalid",
    "-c",
    "init.defaultBranch=main",
];

/// Why the smoke stopped early.
enum Stop {
    /// A step did not hold: printed as the FAIL record.
    Fail(String, Value),
    /// Something the smoke did not expect (unreadable output, a timeout).
    Crash(String),
}

type Step<T> = Result<T, Stop>;

struct Done {
    code: i64,
    stdout: String,
    stderr: String,
}

fn tail(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(n)).collect()
}

fn crash(err: impl std::fmt::Display) -> Stop {
    Stop::Crash(err.to_string())
}

fn key<'a>(value: &'a Value, name: &str) -> Step<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| crash(format!("missing key {name:?} in {}", compact(value))))
}

fn index(value: &Value, i: usize) -> Step<&Value> {
    value
        .get(i)
        .ok_or_else(|| crash(format!("missing item {i} in {}", compact(value))))
}

fn items(value: &Value) -> Step<&Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| crash(format!("not a list: {}", compact(value))))
}

fn is(value: &Value, name: &str, text: &str) -> Step<bool> {
    Ok(key(value, name)?.as_str() == Some(text))
}

fn read_text(path: &Path) -> Step<String> {
    fs::read_to_string(path).map_err(|e| crash(format!("{}: {e}", path.display())))
}

fn read_json(path: &Path) -> Step<Value> {
    parse(&read_text(path)?).map_err(|e| crash(format!("{}: {e}", path.display())))
}

fn write_text(path: &Path, text: &str) -> Step<()> {
    fs::write(path, text).map_err(|e| crash(format!("{}: {e}", path.display())))
}

fn copy_dir(src: &Path, dst: &Path) -> Step<()> {
    copy_tree(src, dst, &[]).map_err(crash)
}

struct Smoke {
    root: PathBuf,
    skill: PathBuf,
    steps: Vec<String>,
}

impl Smoke {
    fn fail<T>(&self, step: impl Into<String>, detail: Value) -> Step<T> {
        Err(Stop::Fail(step.into(), detail))
    }

    fn run(
        &self,
        program: &Path,
        args: &[&str],
        cwd: Option<&Path>,
        stdin: Option<&str>,
    ) -> Step<Done> {
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd.unwrap_or(&self.root));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("PM_") {
                command.env_remove(&name);
            }
        }
        command.env("PM_EXECUTOR", "smoke");
        if stdin.is_none() {
            command.stdin(Stdio::null());
        }
        match capture_input(command, stdin, Duration::from_secs(300)) {
            Captured::Done {
                status,
                stdout,
                stderr,
            } => Ok(Done {
                code: exit_code(status),
                stdout,
                stderr,
            }),
            Captured::Timeout => Err(crash(format!(
                "{} timed out after 300 seconds",
                program.display()
            ))),
            Captured::Failed(err) => Err(crash(format!("{}: {err}", program.display()))),
        }
    }

    fn tool(&self, name: &str) -> PathBuf {
        self.skill.join("target").join("release").join(name)
    }

    fn pm_code(&self, args: &[&str], code: i64, stdin: Option<&str>) -> Step<Value> {
        let done = self.run(&self.tool("pm"), args, None, stdin)?;
        let label = format!("pm {}", args.join(" "));
        if done.code != code {
            return self.fail(
                label,
                json!({"exit": done.code, "stdout": tail(&done.stdout, 2000),
                       "stderr": tail(&done.stderr, 2000)}),
            );
        }
        if args.first() == Some(&"stop-hook") {
            return Ok(json!({"stdout": done.stdout}));
        }
        match parse(&done.stdout) {
            Ok(value) => Ok(value),
            Err(_) => self.fail(
                format!("{label} output"),
                Value::from(tail(&done.stdout, 2000)),
            ),
        }
    }

    fn pm(&self, args: &[&str]) -> Step<Value> {
        self.pm_code(args, 0, None)
    }

    fn hook(&self, harness: &str, stdin: &str) -> Step<String> {
        let out = self.pm_code(&["stop-hook", "--harness", harness], 0, Some(stdin))?;
        Ok(key(&out, "stdout")?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    fn lint(&self, args: &[&str]) -> Step<Value> {
        let done = self.run(&self.tool("check_campaign"), args, None, None)?;
        if done.code != 0 {
            return self.fail(
                format!("lint {}", args.join(" ")),
                Value::from(tail(&done.stdout, 3000)),
            );
        }
        parse(&done.stdout).map_err(crash)
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Step<()> {
        let all: Vec<&str> = GIT.iter().copied().chain(args.iter().copied()).collect();
        let done = self.run(Path::new("git"), &all, Some(cwd), None)?;
        if done.code != 0 {
            return self.fail(format!("git {}", args.join(" ")), Value::from(done.stderr));
        }
        Ok(())
    }

    fn ok(&mut self, step: &str) {
        self.steps.push(step.to_string());
    }
}

fn install(s: &mut Smoke, package: &Path, bin_dir: &Path) -> Step<()> {
    fs::create_dir(&s.root).map_err(crash)?;
    copy_tree(package, &s.skill, &SKIP).map_err(crash)?;
    if let Err(err) = copy_binaries(bin_dir, &s.skill) {
        return s.fail("install", Value::from(err));
    }
    let agents = s.root.join(".agents").join("skills");
    fs::create_dir_all(&agents).map_err(crash)?;
    std::os::unix::fs::symlink(
        Path::new("..")
            .join("..")
            .join(".claude")
            .join("skills")
            .join("progress-monitor"),
        agents.join("progress-monitor"),
    )
    .map_err(crash)?;
    s.ok("install");
    Ok(())
}

fn init_steps(s: &mut Smoke) -> Step<()> {
    let dry = s.pm(&["init"])?;
    if !is(&dry, "mode", "dry-run")?
        || !truthy(key(&dry, "created")?)
        || s.root.join("campaigns").exists()
    {
        return s.fail("init dry-run", dry);
    }
    for hook in items(key(&dry, "hooks")?)? {
        if !is(hook, "state", "would-create")? {
            return s.fail("init dry-run hooks", dry["hooks"].clone());
        }
    }
    s.ok("init dry-run writes nothing");

    let applied = s.pm(&["init", "--apply"])?;
    let laid = items(key(&applied, "created")?)?;
    if laid.is_empty() || !laid.iter().all(|p| s.root.join(py_str(p)).is_file()) {
        return s.fail("init --apply files", applied);
    }
    for hook in items(key(&applied, "hooks")?)? {
        let text = read_text(&s.root.join(py_str(key(hook, "file")?)))?;
        let wanted = format!("stop-hook --harness {}", py_str(key(hook, "harness")?));
        if !is(hook, "state", "created")? || !text.contains(&wanted) {
            return s.fail("init --apply hooks", applied["hooks"].clone());
        }
    }
    let settings = read_json(&s.root.join(".claude").join("settings.local.json"))?;
    let stop = index(key(key(&settings, "hooks")?, "Stop")?, 0)?;
    let claude_cmd = key(index(key(stop, "hooks")?, 0)?, "command")?;
    if !py_str(claude_cmd).contains(".claude/skills/progress-monitor/target/release/pm") {
        return s.fail("hook command path", claude_cmd.clone());
    }
    s.ok("init --apply lays campaigns/ and hooks");

    let readme = s.root.join("campaigns").join("README.md");
    write_text(&readme, "local edit\n")?;
    let again = s.pm(&["init", "--apply"])?;
    let mut changed = truthy(key(&again, "created")?);
    for hook in items(key(&again, "hooks")?)? {
        changed |= !is(hook, "state", "present")?;
    }
    if changed {
        return s.fail("init idempotent", again);
    }
    if read_text(&readme)? != "local edit\n" {
        return s.fail("init overwrote a file", Value::from("campaigns/README.md"));
    }
    let source = s.skill.join("assets").join("campaigns").join("README.md");
    fs::copy(&source, &readme).map_err(crash)?;
    s.ok("init keeps existing files");

    let template = s.lint(&[])?;
    if !is(&template, "mode", "template")? || !is(&template, "status", "PASS")? {
        return s.fail("lint template", template);
    }
    s.ok("lint template");
    Ok(())
}

fn campaign_steps(s: &mut Smoke) -> Step<()> {
    let examples = s.skill.join("assets").join("examples");
    let target = s.root.join("demo-target");
    copy_dir(&examples.join("demo-target"), &target)?;
    s.git(&target, &["init", "-q", "."])?;
    s.git(&target, &["add", "-A"])?;
    s.git(&target, &["commit", "-q", "-m", "demo target"])?;
    copy_dir(&examples.join("docs"), &s.root.join("docs"))?;
    let demo = s.root.join("campaigns").join("demo");
    copy_dir(&examples.join("demo-campaign"), &demo)?;
    write_text(&s.root.join("campaigns").join("ACTIVE"), "demo\n")?;
    s.pm(&["open"])?;
    s.ok("open demo");

    let hook_in = compact(&json!({"session_id": "smoke", "stop_hook_active": false,
                                  "background_tasks": []}));
    let guided = s.hook("claude", &hook_in)?;
    if !guided.contains("additionalContext") && !guided.contains("decision") {
        return s.fail("stop-hook with work left", Value::from(guided));
    }
    s.ok("stop-hook keeps the session going");
    s.pm(&["monitor", "reset"])?;

    let edits = [
        ("greet", "greet/message.txt", "hello, campaign\n"),
        ("count", "count/value.txt", "2\n"),
    ];
    for (cell, rel, text) in edits {
        s.pm(&["phase", "start", cell, "prepare"])?;
        s.pm(&["phase", "close", cell, "prepare"])?;
        s.pm(&["phase", "start", cell, "change"])?;
        write_text(&target.join(rel), text)?;
        s.git(&target, &["add", "-A"])?;
        s.git(
            &target,
            &["commit", "-q", "-m", &format!("[{cell}/change] demo edit")],
        )?;
        s.pm(&["phase", "close", cell, "change"])?;
    }
    s.ok("cells prepare -> change");

    let gate = s.pm(&["gate", "run", "g-demo"])?;
    let record = read_json(&demo.join("gates").join("g-demo.status.json"))?;
    let checks = items(key(&record, "checks")?)?;
    let mut results = Vec::new();
    for check in checks {
        results.push(key(check, "result")?.clone());
    }
    let all_pass = !results.is_empty() && results.iter().all(|r| r.as_str() == Some("PASS"));
    if !is(&record, "state", "passed")? || !all_pass {
        return s.fail("gate g-demo", json!({"out": gate, "record": record}));
    }
    s.ok("gate g-demo passed");

    s.pm(&["final", "run"])?;
    let last = read_json(&demo.join("gates").join("_final.status.json"))?;
    if !is(&last, "state", "passed")? {
        return s.fail("final", last);
    }
    s.ok("final passed");

    let instance = s.lint(&["campaigns/demo"])?;
    if !is(&instance, "mode", "instance")? || !is(&instance, "status", "PASS")? {
        return s.fail("lint instance", instance);
    }
    s.ok("lint instance");

    let metrics = s.pm(&["metrics"])?;
    fs::remove_file(s.root.join("campaigns").join("ACTIVE")).map_err(crash)?;
    let silent = s.hook("codex", &hook_in)?;
    if !strip(&silent).is_empty() {
        return s.fail("stop-hook without ACTIVE", Value::from(silent));
    }
    s.ok("stop-hook silent without ACTIVE");

    let totals = metrics.get("totals").cloned().unwrap_or(metrics);
    println!(
        "{}",
        pretty(&json!({"status": "PASS", "passed": s.steps, "metrics": totals}))
    );
    println!("SMOKE-PASS");
    Ok(())
}

fn smoke(tmp: &Path, package: &Path, bin_dir: &Path) -> Result<(), (Stop, Vec<String>)> {
    let root = tmp.join("project");
    let mut s = Smoke {
        skill: root.join(".claude").join("skills").join("progress-monitor"),
        root,
        steps: Vec::new(),
    };
    install(&mut s, package, bin_dir)
        .and_then(|_| init_steps(&mut s))
        .and_then(|_| campaign_steps(&mut s))
        .map_err(|stop| (stop, s.steps.clone()))
}

fn main() {
    let keep = std::env::args().skip(1).any(|a| a == "--keep");
    let paths = match Paths::discover() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    let bin_dir = paths.exe.parent().unwrap_or(Path::new("/")).to_path_buf();
    let tmp = match make_temp_dir("pm-smoke-") {
        Ok(t) => t,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    let result = smoke(&tmp, &paths.skill_dir, &bin_dir);
    if keep {
        eprintln!("kept: {}", tmp.display());
    } else {
        let _ = fs::remove_dir_all(&tmp);
    }
    let code = match result {
        Ok(()) => 0,
        Err((Stop::Fail(step, detail), passed)) => {
            println!(
                "{}",
                pretty(
                    &json!({"status": "FAIL", "step": step, "detail": detail, "passed": passed})
                )
            );
            1
        }
        Err((Stop::Crash(err), _)) => {
            eprintln!("{err}");
            1
        }
    };
    std::process::exit(code);
}
