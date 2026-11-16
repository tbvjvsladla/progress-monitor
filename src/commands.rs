//! The commands that change an instance: open, phases, gates, the final run,
//! explorations, cells, reviews, hand-overs and free events.

use std::fs;

use serde_json::Value;

use crate::campaign::Campaign;
use crate::checks::{
    check_id, covered_inputs, decide, force_cap, gate_blockers, gate_state_of, member_attempts,
    reopen_gate, reset_final, run_check, run_checks, running_elsewhere, skip_check, Action,
};
use crate::cli::Args;
use crate::error::{PmError, Res};
use crate::jsonio::{read_json, write_json};
use crate::proc::execute;
use crate::py::{
    as_list, as_obj, eq_str, is_int, is_plain_name, list_or_empty, need, obj, plus_one, py_eq,
    py_repr, py_str, repr_str, strip, truthy, Num, Obj,
};
use crate::time::{now, parse_time, seconds_between, stamp, stamp_now};
use crate::{FINAL, RESERVED_EVENTS, SCHEMA, SECTIONS, TOOL};

pub type Out = Res<Obj>;

fn v(text: &str) -> Value {
    Value::from(text)
}

fn opt_v(text: Option<&str>) -> Value {
    text.map_or(Value::Null, Value::from)
}

fn list_str(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

/// A refusal carrying a result object minus its `ok`.
fn refuse_with(name: String, out: &Obj) -> PmError {
    let detail: Vec<(&str, Value)> = out
        .iter()
        .filter(|(k, _)| k.as_str() != "ok")
        .map(|(k, val)| (k.as_str(), val.clone()))
        .collect();
    PmError::refused(name, detail)
}

fn state_of(map: &Obj) -> Res<Value> {
    Ok(need(map, "state")?.clone())
}

fn is_state(map: &Obj, state: &str) -> Res<bool> {
    Ok(eq_str(Some(need(map, "state")?), state))
}

// --- open, phases -----------------------------------------------------------------

pub fn cmd_open(camp: &mut Campaign, args: &Args) -> Out {
    let _guard = camp.lock(None)?;
    if camp
        .events()?
        .iter()
        .any(|e| eq_str(e.get("event"), "campaign-open"))
    {
        return Err(PmError::refused("already-open", vec![]));
    }
    let groups = camp.group_names()?;
    let mut ids = Vec::new();
    for c in camp.check_entries() {
        ids.push(need(as_obj(c)?, "id")?.clone());
    }
    let phases = camp.phases();
    for cell in camp.cells()? {
        let group = need(&cell, "group")?;
        if !groups.iter().any(|g| eq_str(Some(group), g)) {
            return Err(PmError::usage(format!(
                "cell {} is in unknown group {}",
                py_repr(need(&cell, "cell")?),
                py_repr(group)
            )));
        }
        let name = py_str(need(&cell, "cell")?);
        for phase in &phases {
            camp.phase(&name, phase)?;
        }
    }
    for group in camp.group_entries() {
        let group = as_obj(group)?;
        for name in as_list(need(group, "checks")?)? {
            if !crate::py::contains(&ids, name) {
                return Err(PmError::usage(format!(
                    "group {} names unknown check {}",
                    py_repr(need(group, "group")?),
                    py_repr(name)
                )));
            }
        }
    }
    let mut briefing = obj([
        ("on_phase", Value::Bool(true)),
        ("every_hours", Value::Null),
    ]);
    match camp.decl.get("briefing") {
        Some(Value::Object(b)) => {
            for (k, val) in b {
                briefing.insert(k.clone(), val.clone());
            }
        }
        Some(other) if truthy(other) => {
            return Err(PmError::Internal(
                "TypeError: 'briefing' must be a mapping".into(),
            ))
        }
        _ => {}
    }
    let mut source = "default";
    if let Some(on) = args.opt("--briefing-on-phase") {
        briefing.insert("on_phase".into(), Value::Bool(on == "true"));
        source = "user";
    }
    if let Some(hours) = args.opt("--briefing-every-hours") {
        let hours = parse_float(hours)?;
        let value = if hours > 0.0 {
            crate::py::float_value(hours)
        } else {
            Value::Null
        };
        briefing.insert("every_hours".into(), value);
        source = "user";
    }
    camp.decl
        .insert("briefing".into(), Value::Object(briefing.clone()));
    camp.save_decl()?;
    for folder in ["gates", "receipts", "logs"] {
        let path = camp.dir.join(folder);
        if !path.is_dir() {
            fs::create_dir(&path).map_err(|e| PmError::os(&e))?;
        }
    }
    let monitor = camp.monitor()?;
    camp.save_monitor(&monitor)?;
    let mut cells = Vec::new();
    for c in camp.cells()? {
        cells.push(Value::Object(obj([
            ("cell", need(&c, "cell")?.clone()),
            ("group", need(&c, "group")?.clone()),
        ])));
    }
    camp.emit(
        "campaign-open",
        vec![
            ("cells", Value::Array(cells)),
            ("groups", list_str(&groups)),
            ("briefing", Value::Object(briefing.clone())),
            ("briefing_source", v(source)),
        ],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        (
            "campaign",
            camp.decl.get("id").cloned().unwrap_or(Value::Null),
        ),
        ("briefing", Value::Object(briefing)),
        ("briefing_source", v(source)),
    ]))
}

/// A command-line float (checked by the parser already).
pub fn parse_float(text: &str) -> Res<f64> {
    crate::py::py_float(text)
        .ok_or(())
        .map_err(|_| PmError::usage(format!("invalid float value: {}", repr_str(text))))
}

pub fn start_problems(camp: &mut Campaign, cell: &Obj, phase: &str) -> Res<Vec<String>> {
    let state = need(cell, "state")?;
    if !eq_str(Some(state), "open") {
        return Ok(vec![format!("cell is {}", py_str(state))]);
    }
    let phases = camp.phases();
    let name = py_str(need(cell, "cell")?);
    let index = phases.iter().position(|p| p == phase).ok_or_else(|| {
        PmError::Internal(format!("ValueError: {} is not in list", repr_str(phase)))
    })?;
    let mut problems = Vec::new();
    for earlier in &phases[..index] {
        if !eq_str(Some(&camp.phase_state(&name, earlier)?), "done") {
            problems.push(format!("{earlier} is not done"));
        }
    }
    if phases.last().map(String::as_str) == Some(phase) {
        let my_group = need(cell, "group")?.clone();
        for need_name in list_or_empty(cell.get("needs"))? {
            let i = camp.cell_index(&need_name)?;
            let other = camp.cell_at(i).clone();
            if eq_str(Some(need(&other, "state")?), "cancelled") {
                continue;
            }
            let need_text = py_str(&need_name);
            let other_group = need(&other, "group")?;
            if py_eq(other_group, &my_group) {
                if !eq_str(Some(&camp.phase_state(&need_text, phase)?), "done") {
                    problems.push(format!("needs {need_text}: its {phase} is not done"));
                }
            } else {
                let group = py_str(other_group);
                if !eq_str(Some(&camp.gate_state(&group)?), "passed") {
                    problems.push(format!("needs {need_text}: gate {group} is not passed"));
                }
            }
        }
    }
    Ok(problems)
}

