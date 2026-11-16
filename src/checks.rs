//! Deciding, running and recording one check, and the gate bookkeeping around it.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::campaign::{Campaign, Fingerprint};
use crate::error::{PmError, Res};
use crate::jsonio::write_json;
use crate::proc::{alive, execute};
use crate::py::{
    eq_str, list_or_empty, need, num, obj, opt_truthy, plus_one, py_eq, py_str, truthy, Num, Obj,
};
use crate::{FINAL, SCHEMA};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Run,
    Skip,
    Refuse,
}

/// The first matching rule for one check.
#[derive(Clone, Debug)]
pub struct Plan {
    pub action: Action,
    pub outcome: &'static str,
    pub rule: u8,
    pub fp: Fingerprint,
    pub receipt: Option<String>,
    pub via: Option<String>,
    pub reason: Option<String>,
    pub covers: Obj,
}

impl Plan {
    fn new(action: Action, outcome: &'static str, rule: u8, fp: Fingerprint) -> Plan {
        Plan {
            action,
            outcome,
            rule,
            fp,
            receipt: None,
            via: None,
            reason: None,
            covers: Obj::new(),
        }
    }
}

fn opt(text: &Option<String>) -> Value {
    text.as_ref()
        .map_or(Value::Null, |s| Value::from(s.as_str()))
}

fn path_of(receipt: &Obj) -> Option<String> {
    receipt
        .get("_path")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn is_verdict(receipt: &Obj) -> bool {
    eq_str(receipt.get("result"), "PASS") || eq_str(receipt.get("result"), "FAIL")
}

pub fn check_id(check: &Obj) -> Res<String> {
    Ok(py_str(need(check, "id")?))
}

/// Apply the rule table (first match): run, skip or refuse.
pub fn decide(
    camp: &mut Campaign,
    check: &Obj,
    root: &Path,
    scope: &str,
    force: bool,
) -> Res<Plan> {
    let id = check_id(check)?;
    let fp = camp.fingerprint(check, root)?;
    if !fp.reusable {
        let mut plan = Plan::new(Action::Run, "ran", 1, fp.clone());
        plan.reason = fp.why.map(str::to_string);
        return Ok(plan);
    }
    if force {
        return Ok(Plan::new(Action::Run, "forced", 2, fp));
    }
    let mut same = Vec::new();
    for r in camp.receipts(&id)? {
        if opt_truthy(r.get("reusable"))
            && eq_str(r.get("key_sha256"), &fp.sha)
            && camp.usable(&r, None)?
            && (scope != "final" || eq_str(r.get("scope"), "final"))
        {
            same.push(r);
        }
    }
    if let Some(last) = same.iter().rev().find(|r| is_verdict(r)) {
        if eq_str(last.get("result"), "PASS") {
            let mut plan = Plan::new(Action::Skip, "reused", 3, fp);
            plan.receipt = path_of(last);
            return Ok(plan);
        }
        let mut plan = Plan::new(Action::Refuse, "refused", 4, fp);
        plan.receipt = path_of(last);
        plan.reason = Some("same-input-fail".into());
        return Ok(plan);
    }
    let cap = camp.budget("exec_retries")
        + 1
        + camp.allows("exec-retry-cap", vec![("check", Value::from(id.as_str()))])?;
    if same.len() as i64 >= cap {
        let mut plan = Plan::new(Action::Refuse, "refused", 5, fp);
        plan.receipt = same.last().and_then(path_of);
        plan.reason = Some("exec-retry-cap".into());
        return Ok(plan);
    }
    if scope != "final" {
        let wider_all: Vec<Value> = camp.check_entries().clone();
        for wider in &wider_all {
            let wider = crate::py::as_obj(wider)?;
            let covers = list_or_empty(wider.get("covers"))?;
            let wider_id = need(wider, "id")?;
            if !crate::py::contains(&covers, &Value::from(id.as_str()))
                || eq_str(Some(wider_id), &id)
            {
                continue;
            }
            let wide_fp = camp.fingerprint(wider, root)?;
            if !wide_fp.reusable {
                continue;
            }
            let mut wide = Vec::new();
            for r in camp.receipts(&py_str(wider_id))? {
                if opt_truthy(r.get("reusable"))
                    && eq_str(r.get("key_sha256"), &wide_fp.sha)
                    && camp.usable(&r, None)?
                    && is_verdict(&r)
                {
                    wide.push(r);
                }
            }
            let Some(last) = wide.last() else {
                continue;
            };
            if !eq_str(last.get("result"), "PASS") {
                continue;
            }
            let entry = match last.get("covers") {
                Some(c) if truthy(c) => c.get(id.as_str()).cloned(),
                _ => None,
            };
            if let Some(Value::Object(entry)) = entry {
                let recorded = entry.get("inputs").unwrap_or(&Value::Null);
                if py_eq(recorded, fp.inputs().unwrap_or(&Value::Null))
                    && camp.usable(last, Some(&id))?
                {
                    let mut plan = Plan::new(Action::Skip, "covered", 6, fp);
                    plan.via = path_of(last);
                    return Ok(plan);
                }
            }
        }
    }
    Ok(Plan::new(Action::Run, "ran", 7, fp))
}

/// The inputs of the checks this one covers, as they are before the run starts.
pub fn covered_inputs(camp: &mut Campaign, check: &Obj, root: &Path) -> Res<Obj> {
    let mut covers = Obj::new();
    for narrow in list_or_empty(check.get("covers"))? {
        let name = py_str(&narrow);
        let narrow_check = match camp.check(&name) {
            Ok(c) => c,
            Err(PmError::Usage(_)) => continue,
            Err(e) => return Err(e),
        };
        let fp = match camp.fingerprint(&narrow_check, root) {
            Ok(fp) => fp,
            Err(PmError::Usage(_)) => continue,
            Err(e) => return Err(e),
        };
        if fp.reusable {
            // uncommitted or unresolved inputs cannot be said to be covered
            let inputs = fp.inputs().cloned().unwrap_or(Value::Null);
            covers.insert(name, Value::Object(obj([("inputs", inputs)])));
        }
    }
    Ok(covers)
}

fn timeout_of(camp: &Campaign, check: &Obj) -> Res<f64> {
    match check.get("timeout_s") {
        Some(v) if truthy(v) => Ok(num(v)?.as_f64()),
        _ => Ok(camp.budget("check_timeout_s") as f64),
    }
}

fn monotonic_ns() -> u128 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes into the provided struct only.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as u128 * 1_000_000_000 + ts.tv_nsec as u128
}

