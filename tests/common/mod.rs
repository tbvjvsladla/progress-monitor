//! Shared fixture for the executor regression tests.
//!
//! Each test builds a small Git target and a campaign instance in a temporary
//! directory, then calls the `pm` binary as a child process. "Rule N" is the order
//! in references/gate.md ("검사 하나를 돌리는 규칙").
//!
//! Each test crate uses a different part of this module, so unused helpers are
//! expected in any one crate.
#![allow(dead_code)]

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

pub const SECTIONS: [&str; 6] = [
    "목표",
    "확인된 사실",
    "적용한 변경",
    "검증",
    "막다른 길",
    "남은 계획",
];

pub const PM: &str = env!("CARGO_BIN_EXE_pm");
pub const LINT: &str = env!("CARGO_BIN_EXE_check_campaign");

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A path as a string argument.
pub fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A command with every `PM_*` variable removed from the inherited environment.
pub fn clean_command(program: &str) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("PM_") {
            command.env_remove(&key);
        }
    }
    command
}

/// A fresh empty directory under the system temporary directory.
fn make_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("pm-test-{}-{n}-{nanos}", std::process::id()));
    fs::create_dir(&path).expect("create the temporary directory");
    path
}

/// The current UTC time minus `hours`, in ISO 8601 with microseconds and `+00:00`.
pub fn utc_iso_hours_ago(hours: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970");
    let micros_total = now.as_micros() as i128 - (hours as i128) * 3_600_000_000;
    let secs = micros_total.div_euclid(1_000_000) as i64;
    let micros = micros_total.rem_euclid(1_000_000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{micros:06}+00:00",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn check(ident: &str, cmd: &str, expect: &str, inputs: &[&str], covers: &[&str]) -> Value {
    json!({"id": ident, "cmd": cmd, "cwd": ".", "expect": expect, "inputs": inputs, "env": [],
           "covers": covers})
}

fn cell_entry(name: &str, group: &str) -> Value {
    json!({"cell": name, "goal": format!("work on {name}"), "group": group, "needs": [],
           "review": "none", "origin": "plan", "parent": null, "state": "open"})
}

/// Merge `fields` into `target` like `dict.update`: existing keys keep their place.
fn update(target: &mut Value, fields: Value) {
    let obj = target.as_object_mut().expect("an object to update");
    for (key, value) in fields.as_object().expect("an object of fields").clone() {
        obj.insert(key, value);
    }
}

fn find_mut<'a>(list: &'a mut Value, key: &str, name: &str) -> &'a mut Value {
    list.as_array_mut()
        .expect("a list")
        .iter_mut()
        .find(|item| item[key] == name)
        .unwrap_or_else(|| panic!("no entry with {key} = {name}"))
}

/// A Git target with alpha/ and beta/, and an instance with groups g-a (a1, a2) and g-b (b1).
pub struct Sandbox {
    pub tmp: PathBuf,
    pub target: PathBuf,
    pub base: PathBuf,
    pub inst: PathBuf,
    /// Extra variables given to each `pm` call after the `PM_*` removal.
    env: RefCell<Vec<(String, String)>>,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

impl Sandbox {
    pub fn new() -> Self {
        Self::with(json!({}), true, &[])
    }

    pub fn with(budgets: Value, opened: bool, open_args: &[&str]) -> Self {
        let tmp = make_temp_dir();
        let sb = Sandbox {
            target: tmp.join("target"),
            base: tmp.join("campaigns"),
            inst: tmp.join("campaigns").join("camp"),
            tmp,
            env: RefCell::new(Vec::new()),
        };
        fs::create_dir(&sb.target).unwrap();
        sb.git(&["init", "-q", "."]);
        fs::write(sb.target.join(".gitignore"), "scratch/\n").unwrap();
        sb.commit("alpha/state.txt", "alpha-ok\n", "init alpha");
        sb.commit("beta/state.txt", "beta-ok\n", "init beta");
        sb.flag("flag-ok");
        sb.make_instance(budgets);
        if opened {
            let mut args = vec!["open"];
            args.extend_from_slice(open_args);
            sb.ok(&args);
        }
        sb
    }

    /// Set a variable for the following `pm` calls (and the checks they run).
    pub fn set_env(&self, key: &str, value: &str) {
        let mut env = self.env.borrow_mut();
        env.retain(|(k, _)| k != key);
        env.push((key.to_string(), value.to_string()));
    }