pub fn cmd_phase_start(camp: &mut Campaign, args: &Args) -> Out {
    let (name, phase) = (args.pos(0), args.pos(1));
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    let mut data = camp.phase(name, phase)?;
    if !is_state(&data, "pending")? {
        return Err(PmError::refused(
            "phase-not-pending",
            vec![("state", state_of(&data)?)],
        ));
    }
    let problems = start_problems(camp, &cell, phase)?;
    if !problems.is_empty() {
        return Err(PmError::refused(
            "conditions-unmet",
            vec![("problems", list_str(&problems))],
        ));
    }
    data.insert("state".into(), v("running"));
    data.insert("started".into(), Value::from(stamp_now()));
    data.insert("ended".into(), Value::Null);
    camp.save_phase(name, phase, &data)?;
    let attempt = data["attempt"].clone();
    camp.emit(
        "phase-start",
        vec![
            ("cell", v(name)),
            ("phase", v(phase)),
            ("attempt", attempt.clone()),
        ],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("phase", v(phase)),
        ("state", v("running")),
        ("attempt", attempt),
    ]))
}

pub fn cmd_phase_retry(camp: &mut Campaign, args: &Args) -> Out {
    let (name, phase) = (args.pos(0), args.pos(1));
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    let mut data = camp.phase(name, phase)?;
    if !is_state(&cell, "open")? || !is_state(&data, "running")? {
        return Err(PmError::refused(
            "phase-not-running",
            vec![
                ("cell_state", state_of(&cell)?),
                ("state", state_of(&data)?),
            ],
        ));
    }
    if crate::py::ge_num(&data["retries"], camp.budget("stall_retries"))? {
        return Err(PmError::refused(
            "stall-retries-spent",
            vec![("retries", data["retries"].clone())],
        ));
    }
    let attempt = plus_one(&data["attempt"])?;
    let retries = plus_one(&data["retries"])?;
    data.insert("attempt".into(), attempt.clone());
    data.insert("retries".into(), retries);
    data.insert("started".into(), Value::from(stamp_now()));
    data.insert("ended".into(), Value::Null);
    camp.save_phase(name, phase, &data)?;
    camp.emit(
        "phase-retry",
        vec![
            ("cell", v(name)),
            ("phase", v(phase)),
            ("attempt", attempt.clone()),
        ],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("phase", v(phase)),
        ("state", v("running")),
        ("attempt", attempt),
    ]))
}

pub fn cmd_phase_close(camp: &mut Campaign, args: &Args) -> Out {
    let (name, phase) = (args.pos(0), args.pos(1));
    let (log, attempt, check, expect) = {
        let _guard = camp.lock(None)?;
        let cell = camp.cell(name)?;
        let data = camp.phase(name, phase)?;
        if !is_state(&cell, "open")? {
            return Err(PmError::refused(
                "cell-not-open",
                vec![("state", state_of(&cell)?)],
            ));
        }
        if !is_state(&data, "running")? && !is_state(&data, "failed")? {
            let hint = if is_state(&data, "pending")? {
                v("phase start first")
            } else {
                Value::Null
            };
            return Err(PmError::refused(
                "phase-not-running",
                vec![("state", state_of(&data)?), ("hint", hint)],
            ));
        }
        let proof = data["proof"].as_object().cloned().unwrap_or_default();
        let text = |key: &str| match proof.get(key) {
            Some(val) if truthy(val) => py_str(val),
            _ => String::new(),
        };
        let (check, expect) = (text("check"), text("expect"));
        if strip(&check).is_empty() || strip(&expect).is_empty() {
            return Err(PmError::usage(
                "proof.check and proof.expect must be filled",
            ));
        }
        let attempt = data["attempt"].clone();
        let a = py_str(&attempt);
        let mut index = 0;
        let mut log = format!("logs/{name}-{phase}-{a}.log");
        while camp.dir.join(crate::jsonio::os_text(&log)).exists() {
            index += 1;
            log = format!("logs/{name}-{phase}-{a}-{index}.log");
        }
        let logs = camp.dir.join("logs");
        if !logs.is_dir() {
            fs::create_dir(&logs).map_err(|e| PmError::os(&e))?;
        }
        fs::write(camp.dir.join(crate::jsonio::os_text(&log)), "").map_err(|e| PmError::os(&e))?;
        (log, attempt, check, expect)
    };
    let timeout = camp.budget("check_timeout_s").as_f64();
    let root = camp.root.clone();
    let done = execute(
        &check,
        &root,
        timeout,
        &camp.dir.join(crate::jsonio::os_text(&log)),
    )
    .map_err(|e| PmError::os(&e))?;
    let ok = done.error.is_none() && done.exit_code == Some(0) && done.output.contains(&expect);
    let state = {
        let _guard = camp.lock(None)?;
        let mut data = camp.phase(name, phase)?;
        if !py_eq(&data["attempt"], &attempt)
            || (!is_state(&data, "running")? && !is_state(&data, "failed")?)
        {
            return Err(PmError::refused(
                "phase-changed-meanwhile",
                vec![
                    ("state", state_of(&data)?),
                    ("attempt", data["attempt"].clone()),
                ],
            ));
        }
        data.insert("state".into(), v(if ok { "done" } else { "failed" }));
        data.insert("ended".into(), Value::from(done.ended.as_str()));
        if let Some(Value::Object(proof)) = data.get_mut("proof") {
            proof.insert("ok".into(), Value::Bool(ok));
            proof.insert("source".into(), v(&log));
        }
        let mut by = obj([
            ("tool", v(TOOL)),
            ("log", v(&log)),
            ("executor", v(&camp.executor)),
        ]);
        if let Some(err) = done.error {
            by.insert("error".into(), v(err));
        }
        data.insert("by".into(), Value::Object(by));
        camp.save_phase(name, phase, &data)?;
        camp.emit(
            "phase-close",
            vec![
                ("cell", v(name)),
                ("phase", v(phase)),
                ("ok", Value::Bool(ok)),
                ("log", v(&log)),
            ],
        )?;
        data["state"].clone()
    };
    let result = obj([
        ("ok", Value::Bool(ok)),
        ("cell", v(name)),
        ("phase", v(phase)),
        ("state", state),
        ("log", v(&log)),
        ("exit_code", done.exit_code.map_or(Value::Null, Value::from)),
        ("error", opt_v(done.error)),
    ]);
    if !ok {
        return Err(refuse_with("proof-failed".into(), &result));
    }
    Ok(result)
}

// --- gates --------------------------------------------------------------------------

fn check_force(
    camp: &mut Campaign,
    names: &[String],
    force: Option<&str>,
    reason: Option<&str>,
) -> Res<()> {
    let Some(force) = force else {
        return Ok(());
    };
    if !names.iter().any(|n| n == force) {
        return Err(PmError::usage(format!(
            "--force names {}, which this run does not include",
            repr_str(force)
        )));
    }
    if reason.is_none_or(|r| strip(r).is_empty()) {
        return Err(PmError::usage("--force needs --reason"));
    }
    let check = camp.check(force)?;
    let root = camp.root.clone();
    if !camp.fingerprint(&check, &root)?.reusable {
        return Ok(()); // rule 1 runs it anyway; that is not a forced rerun
    }
    if let Some(cap) = force_cap(camp, force)? {
        let ident = camp.open_hitl(
            "forced-rerun-cap",
            &format!("검사 {force}의 강제 재실행이 상한({cap})에 닿았다. 한 번 더 허용하는가?"),
            vec![("check", v(force)), ("cap", v(cap))],
        )?;
        return Err(PmError::refused(
            "forced-rerun-cap",
            vec![("check", v(force)), ("cap", v(cap)), ("hitl", ident)],
        ));
    }
    Ok(())
}

