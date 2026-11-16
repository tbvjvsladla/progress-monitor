//! The Stop hook: whether to keep the main agent going, and with what text.

use std::io::{Read, Write};
use std::path::Path;

use serde_json::Value;

use crate::campaign::Campaign;
use crate::error::{PmError, Res};
use crate::jsonio::{compact, digest};
use crate::paths::Paths;
use crate::py::{
    eq_str, ge_num, need, num, obj, opt_truthy, plus_one, py_eq, py_str, strip, Num, Obj,
};
use crate::status::{actionable, briefing_due};
use crate::time::{now, parse_time, stamp_now};
use crate::{FINAL, TOOL};

pub fn state_digest(camp: &mut Campaign) -> Res<String> {
    let phases = camp.phases();
    let mut cells = Obj::new();
    let mut table = Obj::new();
    for c in camp.cells()? {
        let name = py_str(need(&c, "cell")?);
        cells.insert(name.clone(), c["state"].clone());
        let mut row = Obj::new();
        for p in &phases {
            let data = camp.phase(&name, p)?;
            row.insert(
                p.clone(),
                Value::Array(vec![need(&data, "state")?.clone(), data["attempt"].clone()]),
            );
        }
        table.insert(name, Value::Object(row));
    }
    let mut gates = Obj::new();
    let mut names = camp.group_names()?;
    names.push(FINAL.to_string());
    for name in names {
        let state = camp.gate_state(&name)?;
        gates.insert(name, state);
    }
    let executed = camp
        .all_receipts()?
        .iter()
        .filter(|r| eq_str(r.get("result"), "PASS") || eq_str(r.get("result"), "FAIL"))
        .count();
    let reviews = camp.events_named("review")?.len();
    let view = Value::Object(obj([
        ("cells", Value::Object(cells)),
        ("phases", Value::Object(table)),
        ("gates", Value::Object(gates)),
        ("executed", Value::from(executed)),
        ("reviews", Value::from(reviews)),
    ]));
    digest(&view)
}

fn directive(pm: &str, item: &Obj, count: Num, retries: Num) -> Res<(&'static str, String)> {
    let kind = py_str(need(item, "kind")?);
    let spent =
        kind == "phase-running" && ge_num(item.get("retries").unwrap_or(&Value::from(0)), retries)?;
    let text = |k: &str| item.get(k).map_or("None".to_string(), py_str);
    if retries.ge(count) && !spent {
        if kind == "phase-running" {
            return Ok((
                "retry",
                format!(
                    "새 수행자로 다시 시작한다: {pm} phase retry {} {}",
                    text("cell"),
                    text("phase")
                ),
            ));
        }
        return Ok((
            "retry",
            format!("그 일을 한 번 더 시도한다: {}", text("command")),
        ));
    }
    if ["phase-running", "review", "phase-failed", "phase-start"].contains(&kind.as_str()) {
        return Ok((
            "bypass",
            format!(
                "이 cell을 멈추고 다음 일로 넘어간다: {pm} cell stop {} --reason hang",
                text("cell")
            ),
        ));
    }
    if kind == "gate-run" || kind == "gate-failed" {
        return Ok((
            "bypass",
            format!(
                "사람에게 넘기고 다음 일로 넘어간다: {pm} hitl open --group {} --kind hang --question <물음>",
                text("group")
            ),
        ));
    }
    Ok((
        "bypass",
        format!("사람에게 넘긴다: {pm} hitl open --final --kind hang --question <물음>"),
    ))
}

pub enum Decision {
    Guide(String),
    Block(String),
}

fn inc(map: &mut Obj, key: &str) -> Res<()> {
    let next = plus_one(need(map, key)?)?;
    map.insert(key.to_string(), next);
    Ok(())
}

