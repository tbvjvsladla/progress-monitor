//! Negative control: the checker must pass a valid instance built from the real
//! template, and each single-defect copy must fail with exactly its own rule.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    check, file_name, findings_value, is_dir, parse_object, read_utf8, rules::read_events,
    sorted_entries, Finding, LintError, LintResult, PLACEHOLDER, SKELETON_CELL, STATES,
};
use crate::jsonio;
use crate::py::{is_digits, obj, py_repr, py_str, Obj};
use crate::{SCHEMA, SUFFIX, TOOL};

/// The baseline's `tempfile.gettempdir()`: the first of `TMPDIR`, `TEMP`, `TMP`,
/// `/tmp`, `/var/tmp`, `/usr/tmp` and the working folder where a new file can be
/// created and written.
fn temp_base() -> LintResult<PathBuf> {
    let cwd = std::env::current_dir().map_err(|e| LintError::io(Path::new("."), &e))?;
    let mut candidates: Vec<PathBuf> = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .filter_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
        .map(|dir| cwd.join(dir))
        .collect();
    candidates.extend(["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from));
    candidates.push(cwd);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    for dir in &candidates {
        for attempt in 0..100u32 {
            let probe = dir.join(format!(
                "pm-lint-probe-{}-{nanos}-{attempt}",
                std::process::id()
            ));
            let opened = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&probe);
            match opened {
                Ok(mut file) => {
                    let written = std::io::Write::write_all(&mut file, b"blat");
                    drop(file);
                    let _ = fs::remove_file(&probe);
                    if written.is_ok() {
                        return Ok(dir.clone());
                    }
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
    }
    let listed: Vec<String> = candidates
        .iter()
        .map(|d| format!("'{}'", d.display()))
        .collect();
    Err(LintError::Internal(format!(
        "FileNotFoundError: [Errno 2] No usable temporary directory found in [{}]",
        listed.join(", ")
    )))
}

/// A folder under the system temporary folder, removed when dropped.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub fn new() -> LintResult<ScratchDir> {
        let base = temp_base()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for attempt in 0..100u32 {
            let path = base.join(format!("pm-lint-{}-{nanos}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(ScratchDir { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(LintError::io(&path, &err)),
            }
        }
        Err(LintError::Internal(format!(
            "FileExistsError: no free temporary folder in {}",
            base.display()
        )))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn io<T>(path: &Path, result: std::io::Result<T>) -> LintResult<T> {
    result.map_err(|e| LintError::io(path, &e))
}

/// Copy a folder; linked files and folders are copied as their contents.
fn copy_tree(from: &Path, to: &Path) -> LintResult<()> {
    io(to, fs::create_dir_all(to))?;
    for entry in sorted_entries(from)? {
        let dest = to.join(entry.file_name().unwrap_or_default());
        if is_dir(&entry) {
            copy_tree(&entry, &dest)?;
        } else {
            io(&entry, fs::copy(&entry, &dest))?;
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> LintResult<()> {
    io(path, fs::remove_dir_all(path))
}

fn remove_file(path: &Path) -> LintResult<()> {
    io(path, fs::remove_file(path))
}

fn write_text(path: &Path, text: &str) -> LintResult<()> {
    io(path, fs::write(path, text))
}

fn append_text(path: &Path, text: &str) -> LintResult<()> {
    use std::io::Write;
    let mut file = io(
        path,
        fs::OpenOptions::new().append(true).create(true).open(path),
    )?;
    io(path, file.write_all(text.as_bytes()))
}

fn write_json(path: &Path, data: &Value) -> LintResult<()> {
    write_text(path, &(jsonio::pretty(data) + "\n"))
}

fn not_shaped(path: &Path) -> LintError {
    LintError::Internal(format!(
        "TypeError: {}: the fixture does not have the expected shape",
        path.display()
    ))
}

/// Read a JSON file, change it, write it back.
fn edit_json(path: &Path, change: impl FnOnce(&mut Obj) -> Option<()>) -> LintResult<()> {
    let mut data = parse_object(path)?;
    let map = data.as_object_mut().ok_or_else(|| not_shaped(path))?;
    change(map).ok_or_else(|| not_shaped(path))?;
    write_json(path, &data)
}

fn entry_mut<'a>(list: &'a mut Value, key: &str, name: &str) -> Option<&'a mut Obj> {
    list.as_array_mut()?
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|e| e.get(key).and_then(Value::as_str) == Some(name))
}

fn set(map: &mut Obj, key: &str, value: Value) -> Option<()> {
    map.insert(key.to_string(), value);
    Some(())
}

fn phases_of(instance: &Path) -> LintResult<Vec<String>> {
    let path = instance.join("campaign.json");
    let decl = parse_object(&path)?;
    let phases = decl
        .get("phases")
        .and_then(Value::as_array)
        .ok_or_else(|| not_shaped(&path))?;
    Ok(phases
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect())
}

const FIXTURE_UNIT_CMD: &str = "cargo test --locked";
const FIXTURE_STYLE_CMD: &str = "tools/style.sh";

/// Fill a copy of the real template: every phase state, a passed gate, a receipt,
/// and a journey that adds, regroups and cancels cells.
pub fn build_valid_instance(template_dir: &Path, root: &Path) -> LintResult<PathBuf> {
    let instance = root.join("campaigns").join("camp-control");
    copy_tree(template_dir, &instance)?;
    let skeleton = instance.join("cells").join(SKELETON_CELL);
    let cell_names = ["alpha", "beta", "gamma", "delta"];
    for name in cell_names {
        copy_tree(&skeleton, &instance.join("cells").join(name))?;
    }
    remove_tree(&skeleton)?;
    io(&root.join("docs"), fs::create_dir(root.join("docs")))?;
    write_text(&root.join("docs").join("evidence.txt"), "observed output\n")?;

    edit_json(&instance.join("campaign.json"), |decl| {
        set(decl, "id", json!("camp-control"))?;
        set(decl, "goal", json!("control instance"))?;
        set(decl, "plan_ref", json!("docs/evidence.txt"))?;
        set(decl, "checklist_ref", json!("docs/evidence.txt"))?;
        set(decl, "declared", json!("2026-01-01T00:00:00+00:00"))?;
        set(decl, "target", json!({"root": ".", "env_probe": []}))?;
        set(
            decl,
            "checks",
            json!([
                {"id": "unit", "cmd": FIXTURE_UNIT_CMD, "cwd": ".", "expect": "OK",
                 "inputs": ["src"], "env": [], "covers": ["unit-fast"]},
                {"id": "unit-fast", "cmd": "cargo test --locked fast", "cwd": ".", "expect": "OK",
                 "inputs": ["src"], "env": [], "covers": []},
                {"id": "style", "cmd": FIXTURE_STYLE_CMD, "cwd": ".", "expect": "style ok",
                 "inputs": ["src"], "env": [], "covers": []}
            ]),
        )?;
        set(
            decl,
            "groups",
            json!([
                {"group": "g-one", "checks": ["unit", "unit-fast"]},
                {"group": "g-two", "checks": ["style"]},
                {"group": "g-three", "checks": ["style"]}
            ]),
        )?;
        set(decl, "final", json!({"checks": ["unit", "style"]}))?;
        set(
            decl,
            "cells",
            json!([
                {"cell": "alpha", "goal": "first unit", "group": "g-one", "needs": [], "review": "none",
                 "origin": "plan", "parent": null, "state": "closed"},
                {"cell": "beta", "goal": "second unit", "group": "g-two", "needs": ["alpha"], "review": "judgment",
                 "origin": "plan", "parent": null, "state": "open"},
                {"cell": "gamma", "goal": "found on the way", "group": "g-three", "needs": [], "review": "none",
                 "origin": "auto", "parent": null, "state": "open"},
                {"cell": "delta", "goal": "split from beta, dropped", "group": "g-two", "needs": [],
                 "review": "judgment", "origin": "auto", "parent": "beta", "state": "cancelled"}
            ]),
        )
    })?;
    let phases = phases_of(&instance)?;
    io(
        &instance.join("logs"),
        fs::create_dir(instance.join("logs")),
    )?;
    for name in cell_names {
        let planned: Vec<&str> = match name {
            "alpha" => vec!["done"; phases.len()],
            "beta" => vec!["failed"],
            "gamma" => vec!["running"],
            _ => vec![],
        };
        let cell_dir = instance.join("cells").join(name);
        let summary = cell_dir.join("cell.md");
        let text = read_utf8(&summary)?.replace(PLACEHOLDER, "없음");
        write_text(&summary, &text)?;
        for (index, phase) in phases.iter().enumerate() {
            let state = planned.get(index).copied().unwrap_or("pending");
            let log = format!("logs/{name}-{phase}-1.log");
            let closed = matches!(state, "done" | "failed");
            if closed {
                let body = if state == "done" {
                    "verification passed\n"
                } else {
                    "boom\n"
                };
                write_text(&crate::paths::join(&instance, &log), body)?;
            }
            edit_json(
                &cell_dir.join("phases").join(format!("{phase}{SUFFIX}")),
                |data| {
                    set(data, "cell", json!(name))?;
                    set(data, "state", json!(state))?;
                    set(
                        data,
                        "proof",
                        json!({
                            "check": "sh verify.sh", "expect": "verification passed",
                            "ok": state == "done", "source": if closed { json!(log) } else { Value::Null }
                        }),
                    )?;
                    let by = if closed {
                        json!({"tool": TOOL, "log": log, "executor": "control"})
                    } else {
                        Value::Null
                    };
                    set(data, "by", by)
                },
            )?;
        }
    }
    let inputs = json!({"src": "0".repeat(40)});
    let receipts = instance.join("receipts").join("unit");
    io(&receipts, fs::create_dir_all(&receipts))?;
    write_text(&instance.join("logs").join("unit-1.log"), "OK\n")?;
    write_json(
        &receipts.join("1.json"),
        &json!({
            "schema_version": SCHEMA, "check": "unit", "n": 1, "scope": "gate",
            "key": {"cmd": FIXTURE_UNIT_CMD, "cwd": ".", "expect": "OK", "env": {}, "inputs": inputs,
                    "host": "control", "env_probe": {}},
            "key_sha256": "0".repeat(64), "reusable": true, "result": "PASS", "exit_code": 0, "error": null,
            "log": "logs/unit-1.log", "started": "2026-01-01T03:00:00+00:00", "ended": "2026-01-01T03:00:01+00:00",
            "seconds": 1.0, "executor": "control", "forced": false, "reason": null, "target": ".",
            "covers": {"unit-fast": {"inputs": inputs}}
        }),
    )?;
    io(
        &instance.join("gates"),
        fs::create_dir(instance.join("gates")),
    )?;
    write_json(
        &instance.join("gates").join(format!("g-one{SUFFIX}")),
        &json!({
            "schema_version": SCHEMA, "group": "g-one", "state": "passed", "runs": 1,
            "started": "2026-01-01T03:00:00+00:00", "ended": "2026-01-01T03:00:01+00:00",
            "members": ["alpha"], "hitl": null,
            "checks": [{"check": "unit", "outcome": "ran", "result": "PASS",
                        "receipt": "receipts/unit/1.json", "via": null},
                       {"check": "unit-fast", "outcome": "covered", "result": null,
                        "receipt": null, "via": "receipts/unit/1.json"}]
        }),
    )?;
    let first = phases.first().cloned().unwrap_or_default();
    let journey = [
        json!({"utc": "2026-01-01T00:00:00+00:00", "event": "campaign-open",
               "cells": [{"cell": "alpha", "group": "g-one"}, {"cell": "beta", "group": "g-two"}],
               "groups": ["g-one", "g-two"], "briefing": {"on_phase": true, "every_hours": null},
               "briefing_source": "default"}),
        json!({"utc": "2026-01-01T01:00:00+00:00", "event": "phase-close", "cell": "alpha", "phase": first,
               "ok": true, "log": format!("logs/alpha-{first}-1.log")}),
        json!({"utc": "2026-01-01T02:00:00+00:00", "event": "cell-add", "cell": "gamma", "group": "g-two",
               "parent": null, "review": "none", "reason": "found on the way"}),
        json!({"utc": "2026-01-01T02:10:00+00:00", "event": "regroup", "cell": "gamma", "from": "g-two",
               "to": "g-three", "reason": "verified by a different check", "group_checks": ["style"]}),
        json!({"utc": "2026-01-01T02:20:00+00:00", "event": "cell-add", "cell": "delta", "group": "g-two",
               "parent": "beta", "review": "judgment", "reason": "split"}),
        json!({"utc": "2026-01-01T02:30:00+00:00", "event": "cell-cancel", "cell": "delta", "reason": "not needed"}),
        json!({"utc": "2026-01-01T03:00:01+00:00", "event": "gate", "group": "g-one", "state": "passed",
               "checks": [{"check": "unit", "outcome": "ran"}, {"check": "unit-fast", "outcome": "covered"}]}),
    ];
    let lines: String = journey.iter().map(|e| jsonio::compact(e) + "\n").collect();
    write_text(&instance.join("journey.jsonl"), &lines)?;
    edit_json(&instance.join("evidence_pointers.json"), |data| {
        set(
            data,
            "pointers",
            json!([
                {"kind": "log", "path": "docs/evidence.txt", "cell": "alpha", "note": "control"}
            ]),
        )
    })?;
    Ok(instance)
}

type Seed = Box<dyn Fn(&Path) -> LintResult<()>>;

/// One single-defect copy: its name, the rule it must trip, and how it is made.
pub struct Case {
    pub name: &'static str,
    pub rule: &'static str,
    pub seed: Seed,
}

fn case(
    name: &'static str,
    rule: &'static str,
    seed: impl Fn(&Path) -> LintResult<()> + 'static,
) -> Case {
    Case {
        name,
        rule,
        seed: Box::new(seed),
    }
}

type Change = fn(&mut Obj) -> Option<()>;

fn phase_edit(
    cell: &'static str,
    file: String,
    change: Change,
) -> impl Fn(&Path) -> LintResult<()> {
    move |inst| {
        edit_json(
            &inst.join("cells").join(cell).join("phases").join(&file),
            change,
        )
    }
}

fn proof_mut(data: &mut Obj) -> Option<&mut Obj> {
    data.get_mut("proof")?.as_object_mut()
}

fn summary_edit(change: fn(&str) -> String) -> impl Fn(&Path) -> LintResult<()> {
    move |inst| {
        let path = inst.join("cells").join("beta").join("cell.md");
        let text = read_utf8(&path)?;
        write_text(&path, &change(&text))
    }
}

fn append(name: &'static str, text: String) -> impl Fn(&Path) -> LintResult<()> {
    move |inst| append_text(&crate::paths::join(inst, name), &text)
}

fn file_edit(rel: &'static [&'static str], change: Change) -> impl Fn(&Path) -> LintResult<()> {
    move |inst| {
        let path = rel.iter().fold(inst.to_path_buf(), |p, part| p.join(part));
        edit_json(&path, change)
    }
}

const DECLARATION: &[&str] = &["campaign.json"];
const POINTERS: &[&str] = &["evidence_pointers.json"];
const GATE: &[&str] = &["gates", "g-one.status.json"];

fn first_pointer(data: &mut Obj) -> Option<&mut Obj> {
    data.get_mut("pointers")?.get_mut(0)?.as_object_mut()
}

fn cells_mut(decl: &mut Obj) -> Option<&mut Value> {
    decl.get_mut("cells")
}

fn push_check(decl: &mut Obj, entry: Value) -> Option<()> {
    decl.get_mut("checks")?.as_array_mut()?.push(entry);
    Some(())
}

/// Text after the first `## 남은 계획` heading line is dropped.
fn empty_last_section(text: &str) -> String {
    let heading = "## 남은 계획\n";
    match text.find(heading) {
        Some(at) => text[..at + heading.len()].to_string(),
        None => text.to_string(),
    }
}

/// The single-defect seeds, in the baseline's order.
pub fn control_cases(phases: &[String]) -> Vec<Case> {
    let first = format!("{}{SUFFIX}", phases.first().map_or("", String::as_str));
    let last = format!("{}{SUFFIX}", phases.last().map_or("", String::as_str));
    let last_phase = phases.last().cloned().unwrap_or_default();
    let first_copy = first.clone();
    let last_copy = last.clone();
    vec![
        case(
            "proof-blank",
            "proof-blank",
            phase_edit("alpha", last.clone(), |d| {
                set(proof_mut(d)?, "check", json!(""))
            }),
        ),
        case(
            "source-missing",
            "source-missing",
            phase_edit("alpha", first.clone(), |d| {
                set(proof_mut(d)?, "source", Value::Null)
            }),
        ),
        case(
            "state-invalid",
            "state-invalid",
            phase_edit("alpha", last.clone(), |d| {
                set(d, "state", json!("finished"))
            }),
        ),
        case(
            "done-not-ok",
            "done-not-ok",
            phase_edit("alpha", first.clone(), |d| {
                set(proof_mut(d)?, "ok", json!(false))
            }),
        ),
        case("undeclared-phase", "undeclared", move |inst| {
            let dir = inst.join("cells").join("alpha").join("phases");
            io(
                &dir,
                fs::copy(dir.join(&first_copy), dir.join(format!("deploy{SUFFIX}"))),
            )
            .map(|_| ())
        }),
        case("undeclared-cell", "undeclared", |inst| {
            copy_tree(
                &inst.join("cells").join("alpha"),
                &inst.join("cells").join("stray"),
            )
        }),
        case(
            "summary-section-missing",
            "cell-summary-incomplete",
            summary_edit(|t| t.replace("## 막다른 길", "막다른 길")),
        ),
        case(
            "summary-section-empty",
            "cell-summary-incomplete",
            summary_edit(empty_last_section),
        ),
        case(
            "journey-not-json",
            "journey-invalid",
            append("journey.jsonl", "not json\n".to_string()),
        ),
        case(
            "journey-backward",
            "journey-invalid",
            append(
                "journey.jsonl",
                "{\"utc\": \"2025-12-31T00:00:00+00:00\", \"event\": \"late\"}\n".to_string(),
            ),
        ),
        case(
            "pointer-extra-key",
            "pointer-invalid",
            file_edit(POINTERS, |d| {
                set(first_pointer(d)?, "sha256", json!("0".repeat(64)))
            }),
        ),
        case(
            "pointer-path-absent",
            "pointer-invalid",
            file_edit(POINTERS, |d| {
                set(first_pointer(d)?, "path", json!("docs/absent.txt"))
            }),
        ),
        case("declared-phase-missing", "declared-missing", move |inst| {
            remove_file(
                &inst
                    .join("cells")
                    .join("beta")
                    .join("phases")
                    .join(&last_copy),
            )
        }),
        case("declared-cell-missing", "declared-missing", |inst| {
            remove_tree(&inst.join("cells").join("beta"))
        }),
        case(
            "placeholder-left",
            "placeholder-left",
            append("cells/beta/cell.md", format!("\n{PLACEHOLDER}\n")),
        ),
        case("no-cells", "no-cells", |inst| {
            edit_json(&inst.join("campaign.json"), |d| set(d, "cells", json!([])))?;
            remove_tree(&inst.join("cells"))?;
            io(&inst.join("cells"), fs::create_dir(inst.join("cells")))?;
            write_text(&inst.join("journey.jsonl"), "") // not opened yet
        }),
        case(
            "done-without-record",
            "done-unrecorded",
            phase_edit("alpha", first.clone(), |d| set(d, "by", Value::Null)),
        ),
        case("done-log-absent", "done-unrecorded", move |inst| {
            remove_file(&inst.join("logs").join(format!("alpha-{last_phase}-1.log")))
        }),
        case(
            "closed-gate-not-passed",
            "closed-without-gate",
            file_edit(GATE, |d| set(d, "state", json!("failed"))),
        ),
        case(
            "closed-not-member",
            "closed-without-gate",
            file_edit(GATE, |d| set(d, "members", json!([]))),
        ),
        case("structure-add-unlogged", "structure-unlogged", |inst| {
            copy_tree(
                &inst.join("cells").join("gamma"),
                &inst.join("cells").join("epsilon"),
            )?;
            edit_json(&inst.join("campaign.json"), |d| {
                cells_mut(d)?.as_array_mut()?.push(json!(
                    {"cell": "epsilon", "goal": "slipped in", "group": "g-three", "needs": [], "review": "none",
                     "origin": "auto", "parent": null, "state": "open"}));
                Some(())
            })
        }),
        case(
            "structure-cancel-unlogged",
            "structure-unlogged",
            file_edit(DECLARATION, |d| {
                set(
                    entry_mut(cells_mut(d)?, "cell", "beta")?,
                    "state",
                    json!("cancelled"),
                )
            }),
        ),
        case(
            "structure-regroup-unlogged",
            "structure-unlogged",
            file_edit(DECLARATION, |d| {
                set(
                    entry_mut(cells_mut(d)?, "cell", "gamma")?,
                    "group",
                    json!("g-two"),
                )
            }),
        ),
        case(
            "auto-cap-exceeded",
            "auto-cap-exceeded",
            file_edit(DECLARATION, |d| {
                set(
                    d.get_mut("budgets")?.as_object_mut()?,
                    "auto_cells",
                    json!(1),
                )
            }),
        ),
        case(
            "check-duplicate-id",
            "check-duplicate",
            file_edit(DECLARATION, |d| {
                push_check(
                    d,
                    json!({"id": "style", "cmd": "tools/other.sh", "cwd": ".", "expect": "ok",
                                 "inputs": ["src"], "env": [], "covers": []}),
                )
            }),
        ),
        case(
            "check-duplicate-command",
            "check-duplicate",
            file_edit(DECLARATION, |d| {
                push_check(
                    d,
                    json!({"id": "style-again", "cmd": FIXTURE_STYLE_CMD, "cwd": ".",
                                 "expect": "style ok", "inputs": ["src"], "env": [], "covers": []}),
                )
            }),
        ),
        case(
            "ref-needs-unknown",
            "ref-invalid",
            file_edit(DECLARATION, |d| {
                set(
                    entry_mut(cells_mut(d)?, "cell", "beta")?,
                    "needs",
                    json!(["absent"]),
                )
            }),
        ),
        case(
            "ref-covers-unknown",
            "ref-invalid",
            file_edit(DECLARATION, |d| {
                set(
                    entry_mut(d.get_mut("checks")?, "id", "style")?,
                    "covers",
                    json!(["absent"]),
                )
            }),
        ),
        case(
            "ref-group-check-unknown",
            "ref-invalid",
            file_edit(DECLARATION, |d| {
                let group = d.get_mut("groups")?.get_mut(2)?.as_object_mut()?;
                set(group, "checks", json!(["absent"]))
            }),
        ),
        case(
            "ref-parent-unknown",
            "ref-invalid",
            file_edit(DECLARATION, |d| {
                set(
                    entry_mut(cells_mut(d)?, "cell", "delta")?,
                    "parent",
                    json!("absent"),
                )
            }),
        ),
        case(
            "ref-final-unknown",
            "ref-invalid",
            file_edit(DECLARATION, |d| {
                set(
                    d.get_mut("final")?.as_object_mut()?,
                    "checks",
                    json!(["absent"]),
                )
            }),
        ),
        case(
            "gate-result-not-pass",
            "gate-not-proven",
            file_edit(GATE, |d| {
                let item = d.get_mut("checks")?.get_mut(0)?.as_object_mut()?;
                set(item, "result", json!("FAIL"))
            }),
        ),
        case("gate-receipt-absent", "gate-not-proven", |inst| {
            remove_file(&inst.join("receipts").join("unit").join("1.json"))
        }),
    ]
}

/// The phase states present in the instance, sorted and distinct.
fn instance_states(instance: &Path) -> LintResult<Vec<Value>> {
    let mut states: Vec<Value> = Vec::new();
    let cells = instance.join("cells");
    for cell in sorted_entries(&cells)? {
        let phase_dir = cell.join("phases");
        if !is_dir(&cell) || !is_dir(&phase_dir) {
            continue;
        }
        for path in sorted_entries(&phase_dir)? {
            if !file_name(&path).ends_with(SUFFIX) {
                continue;
            }
            let data = parse_object(&path)?;
            let state = data.get("state").cloned().unwrap_or(Value::Null);
            if !states.contains(&state) {
                states.push(state);
            }
        }
    }
    states.sort_by_key(py_str);
    Ok(states)
}

/// Count what the valid instance carries beyond phase states.
fn instance_contents(instance: &Path) -> LintResult<Obj> {
    let mut passed = 0;
    let gates = instance.join("gates");
    if is_dir(&gates) {
        for path in sorted_entries(&gates)? {
            if file_name(&path).ends_with(SUFFIX)
                && parse_object(&path)?.get("state").and_then(Value::as_str) == Some("passed")
            {
                passed += 1;
            }
        }
    }
    let mut receipts = 0;
    let receipts_dir = instance.join("receipts");
    if is_dir(&receipts_dir) {
        for check_dir in sorted_entries(&receipts_dir)? {
            if !is_dir(&check_dir) {
                continue;
            }
            for path in sorted_entries(&check_dir)? {
                let name = file_name(&path);
                if name.strip_suffix(".json").is_some_and(is_digits) {
                    receipts += 1;
                }
            }
        }
    }
    let events = read_events(&instance.join("journey.jsonl"))?;
    let count = |kind: &str| {
        events
            .iter()
            .filter(|e| e.get("event").and_then(Value::as_str) == Some(kind))
            .count()
    };
    Ok(obj([
        ("gates_passed", Value::from(passed)),
        ("receipts", Value::from(receipts)),
        ("cell_add_events", Value::from(count("cell-add"))),
        ("cell_cancel_events", Value::from(count("cell-cancel"))),
        ("regroup_events", Value::from(count("regroup"))),
    ]))
}

/// What the control saw.
#[derive(Debug, Default)]
pub struct Report {
    pub valid_instance_states: Vec<Value>,
    pub valid_instance: Obj,
    /// Case name → `{expected, observed, outcome}`, in case order.
    pub cases: Obj,
}

fn repr_findings(findings: &[Finding]) -> String {
    py_repr(&findings_value(findings))
}

/// Build the valid instance and every defect copy; return the report and the
/// problems (empty when the checker is sound).
pub fn negative_control(template_dir: &Path) -> LintResult<(Report, Vec<String>)> {
    let mut report = Report::default();
    let mut problems = Vec::new();
    let scratch = ScratchDir::new()?;
    let base = scratch.path();
    let template_root = template_dir.ancestors().nth(2).unwrap_or(Path::new("/"));
    let leftover = check(template_dir, template_root, true)?;
    if !leftover.is_empty() {
        problems.push(format!(
            "the template does not pass: {}",
            repr_findings(&leftover)
        ));
        return Ok((report, problems));
    }
    let clean_root = base.join("clean");
    io(&clean_root, fs::create_dir(&clean_root))?;
    let clean = build_valid_instance(template_dir, &clean_root)?;
    let found = check(&clean, &clean_root, false)?;
    if !found.is_empty() {
        problems.push(format!(
            "the valid instance does not pass: {}",
            repr_findings(&found)
        ));
        return Ok((report, problems));
    }
    report.valid_instance_states = instance_states(&clean)?;
    report.valid_instance = instance_contents(&clean)?;
    let all_present = report
        .valid_instance
        .values()
        .all(|v| v.as_u64().is_some_and(|n| n > 0));
    if !all_present {
        problems.push(format!(
            "the valid instance must carry a passed gate, a receipt and structural events, got {}",
            py_repr(&Value::Object(report.valid_instance.clone()))
        ));
    }
    let every_state: Vec<Value> = {
        let mut names = STATES.to_vec();
        names.sort_unstable();
        names.into_iter().map(Value::from).collect()
    };
    if report.valid_instance_states != every_state {
        problems.push(format!(
            "the valid instance must show every state, got {}",
            py_repr(&Value::Array(report.valid_instance_states.clone()))
        ));
    }
    let phases = phases_of(&clean)?;

    let mut judge = |name: &str, rule: &str, findings: Vec<Finding>| {
        let mut rules: Vec<&str> = findings.iter().map(|f| f.rule).collect();
        rules.sort_unstable();
        rules.dedup();
        let detected = rules == [rule];
        report.cases.insert(
            name.to_string(),
            json!({"expected": rule, "observed": rules,
                   "outcome": if detected { "detected" } else { "missed" }}),
        );
        if !detected {
            let shown: Vec<Value> = rules.iter().map(|r| Value::from(*r)).collect();
            problems.push(format!(
                "{name}: expected only {rule}, got {}",
                py_repr(&Value::Array(shown))
            ));
        }
    };

    for case in control_cases(&phases) {
        let case_root = base.join(case.name);
        copy_tree(&clean_root, &case_root)?;
        let instance = case_root.join("campaigns").join("camp-control");
        (case.seed)(&instance)?;
        judge(case.name, case.rule, check(&instance, &case_root, false)?);
    }
    let copy_root = base.join("unfilled-copy");
    let copy = copy_root.join("campaigns").join("camp-copy");
    copy_tree(template_dir, &copy)?;
    judge(
        "unfilled-copy",
        "placeholder-left",
        check(&copy, &copy_root, false)?,
    );
    let broken_root = base.join("template-defect");
    let broken = broken_root.join("campaigns").join("_template");
    copy_tree(template_dir, &broken)?;
    let first = phases.first().cloned().unwrap_or_default();
    edit_json(
        &broken
            .join("cells")
            .join(SKELETON_CELL)
            .join("phases")
            .join(format!("{first}{SUFFIX}")),
        |d| set(proof_mut(d)?, "expect", json!("")),
    )?;
    judge(
        "template-defect",
        "proof-blank",
        check(&broken, &broken_root, true)?,
    );
    drop(judge);
    Ok((report, problems))
}