/// Run a check without the lock, then write its receipt and event under the lock.
pub fn run_check(
    camp: &mut Campaign,
    check: &Obj,
    root: &Path,
    scope: &str,
    plan: &Plan,
    reason: Option<&str>,
    explore: Option<&str>,
) -> Res<Obj> {
    let id = check_id(check)?;
    let cwd = crate::paths::join(
        root,
        &crate::jsonio::os_text(&check.get("cwd").map_or(".".to_string(), py_str)),
    );
    let timeout = timeout_of(camp, check)?;
    let temp = camp.dir.join("logs").join(crate::jsonio::os_text(&format!(
        ".{id}-{}-{}.tmp",
        std::process::id(),
        monotonic_ns()
    )));
    let cmd = py_str(need(check, "cmd")?);
    let expect = py_str(need(check, "expect")?);
    let done = execute(&cmd, &cwd, timeout, &temp).map_err(|e| PmError::os(&e))?;
    let result = if done.error.is_some() {
        "ERROR"
    } else if done.exit_code == Some(0) && done.output.contains(&expect) {
        "PASS"
    } else {
        "FAIL"
    };
    let _guard = camp.lock(None)?;
    let mut number = Num::Int(0);
    for r in camp.receipts(&id)? {
        let n = num(r.get("n").unwrap_or(&Value::from(0)))?;
        if n.compare(number) == Some(std::cmp::Ordering::Greater) {
            number = n;
        }
    }
    let number = plus_one(&number.to_value())?;
    let n_text = py_str(&number);
    let log = format!("logs/{id}-{n_text}.log");
    if temp.exists() {
        fs::rename(&temp, camp.dir.join(crate::jsonio::os_text(&log)))
            .map_err(|e| PmError::os(&e))?;
    } else {
        fs::write(camp.dir.join(crate::jsonio::os_text(&log)), "").map_err(|e| PmError::os(&e))?;
    }
    let mut covers = plan.covers.clone();
    let mut reusable = plan.fp.reusable;
    let mut note = plan.reason.clone();
    if reusable && camp.fingerprint(check, root)?.sha != plan.fp.sha {
        // nobody knows which input it read
        reusable = false;
        note = Some("input-changed-during-run".into());
        covers = Obj::new();
    }
    let forced = plan.outcome == "forced";
    let receipt_reason = if forced {
        reason.map(str::to_string)
    } else {
        note
    };
    let receipt = obj([
        ("schema_version", Value::from(SCHEMA)),
        ("check", Value::from(id.as_str())),
        ("n", number.clone()),
        ("scope", Value::from(scope)),
        ("key", plan.fp.key.clone()),
        ("key_sha256", Value::from(plan.fp.sha.as_str())),
        ("reusable", Value::Bool(reusable)),
        ("result", Value::from(result)),
        ("exit_code", done.exit_code.map_or(Value::Null, Value::from)),
        ("error", done.error.map_or(Value::Null, Value::from)),
        ("log", Value::from(log.as_str())),
        ("started", Value::from(done.started.as_str())),
        ("ended", Value::from(done.ended.as_str())),
        ("seconds", Value::from(done.seconds)),
        ("executor", Value::from(camp.executor.as_str())),
        ("forced", Value::Bool(forced)),
        ("reason", opt(&receipt_reason)),
        ("target", Value::from(crate::jsonio::path_text(root))),
        ("explore", explore.map_or(Value::Null, Value::from)),
        ("covers", Value::Object(covers)),
    ]);
    let path = format!("receipts/{id}/{n_text}.json");
    write_json(
        &camp.dir.join(crate::jsonio::os_text(&path)),
        &Value::Object(receipt),
    )?;
    let outcome = if result == "ERROR" {
        "error"
    } else {
        plan.outcome
    };
    camp.emit(
        "check",
        vec![
            ("check", Value::from(id.as_str())),
            ("outcome", Value::from(outcome)),
            ("scope", Value::from(scope)),
            ("receipt", Value::from(path.as_str())),
            ("reason", opt(&receipt_reason)),
        ],
    )?;
    Ok(obj([
        ("check", Value::from(id.as_str())),
        ("outcome", Value::from(outcome)),
        ("result", Value::from(result)),
        ("receipt", Value::from(path.as_str())),
        ("via", Value::Null),
        ("reason", opt(&receipt_reason)),
    ]))
}