fn begin_run(gate: &mut Obj) -> Res<()> {
    let runs = plus_one(gate.get("runs").unwrap_or(&Value::from(0)))?;
    gate.insert("started".into(), Value::from(stamp_now()));
    gate.insert("ended".into(), Value::Null);
    gate.insert("pid".into(), Value::from(std::process::id()));
    gate.insert("runs".into(), runs);
    Ok(())
}

fn outcomes(results: &[Obj]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|r| {
                Value::Object(obj([
                    ("check", r["check"].clone()),
                    ("outcome", r["outcome"].clone()),
                ]))
            })
            .collect(),
    )
}

fn elapsed_since(gate: &Obj) -> (String, f64) {
    let ended = now();
    let started = gate
        .get("started")
        .and_then(Value::as_str)
        .and_then(parse_time)
        .unwrap_or(ended);
    (stamp(ended), seconds_between(started, ended))
}

pub fn cmd_gate_run(camp: &mut Campaign, args: &Args) -> Out {
    let group = args.pos(0);
    let force = args.opt("--force");
    let reason = args.opt("--reason");
    let (names, members) = {
        let _guard = camp.lock(None)?;
        let names = camp.group_checks(group)?;
        let mut gate = camp.gate(group)?;
        let state = py_str(need(&gate, "state")?);
        if state != "pending" && state != "failed" {
            let hint = if state == "passed" {
                "gate reopen first"
            } else {
                "hitl answer first"
            };
            return Err(PmError::refused(
                format!("gate-{state}"),
                vec![("hint", v(hint))],
            ));
        }
        if running_elsewhere(&gate) {
            return Err(PmError::refused(
                "gate-running",
                vec![("pid", gate.get("pid").cloned().unwrap_or(Value::Null))],
            ));
        }
        let problems = gate_blockers(camp, group)?;
        if !problems.is_empty() {
            return Err(PmError::refused(
                "members-not-done",
                vec![("problems", list_str(&problems))],
            ));
        }
        if names.is_empty() {
            return Err(PmError::refused("no-checks", vec![]));
        }
        check_force(camp, &names, force, reason)?;
        let members = member_attempts(camp, group)?;
        begin_run(&mut gate)?;
        camp.save_gate(group, &gate)?;
        (names, members)
    };
    let (results, hitl) = run_checks(
        camp,
        &names,
        "gate",
        force,
        reason,
        vec![("group", v(group))],
    )?;
    let (state, note, closed) = {
        let _guard = camp.lock(None)?;
        let mut gate = camp.gate(group)?;
        let mut state = gate_state_of(&results);
        let mut note = Value::Null;
        if !gate_blockers(camp, group)?.is_empty()
            || !py_eq(
                &Value::Object(member_attempts(camp, group)?),
                &Value::Object(members.clone()),
            )
        {
            state = "pending";
            note = v("members-changed");
        }
        let (ended, seconds) = elapsed_since(&gate);
        gate.insert("state".into(), v(state));
        gate.insert("ended".into(), Value::from(ended));
        gate.insert(
            "checks".into(),
            Value::Array(results.iter().cloned().map(Value::Object).collect()),
        );
        gate.insert(
            "hitl".into(),
            if state == "waiting" {
                hitl.clone()
            } else {
                Value::Null
            },
        );
        gate.shift_remove("pid");
        let mut closed = Vec::new();
        if state == "passed" {
            gate.insert(
                "members".into(),
                Value::Array(members.keys().map(|k| v(k)).collect()),
            );
            for i in camp.member_indices(group, true)? {
                let cell = camp.cell_at_mut(i);
                if eq_str(cell.get("review"), "none") {
                    cell.insert("state".into(), v("closed"));
                    closed.push(py_str(&cell["cell"]));
                }
            }
            camp.save_decl()?;
        }
        camp.save_gate(group, &gate)?;
        for name in &closed {
            camp.emit("cell-close", vec![("cell", v(name))])?;
        }
        camp.emit(
            "gate",
            vec![
                ("group", v(group)),
                ("state", v(state)),
                ("seconds", crate::py::float_value(seconds)),
                ("note", note.clone()),
                ("checks", outcomes(&results)),
            ],
        )?;
        (state, note, closed)
    };
    let out = obj([
        ("ok", Value::Bool(state == "passed")),
        ("group", v(group)),
        ("state", v(state)),
        (
            "checks",
            Value::Array(results.into_iter().map(Value::Object).collect()),
        ),
        ("closed", list_str(&closed)),
        ("hitl", hitl),
        ("note", note),
    ]);
    if state != "passed" {
        return Err(refuse_with(format!("gate-{state}"), &out));
    }
    Ok(out)
}

pub fn cmd_gate_reopen(camp: &mut Campaign, args: &Args) -> Out {
    let group = args.pos(0);
    let _guard = camp.lock(None)?;
    camp.group(group)?;
    let state = camp.gate_state(group)?;
    if !eq_str(Some(&state), "passed") {
        return Err(PmError::refused("gate-not-passed", vec![("state", state)]));
    }
    reopen_gate(camp, group, args.opt("--reason").unwrap_or(""), &["passed"])?;
    reset_final(camp)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("group", v(group)),
        ("state", v("pending")),
    ]))
}

pub fn final_settled(camp: &mut Campaign) -> Res<Vec<String>> {
    let mut out = Vec::new();
    for name in camp.group_names()? {
        let state = camp.group_state(&name)?;
        if ![
            crate::GROUP_PASSED,
            crate::GROUP_STOPPED,
            crate::GROUP_CANCELLED,
        ]
        .contains(&state)
        {
            out.push(format!("{name} is {state}"));
        }
    }
    Ok(out)
}