/// Keep the agent going (guide or block), or let it stop (None).
pub fn hook_decision(camp: &mut Campaign, payload: &Obj) -> Res<Option<Decision>> {
    let session = match payload.get("session_id") {
        Some(s) if crate::py::truthy(s) => py_str(s),
        _ => "unknown".to_string(),
    };
    let mut monitor = camp.monitor()?;
    let current = monitor["session"].clone();
    if crate::py::truthy(&current) && !eq_str(Some(&current), &session) {
        return Ok(None); // rule 2
    }
    if !crate::py::truthy(&current) {
        monitor.insert("session".into(), Value::from(session));
        camp.save_monitor(&monitor)?;
    }
    let items = actionable(camp)?;
    if items.is_empty() {
        return Ok(None); // rule 3
    }
    let budget_us = crate::time::timedelta_of(&camp.budget_value("phase_minutes"), 60 * 1_000_000)?;
    let overdue = items.iter().find(|i| {
        eq_str(i.get("kind"), "phase-running")
            && i.get("started")
                .and_then(Value::as_str)
                .and_then(parse_time)
                .is_some_and(|s| i128::from(now().utc_us - s.utc_us) > budget_us)
    });
    if opt_truthy(payload.get("background_tasks")) && overdue.is_none() {
        return Ok(None); // rule 4
    }
    let digest = state_digest(camp)?;
    let changed = !py_eq(&monitor["digest"], &Value::from(digest.as_str()));
    if changed {
        monitor.insert("digest".into(), Value::from(digest));
        monitor.insert("unchanged".into(), Value::from(0));
        monitor.insert("blocks".into(), Value::from(0));
        monitor.insert("released".into(), Value::Bool(false));
    } else {
        inc(&mut monitor, "unchanged")?;
    }
    let refused = num(&monitor["refused_since"])?.gt(Num::Int(0)) && !changed;
    monitor.insert("refused_since".into(), Value::from(0));
    if crate::py::truthy(&monitor["released"]) {
        camp.save_monitor(&monitor)?;
        return Ok(None); // rule 5
    }
    if ge_num(&monitor["blocks"], camp.budget("stop_block_cap"))? {
        camp.emit(
            "monitor-release",
            vec![("blocks", monitor["blocks"].clone())],
        )?;
        monitor.insert("released".into(), Value::Bool(true));
        camp.save_monitor(&monitor)?;
        return Ok(None); // rule 6
    }
    inc(&mut monitor, "blocks")?;
    inc(&mut monitor, "total_blocks")?;
    monitor.insert("last_block_utc".into(), Value::from(stamp_now()));
    let head = format!(
        "[{TOOL}] 캠페인 {}",
        camp.decl.get("id").map_or("None".to_string(), py_str)
    );
    let pm = camp.pm_prefix();
    if ge_num(&monitor["unchanged"], camp.budget("stall_stops"))? || overdue.is_some() || refused {
        let item = overdue.unwrap_or(&items[0]).clone(); // rule 7
        let key = py_str(&item["key"]);
        let stalls = match monitor.get_mut("stalls") {
            Some(Value::Object(s)) => s,
            _ => {
                return Err(PmError::Internal(
                    "TypeError: 'stalls' is not a mapping".into(),
                ))
            }
        };
        let count = plus_one(stalls.get(&key).unwrap_or(&Value::from(0)))?;
        stalls.insert(key.clone(), count.clone());
        monitor.insert("unchanged".into(), Value::from(0));
        let (name, text) = directive(&pm, &item, num(&count)?, camp.budget("stall_retries"))?;
        camp.emit(
            "hang",
            vec![
                ("item", Value::from(key)),
                ("count", count.clone()),
                ("directive", Value::from(name)),
            ],
        )?;
        camp.save_monitor(&monitor)?;
        let why = if overdue.is_some() {
            "시간 예산을 넘겼다"
        } else if refused {
            "같은 입력으로 실패한 검사를 다시 돌리려 했다"
        } else {
            "상태 변화 없이 종료 시도가 이어졌다"
        };
        return Ok(Some(Decision::Block(format!(
            "{head}: 정체다({why}, 이 항목 {}회째). 항목: {}. 지시: {text}",
            py_str(&count),
            py_str(&item["text"])
        ))));
    }
    camp.save_monitor(&monitor)?;
    let item = &items[0]; // rule 8
    let mut text = format!(
        "{head}: 스스로 할 수 있는 일이 {}건 남아 있다. 다음 일: {}. 명령: {}. 전체 목록은 {pm} status로 본다.",
        items.len(),
        py_str(&item["text"]),
        py_str(&item["command"])
    );
    if briefing_due(camp)?["hours"] == Value::Bool(true) {
        text.push_str(" 시간 단위 브리핑 때가 됐다. 브리핑 지시문으로 브리핑을 만든다.");
    }
    Ok(Some(Decision::Guide(text)))
}

