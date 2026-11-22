//! Status, the actionable list, briefing timing and metrics.

use serde_json::Value;

use crate::campaign::Campaign;
use crate::checks::gate_blockers;
use crate::commands::{final_settled, start_problems};
use crate::error::{PmError, Res};
use crate::jsonio::read_json;
use crate::proc::git;
use crate::py::{
    as_obj, eq_str, float_value, list_or_empty, need, num, obj, opt_truthy, py_eq, py_str, round3,
    truthy, Num, Obj,
};
use crate::time::{now, parse_time};
use crate::FINAL;

fn ignore_usage<T>(result: Res<T>) -> Res<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(PmError::Usage(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Checks of a passed gate whose recorded input differs from the present one.
pub fn stale_checks(camp: &mut Campaign, gate: &Obj) -> Res<Vec<String>> {
    let mut stale = Vec::new();
    for item in list_or_empty(gate.get("checks"))? {
        let item = as_obj(&item)?;
        let covered = eq_str(item.get("outcome"), "covered");
        let pointer = if covered {
            item.get("via")
        } else {
            item.get("receipt")
        };
        let pointer = match pointer {
            Some(p) if truthy(p) => py_str(p),
            _ => continue,
        };
        let path = camp.dir.join(crate::jsonio::os_text(&pointer));
        if !path.is_file() {
            continue;
        }
        let receipt = read_json(&path, None)?;
        let receipt = as_obj(&receipt)?.clone();
        let name = py_str(need(item, "check")?);
        let Some(check) = ignore_usage(camp.check(&name))? else {
            continue;
        };
        if !opt_truthy(receipt.get("reusable")) {
            continue;
        }
        let root = camp.root.clone();
        let current = Value::Object(camp.input_ids(&root, &list_or_empty(check.get("inputs"))?)?);
        let recorded = if covered {
            dig(dig(receipt.get("covers"), &name), "inputs")
        } else {
            dig(receipt.get("key"), "inputs")
        };
        if !py_eq(&recorded.cloned().unwrap_or(Value::Null), &current) {
            stale.push(name);
        }
    }
    Ok(stale)
}

/// `(value or {}).get(key)`.
fn dig<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match value {
        Some(Value::Object(m)) => m.get(key),
        _ => None,
    }
}

/// Checks of a passed gate whose inputs were changed and not committed. Display only, like STALE.
pub fn uncommitted_checks(camp: &mut Campaign, gate: &Obj) -> Res<Vec<String>> {
    let mut found = Vec::new();
    for item in list_or_empty(gate.get("checks"))? {
        let item = as_obj(&item)?;
        let name = py_str(need(item, "check")?);
        let Some(check) = ignore_usage(camp.check(&name))? else {
            continue;
        };
        let inputs: Vec<String> = list_or_empty(check.get("inputs"))?
            .iter()
            .map(py_str)
            .collect();
        if inputs.is_empty() {
            continue;
        }
        let mut args = vec!["status", "--porcelain", "--"];
        args.extend(inputs.iter().map(String::as_str));
        let (code, out) = git(&camp.root, &args)?;
        if code == 0 && !out.is_empty() {
            found.push(name);
        }
    }
    Ok(found)
}

pub fn item_key(kind: &str, cell: &str, phase: &str, group: &str) -> String {
    [kind, cell, phase, group].join("|")
}

struct Items {
    list: Vec<(i64, Obj)>,
}

impl Items {
    fn add(
        &mut self,
        order: i64,
        kind: &str,
        text: String,
        command: String,
        place: Vec<(&str, Value)>,
    ) {
        let mut item = obj([
            ("order", Value::from(order)),
            ("kind", Value::from(kind)),
            ("text", Value::from(text)),
            ("command", Value::from(command)),
        ]);
        let get = |k: &str| {
            place
                .iter()
                .find(|(n, _)| *n == k)
                .map_or(String::new(), |(_, v)| py_str(v))
        };
        let key = item_key(kind, &get("cell"), &get("phase"), &get("group"));
        for (k, v) in place.iter() {
            item.insert((*k).to_string(), v.clone());
        }
        item.insert("key".into(), Value::from(key));
        self.list.push((order, item));
    }
}

/// What the main agent can do without a person, in the order the skill gives.
pub fn actionable(camp: &mut Campaign) -> Res<Vec<Obj>> {
    let mut items = Items { list: Vec::new() };
    let pm = camp.pm_prefix();
    let mut held: Vec<(String, bool)> = Vec::new();
    for name in camp.group_names()? {
        let state = camp.group_state(&name)?;
        held.push((
            name,
            state == crate::GROUP_STOPPED || state == crate::GROUP_WAITING,
        ));
    }
    let is_held = |g: &str| {
        held.iter()
            .rev()
            .find(|(n, _)| n == g)
            .is_some_and(|(_, h)| *h)
    };
    let phases = camp.phases();
    let mut cells = Vec::new();
    for c in camp.cells()? {
        if eq_str(c.get("state"), "open") && !is_held(&py_str(need(&c, "group")?)) {
            cells.push(c);
        }
    }
    let mut table: Vec<Vec<Obj>> = Vec::new();
    for c in &cells {
        let name = py_str(need(c, "cell")?);
        table.push(
            phases
                .iter()
                .map(|p| camp.phase(&name, p))
                .collect::<Res<_>>()?,
        );
    }
    let state_is = |data: &Obj, s: &str| eq_str(data.get("state"), s);

    for (c, datas) in cells.iter().zip(&table) {
        let name = py_str(&c["cell"]);
        for (phase, data) in phases.iter().zip(datas) {
            need(data, "state")?;
            if state_is(data, "running") {
                items.add(
                    1,
                    "phase-running",
                    format!("실행 중인 단계 {name}/{phase}를 끝낸다"),
                    format!("{pm} phase close {name} {phase}"),
                    vec![
                        ("cell", Value::from(name.as_str())),
                        ("phase", Value::from(phase.as_str())),
                        (
                            "started",
                            data.get("started").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "retries",
                            data.get("retries").cloned().unwrap_or(Value::from(0)),
                        ),
                    ],
                );
            }
        }
    }
    let groups = camp.group_names()?;
    for name in &groups {
        if !is_held(name)
            && eq_str(Some(&camp.gate_state(name)?), "pending")
            && gate_blockers(camp, name)?.is_empty()
        {
            items.add(
                2,
                "gate-run",
                format!("묶음 {name}의 gate를 돌린다"),
                format!("{pm} gate run {name}"),
                vec![("group", Value::from(name.as_str()))],
            );
        }
    }
    for c in &cells {
        let group = py_str(need(c, "group")?);
        if !eq_str(Some(need(c, "review")?), "none")
            && eq_str(Some(&camp.gate_state(&group)?), "passed")
        {
            let name = py_str(&c["cell"]);
            let (kind, text, command) = match crate::commands::standing_review(camp, &name)?.0 {
                standing @ ("clean" | "accepted") => (
                    "close",
                    format!("cell {name}에 서 있는 검토가 {standing}이다. 닫는다"),
                    format!("{pm} cell close {name}"),
                ),
                "blocking" => (
                    "fix",
                    format!("cell {name}의 차단 지적을 고치거나 사람에게 넘긴다(다음 회차는 고친 뒤에만)"),
                    format!(
                        "{pm} cell reopen {name} --reason <고칠 차단> 또는 사람이 받아들이면 \
                         {pm} cell close {name} --accept-ref <근거>"
                    ),
                ),
                _ => (
                    "review",
                    format!("cell {name}의 새 맥락 검토를 받는다(차단이 없으면 그 회차로 닫힌다)"),
                    format!("{pm} review record {name} <review.json>"),
                ),
            };
            items.add(
                3,
                kind,
                text,
                command,
                vec![
                    ("cell", Value::from(name.as_str())),
                    ("group", c["group"].clone()),
                ],
            );
        }
    }
    for name in &groups {
        if !is_held(name)
            && eq_str(Some(&camp.gate_state(name)?), "failed")
            && !camp.members(name)?.is_empty()
        {
            items.add(
                4,
                "gate-failed",
                format!("실패한 gate {name}의 원인을 가르고 지목한 cell을 다시 연다"),
                format!("{pm} cell reopen <cell> --reason <원인>"),
                vec![("group", Value::from(name.as_str()))],
            );
        }
    }
    for (c, datas) in cells.iter().zip(&table) {
        let name = py_str(&c["cell"]);
        for (phase, data) in phases.iter().zip(datas) {
            if state_is(data, "failed") {
                items.add(
                    5,
                    "phase-failed",
                    format!("실패한 단계 {name}/{phase}를 고쳐 다시 닫는다"),
                    format!("{pm} phase close {name} {phase}"),
                    vec![
                        ("cell", Value::from(name.as_str())),
                        ("phase", Value::from(phase.as_str())),
                    ],
                );
            }
        }
    }
    for (c, datas) in cells.iter().zip(&table) {
        let name = py_str(&c["cell"]);
        for (phase, data) in phases.iter().zip(datas) {
            if state_is(data, "pending") {
                if start_problems(camp, c, phase)?.is_empty() {
                    items.add(
                        6,
                        "phase-start",
                        format!("단계 {name}/{phase}를 시작한다"),
                        format!("{pm} phase start {name} {phase}"),
                        vec![
                            ("cell", Value::from(name.as_str())),
                            ("phase", Value::from(phase.as_str())),
                        ],
                    );
                }
                break;
            }
        }
    }
    let final_gate = camp.gate(FINAL)?;
    if !camp.final_names()?.is_empty() && !groups.is_empty() && final_settled(camp)?.is_empty() {
        let state = need(&final_gate, "state")?.clone();
        if eq_str(Some(&state), "failed") {
            items.add(
                7,
                "final",
                "마지막 전체 검증이 실패했다. 원인을 가려 그 묶음의 cell을 다시 연다".into(),
                format!("{pm} cell reopen <cell> --reason <원인>"),
                vec![],
            );
        } else if eq_str(Some(&state), "pending")
            || (eq_str(Some(&state), "passed") && !stale_checks(camp, &final_gate)?.is_empty())
        {
            items.add(
                7,
                "final",
                "마지막 전체 검증을 돌린다".into(),
                format!("{pm} final run"),
                vec![],
            );
        }
    }
    let mut list = items.list;
    list.sort_by_key(|(order, _)| *order);
    Ok(list.into_iter().map(|(_, item)| item).collect())
}

pub fn briefing_due(camp: &mut Campaign) -> Res<Obj> {
    let briefing = match camp.decl.get("briefing") {
        Some(Value::Object(b)) => b.clone(),
        _ => Obj::new(),
    };
    let events = camp.events()?.clone();
    let opened = events
        .iter()
        .find(|e| eq_str(e.get("event"), "campaign-open"));
    let last_index = events.iter().rposition(|e| eq_str(e.get("event"), "brief"));
    let since = match last_index {
        Some(i) => &events[i + 1..],
        None => &events[..],
    };
    let on_phase = opt_truthy(briefing.get("on_phase"))
        && since.iter().any(|e| {
            ["phase-close", "gate", "final"]
                .iter()
                .any(|n| eq_str(e.get("event"), n))
        });
    let utc_of = |e: &Obj| e.get("utc").and_then(Value::as_str).and_then(parse_time);
    let anchor = match last_index {
        Some(i) => utc_of(&events[i]),
        None => opened.and_then(utc_of),
    };
    let mut by_hours = false;
    if let Some(hours @ Value::Number(_)) = briefing.get("every_hours") {
        if num(hours)?.gt(Num::Int(0)) {
            if let Some(anchor) = anchor {
                let budget = crate::time::timedelta_of(hours, 3_600_000_000)?;
                by_hours = i128::from(now().utc_us - anchor.utc_us) >= budget;
            }
        }
    }
    Ok(obj([
        ("on_phase", Value::Bool(on_phase)),
        ("hours", Value::Bool(by_hours)),
    ]))
}

pub fn build_status(camp: &mut Campaign) -> Res<Obj> {
    let phases = camp.phases();
    let mut cells = Vec::new();
    for cell in camp.cells()? {
        let name = py_str(need(&cell, "cell")?);
        let mut table = Obj::new();
        for phase in &phases {
            let data = camp.phase(&name, phase)?;
            table.insert(
                phase.clone(),
                Value::Object(obj([
                    ("state", need(&data, "state")?.clone()),
                    ("attempt", data["attempt"].clone()),
                ])),
            );
        }
        cells.push(Value::Object(obj([
            ("cell", cell["cell"].clone()),
            ("group", need(&cell, "group")?.clone()),
            ("state", cell["state"].clone()),
            ("review", cell["review"].clone()),
            ("origin", cell["origin"].clone()),
            ("phases", Value::Object(table)),
        ])));
    }
    let mut groups = Vec::new();
    let mut stale_all = Vec::new();
    for name in camp.group_names()? {
        let gate = camp.gate(&name)?;
        let passed = eq_str(Some(need(&gate, "state")?), "passed");
        let stale = if passed {
            stale_checks(camp, &gate)?
        } else {
            Vec::new()
        };
        for check in &stale {
            stale_all.push(Value::Object(obj([
                ("group", Value::from(name.as_str())),
                ("check", Value::from(check.as_str())),
            ])));
        }
        let uncommitted = if passed {
            uncommitted_checks(camp, &gate)?
        } else {
            Vec::new()
        };
        let members: Vec<Value> = camp
            .members(&name)?
            .iter()
            .map(|c| c["cell"].clone())
            .collect();
        let mut checks = Vec::new();
        for item in list_or_empty(gate.get("checks"))? {
            let item = as_obj(&item)?;
            let check = item.get("check").cloned().unwrap_or(Value::Null);
            let is_stale = stale.iter().any(|s| eq_str(Some(&check), s));
            checks.push(Value::Object(obj([
                ("check", check),
                (
                    "outcome",
                    item.get("outcome").cloned().unwrap_or(Value::Null),
                ),
                ("result", item.get("result").cloned().unwrap_or(Value::Null)),
                (
                    "stale",
                    if is_stale {
                        Value::from("STALE")
                    } else {
                        Value::Null
                    },
                ),
            ])));
        }
        let state = camp.group_state(&name)?;
        groups.push(Value::Object(obj([
            ("group", Value::from(name.as_str())),
            ("state", Value::from(state)),
            ("gate", gate["state"].clone()),
            (
                "stale",
                if stale.is_empty() {
                    Value::Null
                } else {
                    Value::from("STALE")
                },
            ),
            ("stale_checks", strings(&stale)),
            ("uncommitted_checks", strings(&uncommitted)),
            ("members", Value::Array(members)),
            ("checks", Value::Array(checks)),
        ])));
    }
    let final_gate = camp.gate(FINAL)?;
    let final_stale = if eq_str(Some(need(&final_gate, "state")?), "passed") {
        stale_checks(camp, &final_gate)?
    } else {
        Vec::new()
    };
    let monitor = camp.monitor()?;
    let actionable: Vec<Value> = actionable(camp)?
        .into_iter()
        .map(|mut item| {
            item.shift_remove("key");
            Value::Object(item)
        })
        .collect();
    let waiting: Vec<Value> = camp
        .hitls()?
        .into_iter()
        .filter(|h| h["answer"].is_null())
        .map(|h| {
            Value::Object(
                h.into_iter()
                    .filter(|(k, _)| k != "answer" && k != "event")
                    .collect(),
            )
        })
        .collect();
    let briefing = briefing_due(camp)?;
    Ok(obj([
        ("ok", Value::Bool(true)),
        (
            "campaign",
            camp.decl.get("id").cloned().unwrap_or(Value::Null),
        ),
        ("instance", Value::from(crate::jsonio::path_text(&camp.dir))),
        ("target", Value::from(crate::jsonio::path_text(&camp.root))),
        ("cells", Value::Array(cells)),
        ("groups", Value::Array(groups)),
        (
            "final",
            Value::Object(obj([
                ("state", final_gate["state"].clone()),
                (
                    "stale",
                    if final_stale.is_empty() {
                        Value::Null
                    } else {
                        Value::from("STALE")
                    },
                ),
                ("stale_checks", strings(&final_stale)),
            ])),
        ),
        ("stale", Value::Array(stale_all)),
        ("actionable", Value::Array(actionable)),
        ("waiting", Value::Array(waiting)),
        ("briefing_due", Value::Object(briefing)),
        (
            "monitor",
            Value::Object(obj([
                ("session", monitor["session"].clone()),
                ("total_blocks", monitor["total_blocks"].clone()),
                ("released", monitor["released"].clone()),
            ])),
        ),
    ]))
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

/// `float(value or 0)`.
fn float_or_zero(value: Option<&Value>) -> Res<f64> {
    match value {
        Some(v) if truthy(v) => match v {
            Value::String(s) => crate::commands::parse_float(s).map_err(|_| {
                PmError::Internal(format!(
                    "ValueError: could not convert string to float: {}",
                    crate::py::repr_str(s)
                ))
            }),
            Value::Number(n) if crate::py::is_int(v) && crate::py::number_f64(n).is_infinite() => {
                Err(PmError::Internal(
                    "OverflowError: int too large to convert to float".into(),
                ))
            }
            Value::Number(_) | Value::Bool(_) => Ok(num(v)?.as_f64()),
            other => Err(PmError::Internal(format!(
                "TypeError: float() argument must be a string or a real number, not '{}'",
                crate::py::type_name(other)
            ))),
        },
        _ => Ok(0.0),
    }
}

pub fn build_metrics(camp: &mut Campaign) -> Res<Obj> {
    let names = ["ran", "forced", "error", "reused", "covered", "refused"];
    let mut counts = [0i64; 6];
    let events = camp.events()?.clone();
    for event in &events {
        if eq_str(event.get("event"), "check") {
            if let Some(i) = names.iter().position(|n| eq_str(event.get("outcome"), n)) {
                counts[i] += 1;
            }
        }
    }
    let executed = counts[0] + counts[1] + counts[2];
    let skipped = counts[3] + counts[4];
    let mut gates = Obj::new();
    for event in &events {
        if eq_str(event.get("event"), "gate") || eq_str(event.get("event"), "final") {
            let name = match event.get("group") {
                Some(g) if truthy(g) => py_str(g),
                _ => FINAL.to_string(),
            };
            let seconds = float_or_zero(event.get("seconds"))?;
            let entry = gates.entry(name).or_insert_with(|| {
                Value::Object(obj([
                    ("runs", Value::from(0)),
                    ("seconds", float_value(0.0)),
                ]))
            });
            if let Value::Object(entry) = entry {
                let runs = num(&entry["runs"])?.plus(Num::Int(1)).to_value();
                let total = num(&entry["seconds"])?.as_f64() + seconds;
                entry.insert("runs".into(), runs);
                entry.insert("seconds".into(), float_value(round3(total)));
            }
        }
    }
    let receipts = camp.all_receipts()?;
    let mut check_seconds = 0.0;
    for r in &receipts {
        check_seconds += float_or_zero(r.get("seconds"))?;
    }
    let total = executed + skipped;
    let ratio = |n: i64| {
        if total > 0 {
            float_value(round3(n as f64 / total as f64))
        } else {
            float_value(0.0)
        }
    };
    let mut count_map = Obj::new();
    for (n, c) in names.iter().zip(counts) {
        count_map.insert((*n).to_string(), Value::from(c));
    }
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("checks", Value::Object(count_map)),
        ("executed", Value::from(executed)),
        ("skipped", Value::from(skipped)),
        ("receipt_files", Value::from(receipts.len())),
        (
            "forced_receipts",
            Value::from(
                receipts
                    .iter()
                    .filter(|r| opt_truthy(r.get("forced")))
                    .count(),
            ),
        ),
        ("reused_ratio", ratio(counts[3])),
        ("covered_ratio", ratio(counts[4])),
        // the baseline's sum() of no receipts is the integer 0
        (
            "check_seconds",
            if receipts.is_empty() {
                Value::from(0)
            } else {
                float_value(round3(check_seconds))
            },
        ),
        ("gates", Value::Object(gates)),
    ]))
}