/// True when every result of a passed final run still stands for the present input.
fn final_current(camp: &mut Campaign, gate: &Obj) -> Res<bool> {
    let mut names: Vec<String> = Vec::new();
    for n in camp.final_names()? {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let entries = list_or_empty(gate.get("checks"))?;
    for name in names {
        let item = entries
            .iter()
            .rev()
            .find(|i| i.as_object().is_some_and(|o| eq_str(o.get("check"), &name)));
        let Some(Value::Object(item)) = item else {
            return Ok(false);
        };
        let pointer = match item.get("receipt") {
            Some(p) if truthy(p) => py_str(p),
            _ => return Ok(false),
        };
        let path = camp.dir.join(crate::jsonio::os_text(&pointer));
        if !path.is_file() {
            return Ok(false);
        }
        let receipt = read_json(&path, None)?;
        let receipt = as_obj(&receipt)?.clone();
        let check = camp.check(&name)?;
        let root = camp.root.clone();
        let fp = camp.fingerprint(&check, &root)?;
        if !(crate::py::opt_truthy(receipt.get("reusable"))
            && fp.reusable
            && eq_str(receipt.get("key_sha256"), &fp.sha)
            && eq_str(receipt.get("result"), "PASS")
            && camp.usable(&receipt, None)?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn cmd_final_run(camp: &mut Campaign, args: &Args) -> Out {
    let force = args.opt("--force");
    let reason = args.opt("--reason");
    let names = {
        let _guard = camp.lock(None)?;
        let names = camp.final_names()?;
        let mut gate = camp.gate(FINAL)?;
        if is_state(&gate, "waiting")? {
            return Err(PmError::refused(
                "final-waiting",
                vec![("hint", v("hitl answer first"))],
            ));
        }
        if running_elsewhere(&gate) {
            return Err(PmError::refused(
                "final-running",
                vec![("pid", gate.get("pid").cloned().unwrap_or(Value::Null))],
            ));
        }
        let unsettled = final_settled(camp)?;
        if !unsettled.is_empty() {
            return Err(PmError::refused(
                "groups-not-settled",
                vec![("problems", list_str(&unsettled))],
            ));
        }
        if names.is_empty() {
            return Err(PmError::refused("no-checks", vec![]));
        }
        if is_state(&gate, "passed")? && force.is_none() && final_current(camp, &gate)? {
            return Ok(obj([
                ("ok", Value::Bool(true)),
                ("state", v("passed")),
                ("note", v("already-passed")),
                ("checks", gate.get("checks").cloned().unwrap_or(Value::Null)),
            ]));
        }
        check_force(camp, &names, force, reason)?;
        begin_run(&mut gate)?;
        camp.save_gate(FINAL, &gate)?;
        names
    };
    let (results, hitl) = run_checks(
        camp,
        &names,
        "final",
        force,
        reason,
        vec![("final", Value::Bool(true))],
    )?;
    let (state, note) = {
        let _guard = camp.lock(None)?;
        let mut gate = camp.gate(FINAL)?;
        let mut state = gate_state_of(&results);
        let mut note = Value::Null;
        if !final_settled(camp)?.is_empty() {
            state = "pending";
            note = v("groups-changed");
        }
        let (ended, seconds) = elapsed_since(&gate);
        gate.insert("state".into(), v(state));
        gate.insert("ended".into(), Value::from(ended));
        gate.insert(
            "checks".into(),
            Value::Array(results.iter().cloned().map(Value::Object).collect()),
        );
        gate.insert(
            "hitl".into(),
            if state == "waiting" {
                hitl.clone()
            } else {
                Value::Null
            },
        );
        gate.shift_remove("pid");
        gate.shift_remove("members");
        camp.save_gate(FINAL, &gate)?;
        camp.emit(
            "final",
            vec![
                ("state", v(state)),
                ("seconds", crate::py::float_value(seconds)),
                ("note", note.clone()),
                ("checks", outcomes(&results)),
            ],
        )?;
        (state, note)
    };
    let out = obj([
        ("ok", Value::Bool(state == "passed")),
        ("state", v(state)),
        (
            "checks",
            Value::Array(results.into_iter().map(Value::Object).collect()),
        ),
        ("hitl", hitl),
        ("note", note),
    ]);
    if state != "passed" {
        return Err(refuse_with(format!("final-{state}"), &out));
    }
    Ok(out)
}

pub fn cmd_check_invalidate(camp: &mut Campaign, args: &Args) -> Out {
    let check = args.pos(0);
    let reason = args.opt("--reason").unwrap_or("");
    let _guard = camp.lock(None)?;
    camp.check(check)?;
    let event = camp.emit(
        "invalidate",
        vec![("check", v(check)), ("reason", v(reason))],
    )?;
    let utc = event["utc"].clone();
    write_json(
        &camp
            .dir
            .join("receipts")
            .join(crate::jsonio::os_text(check))
            .join("invalidated.json"),
        &Value::Object(obj([("utc", utc.clone()), ("reason", v(reason))])),
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("check", v(check)),
        ("utc", utc),
    ]))
}

// --- exploration ----------------------------------------------------------------------

/// Every exploration by id, with whether it is still open.
fn explorations(camp: &mut Campaign) -> Res<Obj> {
    let events = camp.events()?.clone();
    let mut found = Obj::new();
    for e in events
        .iter()
        .filter(|e| eq_str(e.get("event"), "explore-open"))
    {
        let Some(id) = e.get("id").filter(|i| truthy(i)) else {
            continue;
        };
        let mut item = e.clone();
        item.insert("open".into(), Value::Bool(true));
        found.insert(py_str(id), Value::Object(item));
    }
    for e in events
        .iter()
        .filter(|e| eq_str(e.get("event"), "direction"))
    {
        if let Some(Value::String(id)) = e.get("id") {
            if let Some(Value::Object(item)) = found.get_mut(id) {
                item.insert("open".into(), Value::Bool(false));
            }
        }
    }
    Ok(found)
}

fn open_exploration(camp: &mut Campaign, id: &str) -> Res<Option<Obj>> {
    Ok(match explorations(camp)?.get(id) {
        Some(Value::Object(item)) if crate::py::opt_truthy(item.get("open")) => Some(item.clone()),
        _ => None,
    })
}

pub fn cmd_explore_open(camp: &mut Campaign, args: &Args) -> Out {
    let cell_name = args.opt("--cell").unwrap_or("");
    let reason = args.opt("--reason").unwrap_or("");
    let _guard = camp.lock(None)?;
    let cell = camp.cell(cell_name)?;
    if !is_state(&cell, "open")? {
        return Err(PmError::refused(
            "cell-not-open",
            vec![("state", state_of(&cell)?)],
        ));
    }
    let known = explorations(camp)?.len() as i64;
    if Num::Int(i128::from(known)).ge(camp.budget("explore_rounds")) {
        return Err(PmError::refused(
            "explore-rounds-spent",
            vec![("rounds", Value::from(known))],
        ));
    }
    let ident = format!("x{}", known + 1);
    camp.emit(
        "explore-open",
        vec![
            ("id", v(&ident)),
            ("cell", v(cell_name)),
            ("reason", v(reason)),
        ],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("id", v(&ident)),
        ("cell", v(cell_name)),
    ]))
}

pub fn cmd_check_run(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let explore = args.opt("--explore").unwrap_or("");
    let target = crate::paths::resolve(&crate::paths::normalise(&crate::jsonio::os_text(
        args.opt("--target").unwrap_or(""),
    )))
    .map_err(|e| PmError::Internal(format!("RuntimeError: {e}")))?;
    let (check, plan, skipped) = {
        let _guard = camp.lock(None)?;
        let check = camp.check(name)?;
        let Some(found) = open_exploration(camp, explore)? else {
            return Err(PmError::refused(
                "no-open-exploration",
                vec![("explore", v(explore))],
            ));
        };
        let cell = camp.cell(&py_str(need(&found, "cell")?))?;
        let group = py_str(need(&cell, "group")?);
        if !camp.group_checks(&group)?.iter().any(|c| c == name) {
            return Err(PmError::refused(
                "check-not-in-group",
                vec![("group", v(&group))],
            ));
        }
        if !target.is_dir() {
            return Err(PmError::usage(format!(
                "{}: not a directory",
                target.display()
            )));
        }
        let base = crate::paths::resolve(&camp.root).unwrap_or_else(|_| camp.root.clone());
        let mut seen: Vec<String> = Vec::new();
        for r in camp.all_receipts()? {
            if eq_str(r.get("explore"), explore) {
                let t = r.get("target").map_or("None".to_string(), py_str);
                if !seen.contains(&t) {
                    seen.push(t);
                }
            }
        }
        seen.retain(|t| *t != crate::jsonio::path_text(&base));
        let target_text = crate::jsonio::path_text(&target);
        if target != base
            && !seen.contains(&target_text)
            && Num::Int(seen.len() as i128).ge(camp.budget("explore_candidates"))
        {
            seen.sort();
            return Err(PmError::refused(
                "explore-candidates-spent",
                vec![("candidates", list_str(&seen))],
            ));
        }
        let mut plan = decide(camp, &check, &target, "explore", false)?;
        let mut skipped = None;
        if plan.action == Action::Run {
            plan.covers = covered_inputs(camp, &check, &target)?;
        } else {
            skipped = Some(skip_check(camp, &check, "explore", &plan)?);
        }
        (check, plan, skipped)
    };
    let result = match skipped {
        Some(r) => r,
        None => run_check(camp, &check, &target, "explore", &plan, None, Some(explore))?,
    };
    let _ = check_id(&check)?;
    let outcome = py_str(&result["outcome"]);
    let ok = outcome == "reused" || outcome == "covered" || eq_str(result.get("result"), "PASS");
    let mut out = obj([
        ("ok", Value::Bool(ok)),
        ("explore", v(explore)),
        ("target", Value::from(crate::jsonio::path_text(&target))),
    ]);
    for (k, val) in result {
        out.insert(k, val);
    }
    if !ok {
        return Err(refuse_with(format!("check-{outcome}"), &out));
    }
    Ok(out)
}