    // -- target
    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(args, &self.target)
    }

    pub fn git_in(&self, args: &[&str], cwd: &Path) -> String {
        let done = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(
            done.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&done.stderr)
        );
        String::from_utf8_lossy(&done.stdout).trim().to_string()
    }

    pub fn commit(&self, rel: &str, text: &str, message: &str) {
        let target = self.target.clone();
        self.commit_in(rel, text, message, &target);
    }

    pub fn commit_in(&self, rel: &str, text: &str, message: &str, cwd: &Path) {
        let path = cwd.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        self.git_in(&["add", "-A"], cwd);
        self.git_in(&["commit", "-q", "-m", message], cwd);
    }

    /// An ignored file read by the check `flaky`: it changes the result without changing the key.
    pub fn flag(&self, text: &str) {
        let path = self.target.join("scratch").join("flag.txt");
        let _ = fs::create_dir(path.parent().unwrap());
        fs::write(path, format!("{text}\n")).unwrap();
    }

    // -- instance
    fn make_instance(&self, budgets: Value) {
        let decl = json!({
            "schema_version": 2, "id": "camp", "goal": "regression", "plan_ref": "plan.md",
            "checklist_ref": "plan.md", "declared": "2026-01-01T00:00:00+00:00",
            "target": {"root": "target", "env_probe": []}, "phases": ["prepare", "change"],
            "review": {"round_cap": 3}, "budgets": budgets,
            "briefing": {"on_phase": true, "every_hours": null},
            "checks": [
                check("alpha-suite", "cat alpha/state.txt", "alpha-ok", &["alpha"], &["alpha-unit"]),
                check("alpha-unit", "cat alpha/state.txt; echo unit", "alpha-ok", &["alpha"], &[]),
                check("beta-unit", "cat beta/state.txt", "beta-ok", &["beta"], &[]),
                check("flaky", "cat scratch/flag.txt", "flag-ok", &["beta"], &[]),
                check("full", "cat alpha/state.txt beta/state.txt | tr -d '\\n'", "alpha-okbeta-ok",
                      &["."], &[]),
            ],
            "groups": [{"group": "g-a", "checks": ["alpha-suite", "alpha-unit"]},
                       {"group": "g-b", "checks": ["beta-unit"]}],
            "final": {"checks": ["full"]},
            "cells": [cell_entry("a1", "g-a"), cell_entry("a2", "g-a"), cell_entry("b1", "g-b")],
        });
        fs::create_dir_all(&self.inst).unwrap();
        fs::write(self.base.join("ACTIVE"), "camp\n").unwrap();
        Self::write(&self.inst.join("campaign.json"), &decl);
        Self::write(
            &self.inst.join("evidence_pointers.json"),
            &json!({"schema_version": 1, "pointers": []}),
        );
        fs::write(self.inst.join("journey.jsonl"), "").unwrap();
        for entry in decl["cells"].as_array().unwrap() {
            let name = entry["cell"].as_str().unwrap();
            let folder = self.inst.join("cells").join(name).join("phases");
            fs::create_dir_all(&folder).unwrap();
            let mut body = vec![format!("# {name}"), String::new()];
            for section in SECTIONS {
                body.extend([
                    format!("## {section}"),
                    String::new(),
                    "없음".to_string(),
                    String::new(),
                ]);
            }
            fs::write(folder.parent().unwrap().join("cell.md"), body.join("\n")).unwrap();
            for phase in decl["phases"].as_array().unwrap() {
                let phase = phase.as_str().unwrap();
                Self::write(
                    &folder.join(format!("{phase}.status.json")),
                    &json!({
                        "schema_version": 2, "cell": name, "phase": phase, "state": "pending",
                        "attempt": 1, "started": null, "ended": null,
                        "proof": {"check": "echo ready", "expect": "ready", "ok": false, "source": null},
                        "by": null}),
                );
            }
        }
    }

    pub fn write(path: &Path, data: &Value) {
        let text = serde_json::to_string_pretty(data).unwrap();
        fs::write(path, text + "\n").unwrap();
    }

    pub fn read(path: &Path) -> Value {
        let text =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    }

    pub fn edit_decl(&self, change: impl FnOnce(&mut Value)) {
        let path = self.inst.join("campaign.json");
        let mut decl = Self::read(&path);
        change(&mut decl);
        Self::write(&path, &decl);
    }

    pub fn set_check(&self, ident: &str, fields: Value) {
        self.edit_decl(|d| update(find_mut(&mut d["checks"], "id", ident), fields));
    }

    pub fn set_cell(&self, name: &str, fields: Value) {
        self.edit_decl(|d| update(find_mut(&mut d["cells"], "cell", name), fields));
    }

    pub fn set_group(&self, name: &str, checks: &[&str]) {
        self.edit_decl(|d| {
            update(
                find_mut(&mut d["groups"], "group", name),
                json!({ "checks": checks }),
            )
        });
    }

    // -- executor
    pub fn pm_stdin(&self, args: &[&str], stdin: &str) -> (i32, Option<Value>) {
        let mut command = clean_command(PM);
        command
            .arg("--campaigns")
            .arg(&self.base)
            .args(["--executor", "test"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, value) in self.env.borrow().iter() {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn pm");
        {
            let mut input = child.stdin.take().unwrap();
            input.write_all(stdin.as_bytes()).expect("write pm stdin");
        }
        let done = child.wait_with_output().expect("wait for pm");
        let code = done
            .status
            .code()
            .unwrap_or_else(|| panic!("{args:?}: pm ended by a signal"));
        let text = String::from_utf8(done.stdout).expect("pm stdout is UTF-8");
        let data = if text.trim().is_empty() {
            None
        } else {
            Some(
                serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("{args:?}: stdout is not JSON ({e}): {text}")),
            )
        };
        (code, data)
    }

    pub fn pm(&self, args: &[&str]) -> (i32, Option<Value>) {
        self.pm_stdin(args, "")
    }

    pub fn ok(&self, args: &[&str]) -> Value {
        let (code, data) = self.pm(args);
        let data = data.unwrap_or(Value::Null);
        assert_eq!(code, 0, "{args:?}: {data}");
        data
    }

    pub fn no(&self, args: &[&str]) -> Value {
        self.no_code(args, 1)
    }

    pub fn no_code(&self, args: &[&str], code: i32) -> Value {
        let (got, data) = self.pm(args);
        let data = data.unwrap_or(Value::Null);
        assert_eq!(got, code, "{args:?}: {data}");
        data
    }

    /// The Stop hook with `{"session_id", "hook_event_name": "Stop", ..payload}` on stdin.
    pub fn hook_with(&self, session: &str, harness: &str, payload: Value) -> Option<Value> {
        let mut event = Map::new();
        event.insert("session_id".into(), json!(session));
        event.insert("hook_event_name".into(), json!("Stop"));
        if let Value::Object(extra) = payload {
            for (key, value) in extra {
                event.insert(key, value);
            }
        }
        let stdin = serde_json::to_string(&Value::Object(event)).unwrap();
        let (code, data) = self.pm_stdin(&["stop-hook", "--harness", harness], &stdin);
        assert_eq!(code, 0);
        data
    }

    pub fn hook(&self) -> Option<Value> {
        self.hook_with("s1", "claude", json!({}))
    }

    pub fn hook_s(&self, session: &str) -> Option<Value> {
        self.hook_with(session, "claude", json!({}))
    }

    pub fn finish(&self, cells: &[&str]) {
        for name in cells {
            for phase in ["prepare", "change"] {
                self.ok(&["phase", "start", name, phase]);
                self.ok(&["phase", "close", name, phase]);
            }
        }
    }

    /// Reopen a cell for a fix, finish the change again and, if given, pass its gate again.
    pub fn refix(&self, name: &str, gate: Option<&str>) {
        self.ok(&[
            "cell",
            "reopen",
            name,
            "--reason",
            "fix the blocking findings",
        ]);
        self.ok(&["phase", "start", name, "change"]);
        self.ok(&["phase", "close", name, "change"]);
        if let Some(group) = gate {
            self.ok(&["gate", "run", group]);
        }
    }

    // -- reading state
    pub fn cell(&self, name: &str) -> Value {
        let decl = Self::read(&self.inst.join("campaign.json"));
        decl["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["cell"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no cell {name}"))
    }

    pub fn phase(&self, name: &str, phase: &str) -> Value {
        Self::read(
            &self
                .inst
                .join("cells")
                .join(name)
                .join("phases")
                .join(format!("{phase}.status.json")),
        )
    }

    pub fn gate(&self, group: &str) -> Value {
        Self::read(&self.inst.join("gates").join(format!("{group}.status.json")))
    }

    /// Receipts under `receipts/<check or *>/<digits>.json`, sorted by (check, n).
    pub fn receipts_of(&self, check: Option<&str>) -> Vec<Value> {
        let root = self.inst.join("receipts");
        let mut folders = Vec::new();
        match check {
            Some(name) => folders.push(root.join(name)),
            None => {
                if let Ok(entries) = fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            folders.push(entry.path());
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for folder in folders {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                out.push(Self::read(&path));
            }
        }
        out.sort_by(|a, b| {
            let ka = (
                a["check"].as_str().unwrap_or("").to_string(),
                a["n"].as_i64(),
            );
            let kb = (
                b["check"].as_str().unwrap_or("").to_string(),
                b["n"].as_i64(),
            );
            ka.cmp(&kb)
        });
        out
    }

    pub fn receipts(&self, check: &str) -> Vec<Value> {
        self.receipts_of(Some(check))
    }

    pub fn all_receipts(&self) -> Vec<Value> {
        self.receipts_of(None)
    }

    pub fn events_of(&self, name: Option<&str>) -> Vec<Value> {
        let text = fs::read_to_string(self.inst.join("journey.jsonl")).unwrap();
        text.lines()
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .unwrap_or_else(|e| panic!("journey line is not JSON ({e}): {line}"))
            })
            .filter(|e| name.is_none_or(|n| e["event"] == n))
            .collect()
    }

    pub fn events(&self, name: &str) -> Vec<Value> {
        self.events_of(Some(name))
    }

    pub fn all_events(&self) -> Vec<Value> {
        self.events_of(None)
    }

    pub fn monitor(&self) -> Value {
        Self::read(&self.inst.join("monitor.json"))
    }

    pub fn outcome(&self, data: &Value, check: &str) -> Value {
        data["checks"]
            .as_array()
            .unwrap_or_else(|| panic!("no checks in {data}"))
            .iter()
            .find(|item| item["check"] == check)
            .cloned()
            .unwrap_or_else(|| panic!("no outcome for {check} in {data}"))
    }

    pub fn group_state(&self, group: &str) -> Value {
        let status = self.ok(&["status"]);
        status["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["group"] == group)
            .unwrap_or_else(|| panic!("no group {group}"))["state"]
            .clone()
    }

    pub fn kinds(&self) -> Vec<String> {
        self.ok(&["status"])["actionable"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["kind"].as_str().unwrap().to_string())
            .collect()
    }

    /// The actionable kinds of one cell.
    pub fn kinds_of(&self, cell: &str) -> Vec<String> {
        self.ok(&["status"])["actionable"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["cell"] == cell)
            .map(|item| item["kind"].as_str().unwrap().to_string())
            .collect()
    }

    pub fn assert_lint(&self) {
        assert!(Path::new(LINT).is_file(), "lint binary is missing: {LINT}");
        let done = clean_command(LINT)
            .arg(&self.inst)
            .arg("--root")
            .arg(&self.tmp)
            .output()
            .expect("run check_campaign");
        assert_eq!(
            done.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&done.stdout)
        );
    }

    pub fn spec(&self, name: &str, group: &str, extra: Value) -> String {
        let path = self.tmp.join(format!("spec-{name}.json"));
        let mut data = json!({"cell": name, "goal": format!("work on {name}"), "group": group,
                              "parent": null, "needs": [], "reason": "found on the way",
                              "prepare": {"check": "echo ready", "expect": "ready"},
                              "change": {"check": "echo ready", "expect": "ready"}});
        update(&mut data, extra);
        Self::write(&path, &data);
        s(&path)
    }

    pub fn review(&self, round_number: u32, clean: bool) -> String {
        let path = self.tmp.join(format!("review-{round_number}.json"));
        let blocking = if clean {
            json!([])
        } else {
            json!([{"id": format!("r{round_number}-1"), "where": "alpha/state.txt:1", "settled": false}])
        };
        Self::write(
            &path,
            &json!({"round": round_number, "ran": true, "blocking": blocking, "non_blocking": [],
                    "clean": clean}),
        );
        s(&path)
    }
}

/// `[item[key] for item in items]` as a JSON list.
pub fn pluck(items: &[Value], key: &str) -> Value {
    Value::Array(items.iter().map(|item| item[key].clone()).collect())
}