/// Record a check that was not run (reused, covered or refused). The caller holds the lock.
pub fn skip_check(camp: &mut Campaign, check: &Obj, scope: &str, plan: &Plan) -> Res<Obj> {
    let id = check_id(check)?;
    let pointer = plan.via.clone().or_else(|| plan.receipt.clone());
    camp.emit(
        "check",
        vec![
            ("check", Value::from(id.as_str())),
            ("outcome", Value::from(plan.outcome)),
            ("scope", Value::from(scope)),
            ("receipt", opt(&pointer)),
            ("reason", opt(&plan.reason)),
        ],
    )?;
    if plan.reason.as_deref() == Some("same-input-fail") {
        let mut monitor = camp.monitor()?;
        let next = plus_one(need(&monitor, "refused_since")?)?;
        monitor.insert("refused_since".into(), next);
        camp.save_monitor(&monitor)?;
    }
    let result = match plan.outcome {
        "reused" => Value::from("PASS"),
        "covered" => Value::Null,
        _ if plan.rule == 4 => Value::from("FAIL"),
        _ => Value::Null,
    };
    Ok(obj([
        ("check", Value::from(id.as_str())),
        ("outcome", Value::from(plan.outcome)),
        ("result", result),
        ("receipt", opt(&plan.receipt)),
        ("via", opt(&plan.via)),
        ("reason", opt(&plan.reason)),
    ]))
}

/// Checks of one gate, covering checks before the checks they cover.
pub fn ordered(camp: &Campaign, names: &[String]) -> Res<Vec<Obj>> {
    let mut unique: Vec<&String> = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    let mut waiting: Vec<Obj> = unique.iter().map(|n| camp.check(n)).collect::<Res<_>>()?;
    let mut placed = Vec::new();
    while !waiting.is_empty() {
        let mut ready = Vec::new();
        for (i, c) in waiting.iter().enumerate() {
            let cid = need(c, "id")?;
            let mut blocked = false;
            for (j, w) in waiting.iter().enumerate() {
                if i != j && crate::py::contains(&list_or_empty(w.get("covers"))?, cid) {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                ready.push(i);
            }
        }
        if ready.is_empty() {
            ready.push(0);
        }
        let mut keep = Vec::new();
        for (i, c) in waiting.into_iter().enumerate() {
            if ready.contains(&i) {
                placed.push(c);
            } else {
                keep.push(c);
            }
        }
        waiting = keep;
    }
    Ok(placed)
}

/// Which forced-rerun cap is reached, if any.
pub fn force_cap(camp: &mut Campaign, check: &str) -> Res<Option<&'static str>> {
    let forced: Vec<Obj> = camp
        .all_receipts()?
        .into_iter()
        .filter(|r| opt_truthy(r.get("forced")))
        .collect();
    let per_check = forced
        .iter()
        .filter(|r| eq_str(r.get("check"), check))
        .count() as i64;
    let allowed = camp.allows(
        "forced-rerun-cap",
        vec![
            ("check", Value::from(check)),
            ("cap", Value::from("per-check")),
        ],
    )?;
    if per_check >= camp.budget("forced_reruns_per_check") + allowed {
        return Ok(Some("per-check"));
    }
    let allowed = camp.allows("forced-rerun-cap", vec![("cap", Value::from("total"))])?;
    if forced.len() as i64 >= camp.budget("forced_reruns_total") + allowed {
        return Ok(Some("total"));
    }
    Ok(None)
}