pub fn cmd_explore_close(camp: &mut Campaign, args: &Args) -> Out {
    let id = args.pos(0);
    let selected = args.opt("--selected").unwrap_or("");
    let reference = args.opt("--ref").unwrap_or("");
    let _guard = camp.lock(None)?;
    let Some(found) = open_exploration(camp, id)? else {
        return Err(PmError::refused(
            "no-open-exploration",
            vec![("explore", v(id))],
        ));
    };
    camp.emit(
        "direction",
        vec![
            ("id", v(id)),
            ("cell", need(&found, "cell")?.clone()),
            ("selected", v(selected)),
            ("ref", v(reference)),
            ("note", opt_v(args.opt("--note"))),
        ],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("id", v(id)),
        ("selected", v(selected)),
        ("ref", v(reference)),
    ]))
}

// --- cells --------------------------------------------------------------------------

/// The latest review round of a cell: `max` of `int(review_base or 0)` and the
/// int (or bool) rounds of its review events, the value itself as the baseline kept it.
pub fn review_round(camp: &mut Campaign, cell: &Obj) -> Res<Value> {
    let name = need(cell, "cell")?.clone();
    let base = match cell.get("review_base") {
        Some(b) if truthy(b) => crate::py::py_int(b)?.to_value(),
        _ => Value::from(0),
    };
    let mut rounds = vec![base];
    for e in camp.events()?.iter() {
        if eq_str(e.get("event"), "review") && e.get("cell").is_some_and(|c| py_eq(c, &name)) {
            match e.get("round") {
                None => rounds.push(Value::from(0)),
                Some(r @ Value::Bool(_)) => rounds.push(r.clone()),
                Some(r) if is_int(r) => rounds.push(r.clone()),
                _ => {}
            }
        }
    }
    Ok(crate::py::py_max(&rounds)?.unwrap_or(Value::from(0)))
}

fn ensure_group(
    camp: &mut Campaign,
    group: &str,
    checks: Option<Vec<String>>,
) -> Res<Option<Vec<String>>> {
    if camp.has_group(group)? {
        return Ok(None);
    }
    if !is_plain_name(group) {
        return Err(PmError::usage(format!(
            "group name {} is not a plain file name",
            repr_str(group)
        )));
    }
    let checks = match checks {
        Some(c) if !c.is_empty() => c,
        _ => {
            return Err(PmError::usage(format!(
                "group {} is not declared; give its checks",
                repr_str(group)
            )))
        }
    };
    for name in &checks {
        camp.check(name)?;
    }
    if let Some(Value::Array(groups)) = camp.decl.get_mut("groups") {
        groups.push(Value::Object(obj([
            ("group", v(group)),
            ("checks", list_str(&checks)),
        ])));
    }
    Ok(Some(checks))
}

pub fn clear_stalls(
    camp: &Campaign,
    cell: Option<&str>,
    group: Option<&str>,
    final_: bool,
) -> Res<()> {
    let mut monitor = camp.monitor()?;
    let stalls = match need(&monitor, "stalls")? {
        Value::Object(m) => m.clone(),
        _ => {
            return Err(PmError::Internal(
                "AttributeError: 'stalls' has no attribute 'items'".into(),
            ))
        }
    };
    let mut keep = Obj::new();
    for (key, count) in &stalls {
        let mut parts: Vec<&str> = key.split('|').collect();
        parts.extend(["", "", "", ""]);
        let (kind, key_cell, key_group) = (parts[0], parts[1], parts[3]);
        if cell.is_some_and(|c| !c.is_empty() && key_cell == c)
            || group.is_some_and(|g| !g.is_empty() && key_group == g)
            || (final_ && kind == "final")
        {
            continue;
        }
        keep.insert(key.clone(), count.clone());
    }
    if keep.len() != stalls.len() {
        monitor.insert("stalls".into(), Value::Object(keep));
        camp.save_monitor(&monitor)?;
    }
    Ok(())
}

fn nonempty_str(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(s)) if !strip(s).is_empty())
}