fn describe(err: &PmError) -> String {
    match err {
        PmError::Usage(text) => format!("Usage: {text}"),
        PmError::Internal(text) => text.clone(),
        PmError::Refused(payload) => {
            format!("Refused: {}", compact(&Value::Object(payload.clone())))
        }
    }
}

fn run_hook(
    base: &Path,
    harness: &str,
    executor: &str,
    paths: &Paths,
) -> Result<Option<String>, String> {
    let mut raw = Vec::new();
    std::io::stdin()
        .read_to_end(&mut raw)
        .map_err(|e| format!("OSError: {e}"))?;
    let raw = String::from_utf8(raw)
        .map_err(|e| format!("UnicodeDecodeError: {}", crate::py::decode_error(&e)))?;
    let payload = if strip(&raw).is_empty() {
        Obj::new()
    } else {
        match crate::jsonio::parse_or_skip(&raw).map_err(|e| describe(&e))? {
            Some(Value::Object(m)) => m,
            _ => Obj::new(),
        }
    };
    let mut camp = match Campaign::open(base, executor, paths) {
        Ok(c) => c, // rule 1
        Err(PmError::Usage(_)) => return Ok(None),
        Err(e) => return Err(describe(&e)),
    };
    let decision = {
        let _guard = camp.lock(Some(5.0)).map_err(|e| describe(&e))?;
        hook_decision(&mut camp, &payload).map_err(|e| describe(&e))?
    };
    let Some(decision) = decision else {
        return Ok(None);
    };
    let out = match (harness, decision) {
        ("claude", Decision::Guide(text)) => Value::Object(obj([(
            "hookSpecificOutput",
            Value::Object(obj([
                ("hookEventName", Value::from("Stop")),
                ("additionalContext", Value::from(text)),
            ])),
        )])),
        (_, Decision::Guide(text)) | (_, Decision::Block(text)) => Value::Object(obj([
            ("decision", Value::from("block")),
            ("reason", Value::from(text)),
        ])),
    };
    Ok(Some(compact(&out)))
}

/// Never fails: a non-zero exit would be read by the harness as a block.
pub fn stop_hook(base: &Path, harness: &str, executor: &str, paths: &Paths) {
    let result = std::panic::catch_unwind(|| run_hook(base, harness, executor, paths));
    let message = match result {
        Ok(Ok(None)) => return,
        Ok(Ok(Some(line))) => {
            let line = match crate::jsonio::encode(&line) {
                Ok(line) => line,
                Err(e) => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "[{TOOL}] stop-hook error, not blocking: {}",
                        describe(&e)
                    );
                    return;
                }
            };
            let mut stdout = std::io::stdout().lock();
            match writeln!(stdout, "{line}").and_then(|()| stdout.flush()) {
                Ok(()) => return,
                Err(e) => format!("OSError: {e}"),
            }
        }
        Ok(Err(text)) => text,
        Err(_) => "panic".to_string(),
    };
    let _ = writeln!(
        std::io::stderr(),
        "[{TOOL}] stop-hook error, not blocking: {message}"
    );
}