pub fn gate_state_of(results: &[Obj]) -> &'static str {
    if results
        .iter()
        .any(|r| eq_str(r.get("reason"), "exec-retry-cap"))
    {
        return "waiting";
    }
    if results
        .iter()
        .any(|r| eq_str(r.get("result"), "FAIL") || eq_str(r.get("reason"), "same-input-fail"))
    {
        return "failed";
    }
    if results.iter().any(|r| eq_str(r.get("outcome"), "error")) {
        return "pending";
    }
    "passed"
}

/// Evaluate every check of a gate in order; a failure does not stop the rest.
pub fn run_checks(
    camp: &mut Campaign,
    names: &[String],
    scope: &str,
    force: Option<&str>,
    reason: Option<&str>,
    hitl_target: Vec<(&'static str, Value)>,
) -> Res<(Vec<Obj>, Value)> {
    let mut results = Vec::new();
    let mut hitl = Value::Null;
    for check in ordered(camp, names)? {
        let id = check_id(&check)?;
        let plan = {
            let _guard = camp.lock(None)?;
            let root = camp.root.clone();
            let mut plan = decide(camp, &check, &root, scope, force == Some(id.as_str()))?;
            if plan.action == Action::Run {
                plan.covers = covered_inputs(camp, &check, &root)?;
                plan
            } else {
                results.push(skip_check(camp, &check, scope, &plan)?);
                if plan.reason.as_deref() == Some("exec-retry-cap") {
                    let mut target = vec![
                        ("check", Value::from(id.as_str())),
                        ("cap", Value::from("per-check")),
                    ];
                    target.extend(hitl_target.iter().cloned());
                    hitl = camp.open_hitl(
                        "exec-retry-cap",
                        &format!(
                            "검사 {id}의 실행이 같은 입력에서 거듭 실패했다. 환경을 고친 뒤 다시 돌려도 되는가?"
                        ),
                        target,
                    )?;
                }
                continue;
            }
        };
        let root = camp.root.clone();
        results.push(run_check(camp, &check, &root, scope, &plan, reason, None)?);
    }
    Ok((results, hitl))
}

pub fn reset_final(camp: &Campaign) -> Res<()> {
    let mut gate = camp.gate(FINAL)?;
    let state = need(&gate, "state")?;
    if eq_str(Some(state), "passed") || eq_str(Some(state), "failed") {
        gate.insert("state".into(), Value::from("pending"));
        camp.save_gate(FINAL, &gate)?;
    }
    Ok(())
}

/// Leave a settled gate: gate to pending, closed members back to open.
pub fn reopen_gate(camp: &mut Campaign, group: &str, reason: &str, states: &[&str]) -> Res<bool> {
    let mut gate = camp.gate(group)?;
    let state = need(&gate, "state")?;
    if !states.iter().any(|s| eq_str(Some(state), s)) {
        return Ok(false);
    }
    gate.insert("state".into(), Value::from("pending"));
    camp.save_gate(group, &gate)?;
    for i in camp.member_indices(group, true)? {
        let cell = camp.cell_at_mut(i);
        if eq_str(cell.get("state"), "closed") {
            cell.insert("state".into(), Value::from("open"));
        }
    }
    camp.save_decl()?;
    camp.emit(
        "gate-reopen",
        vec![
            ("group", Value::from(group)),
            ("reason", Value::from(reason)),
        ],
    )?;
    Ok(true)
}

pub fn gate_blockers(camp: &mut Campaign, group: &str) -> Res<Vec<String>> {
    let cells = camp.members(group)?;
    if cells.is_empty() {
        return Ok(vec!["the group has no live member".to_string()]);
    }
    let last = camp.last_phase()?;
    let mut problems = Vec::new();
    for c in &cells {
        let state = need(c, "state")?;
        if !eq_str(Some(state), "open") {
            problems.push(format!("{} is {}", py_str(need(c, "cell")?), py_str(state)));
        }
    }
    for c in &cells {
        let name = py_str(need(c, "cell")?);
        let state = camp.phase_state(&name, &last)?;
        if !eq_str(Some(&state), "done") {
            problems.push(format!("{name}/{last} is {}", py_str(&state)));
        }
    }
    Ok(problems)
}

pub fn member_attempts(camp: &mut Campaign, group: &str) -> Res<Obj> {
    let last = camp.last_phase()?;
    let mut out = Obj::new();
    for c in camp.members(group)? {
        let name = py_str(need(&c, "cell")?);
        let attempt = need(&camp.phase(&name, &last)?, "attempt")?.clone();
        out.insert(name, attempt);
    }
    Ok(out)
}

pub fn running_elsewhere(gate: &Obj) -> bool {
    let me = Value::from(std::process::id());
    opt_truthy(gate.get("started"))
        && !opt_truthy(gate.get("ended"))
        && !py_eq(gate.get("pid").unwrap_or(&Value::Null), &me)
        && alive(gate.get("pid"))
}