pub fn cmd_cell_add(camp: &mut Campaign, args: &Args) -> Out {
    let spec = read_json(
        &crate::paths::normalise(&crate::jsonio::os_text(args.pos(0))),
        None,
    )?;
    let Value::Object(spec) = spec else {
        return Err(PmError::usage("the spec must be a JSON object"));
    };
    let name = match spec.get("cell") {
        Some(Value::String(s)) if is_plain_name(s) && !s.starts_with('_') => s.clone(),
        _ => return Err(PmError::usage("spec.cell must be a plain folder name")),
    };
    for key in ["goal", "group", "reason"] {
        if !nonempty_str(spec.get(key)) {
            return Err(PmError::usage(format!(
                "spec.{key} must be a non-empty string"
            )));
        }
    }
    let goal = py_str(&spec["goal"]);
    let group = py_str(&spec["group"]);
    let _guard = camp.lock(None)?;
    let mut exists = false;
    for c in camp.decl["cells"].as_array().cloned().unwrap_or_default() {
        if eq_str(Some(need(as_obj(&c)?, "cell")?), &name) {
            exists = true;
        }
    }
    if exists {
        return Err(PmError::refused("cell-exists", vec![("cell", v(&name))]));
    }
    let phases = camp.phases();
    for phase in &phases {
        let ok = match spec.get(phase.as_str()) {
            Some(Value::Object(p)) => nonempty_str(p.get("check")) && nonempty_str(p.get("expect")),
            _ => false,
        };
        if !ok {
            return Err(PmError::usage(format!(
                "spec.{phase} needs check and expect"
            )));
        }
    }
    let parent = spec.get("parent").filter(|p| truthy(p)).cloned();
    let needs = list_or_empty(spec.get("needs"))?;
    for other in parent.iter().chain(needs.iter()) {
        camp.cell_index(other)?;
    }
    let autos = camp
        .cells()?
        .iter()
        .filter(|c| eq_str(c.get("origin"), "auto"))
        .count() as i128;
    let allowed = camp.allows("auto-cell-cap", vec![])?;
    if Num::Int(autos).ge(camp.budget("auto_cells").plus(allowed)) {
        let ident = camp.open_hitl(
            "auto-cell-cap",
            "스스로 만드는 cell이 상한에 닿았다. 하나 더 허용하는가?",
            vec![],
        )?;
        return Err(PmError::refused(
            "auto-cell-cap",
            vec![("autos", Num::Int(autos).to_value()), ("hitl", ident)],
        ));
    }
    let group_checks = match spec.get("group_checks") {
        Some(val) if truthy(val) => Some(list_or_empty(Some(val))?.iter().map(py_str).collect()),
        _ => None,
    };
    let created = ensure_group(camp, &group, group_checks)?;
    let (mut review, mut base) = (v("none"), Value::from(0));
    if let Some(parent) = &parent {
        let i = camp.cell_index(parent)?;
        let parent_cell = camp.cell_at(i).clone();
        review = need(&parent_cell, "review")?.clone();
        base = review_round(camp, &parent_cell)?;
    }
    let folder = camp.dir.join("cells").join(crate::jsonio::os_text(&name));
    fs::create_dir_all(folder.join("phases")).map_err(|e| PmError::os(&e))?;
    let mut body = vec![
        format!("# {name}"),
        String::new(),
        "이 파일만 읽고 이 cell을 이어갈 수 있게 쓴다. 항목이 비면 \"없음\"이라고 적는다."
            .to_string(),
        String::new(),
    ];
    for section in SECTIONS {
        body.push(format!("## {section}"));
        body.push(String::new());
        body.push(if section == "목표" {
            goal.clone()
        } else {
            "없음".to_string()
        });
        body.push(String::new());
    }
    let body = match crate::jsonio::encode(&body.join("\n")) {
        Ok(body) => body,
        Err(err) => {
            // the baseline opened (emptied) the file before the encoding failed
            fs::write(folder.join("cell.md"), "").map_err(|e| PmError::os(&e))?;
            return Err(err);
        }
    };
    fs::write(folder.join("cell.md"), body).map_err(|e| PmError::os(&e))?;
    for phase in &phases {
        let proof = spec[phase.as_str()]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let data = obj([
            ("schema_version", Value::from(SCHEMA)),
            ("cell", v(&name)),
            ("phase", v(phase)),
            ("state", v("pending")),
            ("attempt", Value::from(1)),
            ("started", Value::Null),
            ("ended", Value::Null),
            (
                "proof",
                Value::Object(obj([
                    ("check", proof["check"].clone()),
                    ("expect", proof["expect"].clone()),
                    ("ok", Value::Bool(false)),
                    ("source", Value::Null),
                ])),
            ),
            ("by", Value::Null),
        ]);
        camp.save_phase(&name, phase, &data)?;
    }
    let parent_v = parent
        .clone()
        .unwrap_or_else(|| spec.get("parent").cloned().unwrap_or(Value::Null));
    let mut entry = obj([
        ("cell", v(&name)),
        ("goal", v(&goal)),
        ("group", v(&group)),
        ("needs", Value::Array(needs.clone())),
        ("review", review.clone()),
        ("origin", v("auto")),
        ("parent", parent_v.clone()),
        ("state", v("open")),
    ]);
    if truthy(&base) {
        entry.insert("review_base".into(), base);
    }
    if let Some(Value::Array(cells)) = camp.decl.get_mut("cells") {
        cells.push(Value::Object(entry));
    }
    camp.save_decl()?;
    let mut fields = vec![
        ("cell", v(&name)),
        ("group", v(&group)),
        ("parent", parent_v),
        ("review", review.clone()),
        ("reason", spec["reason"].clone()),
    ];
    if let Some(c) = &created {
        fields.push(("group_checks", list_str(c)));
    }
    camp.emit("cell-add", fields)?;
    let reopened = reopen_gate(camp, &group, &format!("member added: {name}"), &["passed"])?;
    reset_final(camp)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(&name)),
        ("group", v(&group)),
        ("review", review),
        ("group_created", Value::Bool(created.is_some())),
        ("gate_reopened", Value::Bool(reopened)),
    ]))
}

fn set_cell_state(camp: &mut Campaign, name: &str, state: &str) -> Res<usize> {
    let i = camp.cell_index(&v(name))?;
    camp.cell_at_mut(i).insert("state".into(), v(state));
    Ok(i)
}

pub fn cmd_cell_cancel(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    if !is_state(&cell, "open")? {
        return Err(PmError::refused(
            "cell-not-open",
            vec![("state", state_of(&cell)?)],
        ));
    }
    set_cell_state(camp, name, "cancelled")?;
    camp.save_decl()?;
    camp.emit(
        "cell-cancel",
        vec![("cell", v(name)), ("reason", opt_v(args.opt("--reason")))],
    )?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("state", v("cancelled")),
    ]))
}

pub fn cmd_cell_move(camp: &mut Campaign, args: &Args) -> Out {
    let (name, group) = (args.pos(0), args.pos(1));
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    if !is_state(&cell, "open")? {
        let hint = if is_state(&cell, "closed")? {
            v("gate reopen first")
        } else {
            Value::Null
        };
        return Err(PmError::refused(
            "cell-not-open",
            vec![("state", state_of(&cell)?), ("hint", hint)],
        ));
    }
    let source = need(&cell, "group")?.clone();
    if eq_str(Some(&source), group) {
        return Err(PmError::refused("same-group", vec![]));
    }
    let checks = args
        .opt("--checks")
        .filter(|c| !c.is_empty())
        .map(|c| c.split(',').map(str::to_string).collect());
    let created = ensure_group(camp, group, checks)?;
    let i = camp.cell_index(&v(name))?;
    camp.cell_at_mut(i).insert("group".into(), v(group));
    camp.save_decl()?;
    let mut fields = vec![
        ("cell", v(name)),
        ("from", source.clone()),
        ("to", v(group)),
        ("reason", opt_v(args.opt("--reason"))),
    ];
    if let Some(c) = &created {
        fields.push(("group_checks", list_str(c)));
    }
    camp.emit("regroup", fields)?;
    let mut reopened = Vec::new();
    for g in [py_str(&source), group.to_string()] {
        if reopen_gate(camp, &g, &format!("member moved: {name}"), &["passed"])? {
            reopened.push(g);
        }
    }
    reset_final(camp)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("from", source),
        ("to", v(group)),
        ("gates_reopened", list_str(&reopened)),
    ]))
}

pub fn cmd_cell_reopen(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    let last = camp.last_phase()?;
    let mut data = camp.phase(name, &last)?;
    if !is_state(&cell, "open")? && !is_state(&cell, "closed")? {
        return Err(PmError::refused(
            "cell-not-reopenable",
            vec![("state", state_of(&cell)?)],
        ));
    }
    if !is_state(&data, "done")? {
        return Err(PmError::refused(
            "phase-not-done",
            vec![("phase", v(&last)), ("state", state_of(&data)?)],
        ));
    }
    let attempt = plus_one(&data["attempt"])?;
    data.insert("state".into(), v("pending"));
    data.insert("attempt".into(), attempt.clone());
    data.insert("retries".into(), Value::from(0));
    data.insert("started".into(), Value::Null);
    data.insert("ended".into(), Value::Null);
    data.insert("by".into(), Value::Null);
    if let Some(Value::Object(proof)) = data.get_mut("proof") {
        proof.insert("ok".into(), Value::Bool(false));
        proof.insert("source".into(), Value::Null);
    }
    camp.save_phase(name, &last, &data)?;
    set_cell_state(camp, name, "open")?;
    camp.save_decl()?;
    camp.emit(
        "cell-reopen",
        vec![("cell", v(name)), ("reason", opt_v(args.opt("--reason")))],
    )?;
    let group = py_str(need(&cell, "group")?);
    let reopened = reopen_gate(
        camp,
        &group,
        &format!("cell reopened: {name}"),
        &["passed", "failed"],
    )?;
    reset_final(camp)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("phase", v(&last)),
        ("attempt", attempt),
        ("gate_reopened", Value::Bool(reopened)),
    ]))
}

pub fn cmd_cell_stop(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    if !is_state(&cell, "open")? {
        return Err(PmError::refused(
            "cell-not-open",
            vec![("state", state_of(&cell)?)],
        ));
    }
    set_cell_state(camp, name, "stopped")?;
    camp.save_decl()?;
    camp.emit(
        "cell-stop",
        vec![("cell", v(name)), ("reason", opt_v(args.opt("--reason")))],
    )?;
    let group = py_str(need(&cell, "group")?);
    let group_state = camp.group_state(&group)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("state", v("stopped")),
        ("group", cell["group"].clone()),
        ("group_state", v(group_state)),
    ]))
}

pub fn cmd_cell_resume(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let reference = args.opt("--ref").unwrap_or("");
    if strip(reference).is_empty() {
        return Err(PmError::usage(
            "cell resume needs --ref (where the user's approval is recorded)",
        ));
    }
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    if !is_state(&cell, "stopped")? {
        return Err(PmError::refused(
            "cell-not-stopped",
            vec![("state", state_of(&cell)?)],
        ));
    }
    set_cell_state(camp, name, "open")?;
    camp.save_decl()?;
    for phase in camp.phases() {
        let mut data = camp.phase(name, &phase)?;
        if truthy(&data["retries"]) {
            data.insert("retries".into(), Value::from(0));
            camp.save_phase(name, &phase, &data)?;
        }
    }
    camp.emit(
        "cell-resume",
        vec![("cell", v(name)), ("ref", v(reference))],
    )?;
    clear_stalls(camp, Some(name), None, false)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("state", v("open")),
    ]))
}

pub fn cmd_cell_close(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    let group = py_str(need(&cell, "group")?);
    let mut gate = camp.gate(&group)?;
    if eq_str(Some(need(&cell, "review")?), "none") {
        return Err(PmError::refused(
            "cell-not-marked",
            vec![("hint", v("an unmarked cell closes when its gate passes"))],
        ));
    }
    if !is_state(&cell, "open")? || !is_state(&gate, "passed")? {
        return Err(PmError::refused(
            "conditions-unmet",
            vec![
                ("cell_state", state_of(&cell)?),
                ("gate_state", state_of(&gate)?),
            ],
        ));
    }
    let passed_at = gate
        .get("ended")
        .and_then(Value::as_str)
        .and_then(parse_time);
    let reviews: Vec<Obj> = camp
        .events()?
        .iter()
        .filter(|e| eq_str(e.get("event"), "review") && eq_str(e.get("cell"), name))
        .cloned()
        .collect();
    let latest = reviews.last();
    let when = latest
        .and_then(|l| l.get("utc"))
        .and_then(Value::as_str)
        .and_then(parse_time);
    let clean = latest.is_some_and(|l| {
        l.get("ran") == Some(&Value::Bool(true)) && l.get("clean") == Some(&Value::Bool(true))
    });
    let after = match (passed_at, when) {
        (Some(p), Some(w)) => w > p,
        _ => false,
    };
    if latest.is_none() || !after || !clean {
        let round = latest.map_or(Value::Null, |l| {
            l.get("round").cloned().unwrap_or(Value::Null)
        });
        return Err(PmError::refused(
            "no-clean-review-after-gate",
            vec![("latest_round", round)],
        ));
    }
    set_cell_state(camp, name, "closed")?;
    let members = match gate.get_mut("members") {
        Some(Value::Array(m)) => m,
        _ => return Err(PmError::Internal("KeyError: 'members'".into())),
    };
    if !crate::py::contains(members, &v(name)) {
        members.push(v(name));
        camp.save_gate(&group, &gate)?;
    }
    camp.save_decl()?;
    camp.emit("cell-close", vec![("cell", v(name))])?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("state", v("closed")),
    ]))
}

pub fn cmd_review_record(camp: &mut Campaign, args: &Args) -> Out {
    let name = args.pos(0);
    let review = read_json(
        &crate::paths::normalise(&crate::jsonio::os_text(args.pos(1))),
        None,
    )?;
    let Value::Object(review) = review else {
        return Err(PmError::usage("the review must be a JSON object"));
    };
    for (key, kind) in [
        ("round", "int"),
        ("ran", "bool"),
        ("blocking", "list"),
        ("non_blocking", "list"),
        ("clean", "bool"),
    ] {
        let ok = match (kind, review.get(key)) {
            ("int", Some(val)) => is_int(val),
            ("bool", Some(Value::Bool(_))) => true,
            ("list", Some(Value::Array(_))) => true,
            _ => false,
        };
        if !ok {
            return Err(PmError::usage(format!("review.{key} must be {kind}")));
        }
    }
    let _guard = camp.lock(None)?;
    let cell = camp.cell(name)?;
    if !is_state(&cell, "open")? {
        return Err(PmError::refused(
            "cell-not-open",
            vec![("state", state_of(&cell)?)],
        ));
    }
    let expected = plus_one(&review_round(camp, &cell)?)?;
    if !py_eq(&review["round"], &expected) {
        return Err(PmError::refused(
            "round-out-of-order",
            vec![("expected", expected)],
        ));
    }
    let cap = match camp.decl.get("review") {
        Some(Value::Object(r)) => r.get("round_cap").cloned().unwrap_or(Value::from(3)),
        Some(other) if truthy(other) => {
            return Err(PmError::Internal(
                "AttributeError: 'review' has no attribute 'get'".into(),
            ))
        }
        _ => Value::from(3),
    };
    let cap = match Num::of(&cap) {
        Some(c) if is_int(&cap) && c.ge(Num::Int(1)) => c,
        _ => {
            return Err(PmError::usage(
                "campaign.json: review.round_cap must be a positive integer",
            ))
        }
    };
    camp.emit(
        "review",
        vec![
            ("cell", v(name)),
            ("round", review["round"].clone()),
            ("ran", review["ran"].clone()),
            ("blocking", review["blocking"].clone()),
            ("non_blocking", review["non_blocking"].clone()),
            ("clean", review["clean"].clone()),
        ],
    )?;
    let mut stopped = false;
    // round equals the expected int here (checked above), so it is a number
    let round_reached = crate::py::Num::of(&review["round"]).is_some_and(|r| r.ge(cap));
    if round_reached && truthy(&review["blocking"]) && !truthy(&review["clean"]) {
        set_cell_state(camp, name, "stopped")?;
        camp.save_decl()?;
        camp.emit(
            "cell-stop",
            vec![("cell", v(name)), ("reason", v("review-cap"))],
        )?;
        stopped = true;
    }
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("cell", v(name)),
        ("round", review["round"].clone()),
        ("stopped", Value::Bool(stopped)),
    ]))
}

// --- handing over to a person -----------------------------------------------------------

pub fn cmd_hitl_open(camp: &mut Campaign, args: &Args) -> Out {
    let kind = args.opt("--kind").unwrap_or("");
    let question = args.opt("--question").unwrap_or("");
    let _guard = camp.lock(None)?;
    if let Some(cell_name) = args.opt("--cell").filter(|c| !c.is_empty()) {
        let cell = camp.cell(cell_name)?;
        if !is_state(&cell, "open")? {
            return Err(PmError::refused(
                "cell-not-open",
                vec![("state", state_of(&cell)?)],
            ));
        }
        let ident = camp.open_hitl(kind, question, vec![("cell", v(cell_name))])?;
        set_cell_state(camp, cell_name, "waiting")?;
        camp.save_decl()?;
        return Ok(obj([
            ("ok", Value::Bool(true)),
            ("id", ident),
            ("cell", v(cell_name)),
            ("state", v("waiting")),
        ]));
    }
    let final_ = args.flag("--final");
    let group = if final_ {
        FINAL.to_string()
    } else {
        args.opt("--group").unwrap_or("").to_string()
    };
    if !final_ {
        camp.group(&group)?;
    }
    let mut gate = camp.gate(&group)?;
    if running_elsewhere(&gate) {
        return Err(PmError::refused(
            "gate-running",
            vec![("pid", gate.get("pid").cloned().unwrap_or(Value::Null))],
        ));
    }
    let state = py_str(need(&gate, "state")?);
    let allowed: &[&str] = if final_ {
        &["pending", "failed", "passed"]
    } else {
        &["pending", "failed"]
    };
    if !allowed.contains(&state.as_str()) {
        let hint = if state == "passed" {
            v("use --cell for a passed gate")
        } else {
            Value::Null
        };
        return Err(PmError::refused(
            format!("gate-{state}"),
            vec![("hint", hint)],
        ));
    }
    let target: Vec<(&str, Value)> = if final_ {
        vec![("final", Value::Bool(true))]
    } else {
        vec![("group", v(&group))]
    };
    let ident = camp.open_hitl(kind, question, target.clone())?;
    gate.insert("state".into(), v("waiting"));
    gate.insert("hitl".into(), ident.clone());
    camp.save_gate(&group, &gate)?;
    let mut out = obj([("ok", Value::Bool(true)), ("id", ident)]);
    for (k, val) in target {
        out.insert(k.to_string(), val);
    }
    out.insert("state".into(), v("waiting"));
    Ok(out)
}

pub fn cmd_hitl_answer(camp: &mut Campaign, args: &Args) -> Out {
    let id = args.pos(0);
    let reference = args.opt("--ref").unwrap_or("");
    if strip(reference).is_empty() {
        return Err(PmError::usage(
            "hitl answer needs --ref (where the user's answer is recorded)",
        ));
    }
    let allow = args.flag("--allow");
    let _guard = camp.lock(None)?;
    let item = camp.hitls()?.into_iter().find(|h| eq_str(h.get("id"), id));
    let Some(item) = item else {
        return Err(PmError::usage(format!("unknown hitl id {}", repr_str(id))));
    };
    if !item["answer"].is_null() {
        return Err(PmError::refused("already-answered", vec![]));
    }
    camp.emit(
        "hitl-answer",
        vec![
            ("id", v(id)),
            ("ref", v(reference)),
            ("allow", Value::Bool(allow)),
        ],
    )?;
    let mut released = Obj::new();
    if crate::py::opt_truthy(item.get("group")) {
        let group = py_str(&item["group"]);
        let mut gate = camp.gate(&group)?;
        if is_state(&gate, "waiting")? {
            gate.insert("state".into(), v("pending"));
            gate.insert("hitl".into(), Value::Null);
            camp.save_gate(&group, &gate)?;
            released.insert("group".into(), item["group"].clone());
        }
        clear_stalls(camp, None, Some(&group), false)?;
    } else if crate::py::opt_truthy(item.get("final")) {
        let mut gate = camp.gate(FINAL)?;
        if is_state(&gate, "waiting")? {
            gate.insert("state".into(), v("pending"));
            gate.insert("hitl".into(), Value::Null);
            camp.save_gate(FINAL, &gate)?;
            released.insert("final".into(), Value::Bool(true));
        }
        clear_stalls(camp, None, None, true)?;
    } else if crate::py::opt_truthy(item.get("cell")) {
        let i = camp.cell_index(&item["cell"])?;
        if eq_str(camp.cell_at(i).get("state"), "waiting") {
            camp.cell_at_mut(i).insert("state".into(), v("open"));
            camp.save_decl()?;
            released.insert("cell".into(), item["cell"].clone());
        }
        clear_stalls(camp, Some(&py_str(&item["cell"])), None, false)?;
    }
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("id", v(id)),
        ("allow", Value::Bool(allow)),
        ("released", Value::Object(released)),
    ]))
}

pub fn cmd_event(camp: &mut Campaign, args: &Args) -> Out {
    // the argument is held text; the decoder reads its real characters
    let data = crate::jsonio::parse(&crate::jsonio::os_text(args.pos(0))).map_err(|e| match e {
        crate::jsonio::JsonError::Decode(text) => {
            PmError::usage(format!("the event is not JSON ({text})"))
        }
        crate::jsonio::JsonError::Other(text) => PmError::Internal(text),
    })?;
    let mut data = match data {
        Value::Object(m) if nonempty_str(m.get("event")) => m,
        _ => return Err(PmError::usage("the event needs an event name")),
    };
    let name = py_str(&data["event"]);
    if RESERVED_EVENTS.contains(&name.as_str()) {
        return Err(PmError::usage(format!(
            "the executor writes {} itself",
            repr_str(&name)
        )));
    }
    data.shift_remove("utc");
    data.shift_remove("event");
    for reserved in ["name", "self"] {
        if data.contains_key(reserved) {
            return Err(PmError::Internal(format!(
                "TypeError: emit() got multiple values for argument {}",
                repr_str(reserved)
            )));
        }
    }
    let _guard = camp.lock(None)?;
    let fields: Vec<(&str, Value)> = data
        .iter()
        .map(|(k, val)| (k.as_str(), val.clone()))
        .collect();
    let event = camp.emit(&name, fields)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("event", Value::Object(event)),
    ]))
}

pub fn cmd_brief(camp: &mut Campaign, args: &Args) -> Out {
    if let Some(done) = args.opt("--done").filter(|d| !d.is_empty()) {
        let _guard = camp.lock(None)?;
        let event = camp.emit("brief", vec![("path", v(done))])?;
        let mut monitor = camp.monitor()?;
        monitor.insert("last_brief_utc".into(), event["utc"].clone());
        camp.save_monitor(&monitor)?;
        return Ok(obj([
            ("ok", Value::Bool(true)),
            ("brief", Value::Object(event)),
        ]));
    }
    let status = crate::status::build_status(camp)?;
    let metrics = crate::status::build_metrics(camp)?;
    let mut out = obj([("ok", Value::Bool(true))]);
    for key in [
        "campaign",
        "instance",
        "waiting",
        "cells",
        "groups",
        "final",
        "actionable",
    ] {
        out.insert(key.into(), status.get(key).cloned().unwrap_or(Value::Null));
    }
    out.insert("metrics".into(), Value::Object(metrics));
    out.insert(
        "write_to".into(),
        Value::from(crate::jsonio::path_text(&camp.dir.join("briefing.md"))),
    );
    Ok(out)
}

pub fn cmd_monitor_reset(camp: &mut Campaign, _args: &Args) -> Out {
    let _guard = camp.lock(None)?;
    let total = match camp.monitor() {
        Ok(m) => need(&m, "total_blocks")?.clone(),
        Err(PmError::Usage(_)) => Value::from(0),
        Err(e) => return Err(e),
    };
    let mut fresh = crate::campaign::default_monitor();
    fresh.insert("total_blocks".into(), total);
    camp.save_monitor(&fresh)?;
    Ok(obj([("ok", Value::Bool(true)), ("monitor", v("reset"))]))
}
