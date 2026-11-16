//! The rules, applied to one target in a fixed order (the order of the findings).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value};

use super::{
    blank, exists, file_name, files_below, is_dir, is_file, join_text as join, load_json, one_of,
    os_name, parse_json, read_text, read_utf8, sorted_entries, suffix, text_of, Findings,
    LintError, LintResult, DEFAULT_AUTO_CELLS, PLACEHOLDER, POINTER_KEYS, SKELETON_CELL, STATES,
};
use crate::py::{
    is_int, number_text, py_eq, py_repr, py_str, repr_str, splitlines, truthy, type_name, Num,
};
use crate::time::{parse_time_detail, Moment, Parsed};
use crate::{FINAL, SECTIONS, SUFFIX, TOOL};

type Obj = Map<String, Value>;

/// A value a set can hold; a list or object stops the run as the baseline's TypeError did.
fn hashable(value: &Value) -> LintResult<&Value> {
    match value {
        Value::Array(_) | Value::Object(_) => Err(LintError::Internal(format!(
            "TypeError: unhashable type: '{}'",
            type_name(value)
        ))),
        _ => Ok(value),
    }
}

/// A set of JSON values with the baseline's equality (`1 == 1.0 == True`).
#[derive(Default)]
struct ValueSet(Vec<Value>);

impl ValueSet {
    fn insert(&mut self, value: &Value) -> LintResult<()> {
        if !self.contains(value)? {
            self.0.push(value.clone());
        }
        Ok(())
    }

    fn contains(&self, value: &Value) -> LintResult<bool> {
        let value = hashable(value)?;
        Ok(self.0.iter().any(|v| same_or_eq(v, value)))
    }
}

/// Set membership as the baseline saw it: identity first, then `==`. Every JSON
/// `NaN` the baseline read was the decoder's one NaN object, so two NaN values match.
fn same_or_eq(a: &Value, b: &Value) -> bool {
    let nan = |v: &Value| {
        matches!(v, Value::Number(_)) && Num::of(v).is_some_and(|n| n.as_f64().is_nan())
    };
    (nan(a) && nan(b)) || py_eq(a, b)
}

fn set_of<'a>(values: impl IntoIterator<Item = &'a Value>) -> LintResult<ValueSet> {
    let mut set = ValueSet::default();
    for value in values {
        set.insert(value)?;
    }
    Ok(set)
}

/// `value or []` iterated: a string gives its characters, an object its keys,
/// a true number or `true` stops the run (not iterable).
fn items_or_empty(value: Option<&Value>) -> LintResult<Vec<Value>> {
    match value {
        Some(v) if truthy(v) => match v {
            Value::Array(items) => Ok(items.clone()),
            Value::String(s) => Ok(s.chars().map(|c| Value::from(c.to_string())).collect()),
            Value::Object(m) => Ok(m.keys().map(|k| Value::from(k.clone())).collect()),
            other => Err(LintError::Internal(format!(
                "TypeError: '{}' object is not iterable",
                type_name(other)
            ))),
        },
        _ => Ok(Vec::new()),
    }
}

/// `needle in container` where the needle is a string: list items compare by
/// equality, object keys by name, a string by substring.
fn member_text(needle: &str, container: Option<&Value>) -> LintResult<bool> {
    match container {
        Some(v) if truthy(v) => match v {
            Value::Array(items) => Ok(items.iter().any(|i| i.as_str() == Some(needle))),
            Value::Object(m) => Ok(m.contains_key(needle)),
            Value::String(s) => Ok(s.contains(needle)),
            other => Err(LintError::Internal(format!(
                "TypeError: argument of type '{}' is not iterable",
                type_name(other)
            ))),
        },
        _ => Ok(false),
    }
}

fn repr_opt(value: Option<&Value>) -> String {
    value.map_or_else(|| "None".to_string(), py_repr)
}

fn str_list<'a>(decl: &'a Obj, key: &str) -> &'a [Value] {
    decl.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn entries<'a>(decl: &'a Obj, key: &str) -> impl Iterator<Item = &'a Obj> {
    str_list(decl, key).iter().filter_map(Value::as_object)
}

fn field<'a>(entry: &'a Obj, key: &str) -> &'a str {
    entry.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn list_field<'a>(entry: &'a Obj, key: &str) -> &'a [Value] {
    entry
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// One phase status file.
fn check_phase_file(
    path: &Path,
    rel: &str,
    add: &mut Findings,
    instance: Option<&Path>,
) -> LintResult<()> {
    let data = load_json(path)?;
    let Value::Object(data) = data else {
        return Err(LintError::Unreadable(format!(
            "{rel}: must be a JSON object"
        )));
    };
    let state = data.get("state");
    let empty_proof = Obj::new();
    let proof = data
        .get("proof")
        .and_then(Value::as_object)
        .unwrap_or(&empty_proof);
    if !one_of(state, &STATES) {
        add.add(
            "state-invalid",
            rel,
            format!("state is {}", repr_opt(state)),
        );
    }
    let empty: Vec<&str> = ["check", "expect"]
        .into_iter()
        .filter(|k| blank(proof.get(*k)))
        .collect();
    if !empty.is_empty() {
        add.add(
            "proof-blank",
            rel,
            format!("proof.{} empty", empty.join(" and proof.")),
        );
    }
    let state_text = state.and_then(Value::as_str).unwrap_or_default();
    if one_of(state, &["done", "failed"]) && blank(proof.get("source")) {
        add.add(
            "source-missing",
            rel,
            format!("state is {state_text} but proof.source is empty"),
        );
    }
    let done = state_text == "done" && state.is_some_and(Value::is_string);
    if done && proof.get("ok") != Some(&Value::Bool(true)) {
        add.add("done-not-ok", rel, "state is done but proof.ok is not true");
    }
    if let (true, Some(instance)) = (done, instance) {
        let by = data.get("by").and_then(Value::as_object);
        let log = by
            .filter(|b| b.get("tool").and_then(Value::as_str) == Some(TOOL))
            .and_then(|b| text_of(b.get("log")));
        match log {
            None => add.add(
                "done-unrecorded",
                rel,
                "state is done but the executor record (by) is absent",
            ),
            Some(log) if !is_file(&join(instance, log)) => add.add(
                "done-unrecorded",
                rel,
                format!("the executor log does not exist: {log}"),
            ),
            Some(_) => {}
        }
    }
    Ok(())
}

/// cell.md: six sections, none empty.
fn check_cell_summary(path: &Path, rel: &str, add: &mut Findings) -> LintResult<()> {
    let text = read_utf8(path)?;
    let mut bodies: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;
    for line in splitlines(&text) {
        if let Some(rest) = line.strip_prefix("## ") {
            let name = crate::py::strip(rest).to_string();
            bodies.entry(name.clone()).or_default();
            current = Some(name);
        } else if let Some(name) = &current {
            bodies.entry(name.clone()).or_default().push_str(line);
        }
    }
    for section in SECTIONS {
        match bodies.get(section) {
            None => add.add(
                "cell-summary-incomplete",
                rel,
                format!("section '{section}' missing"),
            ),
            Some(body) if crate::py::strip(body).is_empty() => add.add(
                "cell-summary-incomplete",
                rel,
                format!("section '{section}' is empty"),
            ),
            Some(_) => {}
        }
    }
    Ok(())
}

/// One cell folder: its summary and its phase files.
fn check_cell(
    cell_dir: &Path,
    rel: &str,
    phases: &[String],
    add: &mut Findings,
    instance: Option<&Path>,
) -> LintResult<()> {
    let summary = cell_dir.join("cell.md");
    if is_file(&summary) {
        check_cell_summary(&summary, &format!("{rel}/cell.md"), add)?;
    } else {
        add.add(
            "declared-missing",
            format!("{rel}/cell.md"),
            "cell.md missing",
        );
    }
    let phase_dir = cell_dir.join("phases");
    let mut present: Vec<(String, std::path::PathBuf)> = Vec::new();
    if is_dir(&phase_dir) {
        for path in sorted_entries(&phase_dir)? {
            let name = file_name(&path);
            match name.strip_suffix(SUFFIX) {
                Some(phase) if is_file(&path) => present.push((phase.to_string(), path)),
                _ => add.add(
                    "undeclared",
                    format!("{rel}/phases/{name}"),
                    "not a phase status file",
                ),
            }
        }
    }
    for (name, path) in &present {
        let shown = format!("{rel}/phases/{}", file_name(path));
        if phases.contains(name) {
            check_phase_file(path, &shown, add, instance)?;
        } else {
            add.add(
                "undeclared",
                shown,
                format!("phase '{name}' is not declared"),
            );
        }
    }
    for phase in phases {
        if !present.iter().any(|(name, _)| name == phase) {
            add.add(
                "declared-missing",
                format!("{rel}/phases/{phase}{SUFFIX}"),
                format!("no status file for phase '{phase}'"),
            );
        }
    }
    Ok(())
}

/// journey.jsonl: objects with utc and event, times never going backward.
fn check_journey(path: &Path, add: &mut Findings) -> LintResult<()> {
    let rel = "journey.jsonl";
    if !is_file(path) {
        add.add("journey-invalid", rel, "journey.jsonl missing");
        return Ok(());
    }
    let text = read_utf8(path)?;
    let mut previous: Option<Moment> = None;
    for (index, line) in splitlines(&text).into_iter().enumerate() {
        let at = format!("{rel}:{}", index + 1);
        let Some(event) = parse_json(line)? else {
            add.add("journey-invalid", at, "line is not JSON");
            continue;
        };
        let utc = match &event {
            Value::Object(e) if !blank(e.get("event")) => text_of(e.get("utc")),
            _ => None,
        };
        let Some(utc) = utc else {
            add.add("journey-invalid", at, "line needs utc and event");
            continue;
        };
        let moment = match parse_time_detail(utc) {
            Parsed::Invalid => {
                add.add("journey-invalid", at, "utc is not an ISO 8601 time");
                continue;
            }
            Parsed::Naive => {
                add.add("journey-invalid", at, "utc needs a time zone");
                continue;
            }
            Parsed::Aware(moment) => moment,
        };
        if previous.is_some_and(|p| moment < p) {
            add.add("journey-invalid", at, "time goes backward");
        }
        previous = Some(moment);
    }
    Ok(())
}

/// evidence_pointers.json: known keys, kind and path given, path present.
fn check_pointers(path: &Path, root: &Path, add: &mut Findings) -> LintResult<()> {
    let rel = "evidence_pointers.json";
    if !is_file(path) {
        add.add("pointer-invalid", rel, "evidence_pointers.json missing");
        return Ok(());
    }
    let data = load_json(path)?;
    let Some(Value::Array(pointers)) = data.as_object().and_then(|d| d.get("pointers")) else {
        add.add("pointer-invalid", rel, "pointers must be a list");
        return Ok(());
    };
    for (index, pointer) in pointers.iter().enumerate() {
        let at = format!("{rel}#pointers[{index}]");
        let Value::Object(pointer) = pointer else {
            add.add("pointer-invalid", at, "pointer must be an object");
            continue;
        };
        let extra: BTreeSet<&str> = pointer
            .keys()
            .map(String::as_str)
            .filter(|k| !POINTER_KEYS.contains(k))
            .collect();
        if !extra.is_empty() {
            let extra: Vec<&str> = extra.into_iter().collect();
            add.add(
                "pointer-invalid",
                at.clone(),
                format!("keys outside kind/path/cell/note: {}", extra.join(", ")),
            );
        }
        let location = match (text_of(pointer.get("kind")), text_of(pointer.get("path"))) {
            (Some(_), Some(location)) => location,
            _ => {
                add.add("pointer-invalid", at, "kind and path are required");
                continue;
            }
        };
        if !exists(&join(root, location)) {
            add.add(
                "pointer-invalid",
                at,
                format!("path does not exist: {location}"),
            );
        }
    }
    Ok(())
}

/// Nesting level (the parsed root is level 0) at which the baseline's recursive
/// placeholder walk ran out of interpreter stack (`RecursionError`, exit 1).
const PLACEHOLDER_WALK_LEVELS: usize = 995;

/// Whether a string value outside `_` keys holds the placeholder, visiting
/// values in document order and stopping at the first hit like the baseline.
fn holds_placeholder(root: &Value, level: usize) -> LintResult<bool> {
    let mut pending: Vec<(&Value, usize)> = vec![(root, level)];
    while let Some((node, level)) = pending.pop() {
        if level >= PLACEHOLDER_WALK_LEVELS {
            return Err(LintError::Internal(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        match node {
            Value::Object(map) => pending.extend(
                map.iter()
                    .rev()
                    .filter(|(key, _)| !key.starts_with('_'))
                    .map(|(_, value)| (value, level + 1)),
            ),
            Value::Array(items) => pending.extend(items.iter().rev().map(|v| (v, level + 1))),
            Value::String(s) if s.contains(PLACEHOLDER) => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

/// (instance only) placeholders and the skeleton cell left behind.
fn check_placeholders(target: &Path, add: &mut Findings) -> LintResult<()> {
    if exists(&target.join("cells").join(SKELETON_CELL)) {
        add.add(
            "placeholder-left",
            format!("cells/{SKELETON_CELL}"),
            "skeleton cell folder remains",
        );
    }
    for parts in files_below(target)? {
        let rel = parts.join("/");
        if rel.starts_with("logs/") {
            continue;
        }
        let path = parts
            .iter()
            .fold(target.to_path_buf(), |p, part| p.join(os_name(part)));
        let Ok(text) = read_text(&path)? else {
            continue;
        };
        if !text.contains(PLACEHOLDER) {
            continue;
        }
        let name = parts.last().map(String::as_str).unwrap_or_default();
        if suffix(name) == ".json" {
            if let Some(value) = parse_json(&text)? {
                if !holds_placeholder(&value, 0)? {
                    continue;
                }
            }
        }
        add.add("placeholder-left", rel, "placeholder remains");
    }
    Ok(())
}

/// The journey lines that are JSON objects.
pub fn read_events(path: &Path) -> LintResult<Vec<Obj>> {
    let mut events = Vec::new();
    if is_file(path) {
        let text = read_utf8(path)?;
        for line in splitlines(&text) {
            if let Some(Value::Object(event)) = parse_json(line)? {
                events.push(event);
            }
        }
    }
    Ok(events)
}

/// (instance only) every reference names something declared.
fn check_refs(decl: &Obj, add: &mut Findings) -> LintResult<()> {
    let rel = "campaign.json";
    let check_ids = set_of(entries(decl, "checks").filter_map(|e| e.get("id")))?;
    let groups = set_of(entries(decl, "groups").filter_map(|e| e.get("group")))?;
    let cells = set_of(entries(decl, "cells").filter_map(|e| e.get("cell")))?;
    for entry in entries(decl, "checks") {
        for covered in list_field(entry, "covers") {
            if !check_ids.contains(covered)? {
                add.add(
                    "ref-invalid",
                    rel,
                    format!(
                        "check {} covers unknown check {}",
                        repr_str(field(entry, "id")),
                        py_repr(covered)
                    ),
                );
            }
        }
    }
    for entry in entries(decl, "groups") {
        for name in list_field(entry, "checks") {
            if !check_ids.contains(name)? {
                add.add(
                    "ref-invalid",
                    rel,
                    format!(
                        "group {} names unknown check {}",
                        repr_str(field(entry, "group")),
                        py_repr(name)
                    ),
                );
            }
        }
    }
    let final_checks = decl
        .get("final")
        .and_then(Value::as_object)
        .map_or(&[][..], |f| list_field(f, "checks"));
    for name in final_checks {
        if !check_ids.contains(name)? {
            add.add(
                "ref-invalid",
                rel,
                format!("final names unknown check {}", py_repr(name)),
            );
        }
    }
    for entry in entries(decl, "cells") {
        let cell = repr_str(field(entry, "cell"));
        let group = &entry["group"];
        if !groups.contains(group)? {
            add.add(
                "ref-invalid",
                rel,
                format!("cell {cell} is in unknown group {}", py_repr(group)),
            );
        }
        if let Some(parent) = entry.get("parent").filter(|p| !p.is_null()) {
            if !cells.contains(parent)? {
                add.add(
                    "ref-invalid",
                    rel,
                    format!("cell {cell} has unknown parent {}", py_repr(parent)),
                );
            }
        }
        for need in list_field(entry, "needs") {
            if !cells.contains(need)? {
                add.add(
                    "ref-invalid",
                    rel,
                    format!("cell {cell} needs unknown cell {}", py_repr(need)),
                );
            }
        }
    }
    Ok(())
}

/// (instance only) a check id twice, or two checks with the same cmd, cwd and env.
fn check_duplicates(decl: &Obj, add: &mut Findings) {
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut seen_commands: HashMap<(String, String, Vec<String>), &str> = HashMap::new();
    for entry in entries(decl, "checks") {
        let id = field(entry, "id");
        if !seen_ids.insert(id) {
            add.add(
                "check-duplicate",
                "campaign.json",
                format!("check id {} is declared twice", repr_str(id)),
            );
            continue;
        }
        let mut env: Vec<String> = list_field(entry, "env").iter().map(py_str).collect();
        env.sort();
        let cwd = entry.get("cwd").and_then(Value::as_str).unwrap_or(".");
        let command = (field(entry, "cmd").to_string(), cwd.to_string(), env);
        match seen_commands.get(&command) {
            Some(first) => add.add(
                "check-duplicate",
                "campaign.json",
                format!(
                    "checks {} and {} have the same cmd, cwd and env",
                    repr_str(first),
                    repr_str(id)
                ),
            ),
            None => {
                seen_commands.insert(command, id);
            }
        }
    }
}

fn is_event(event: &Obj, kind: &str) -> bool {
    event.get("event").and_then(Value::as_str) == Some(kind)
}

/// (instance only) replaying the structural events gives the declared layout.
fn check_structure(decl: &Obj, events: &[Obj], add: &mut Findings) -> LintResult<()> {
    if !events.iter().any(|e| is_event(e, "campaign-open")) {
        return Ok(());
    }
    let mut layout: HashMap<String, Value> = HashMap::new();
    let mut cancelled: BTreeSet<String> = BTreeSet::new();
    let mut started = false;
    let group_of = |event: &Obj, key: &str| event.get(key).cloned().unwrap_or(Value::Null);
    for event in events {
        if is_event(event, "campaign-open") && !started {
            started = true;
            for item in items_or_empty(event.get("cells"))? {
                if let Value::Object(item) = &item {
                    if let Some(cell) = text_of(item.get("cell")) {
                        layout.insert(cell.to_string(), group_of(item, "group"));
                    }
                }
            }
            continue;
        }
        if !started {
            continue;
        }
        let Some(cell) = text_of(event.get("cell")) else {
            continue;
        };
        if is_event(event, "cell-add") {
            layout.insert(cell.to_string(), group_of(event, "group"));
        } else if is_event(event, "cell-cancel") {
            cancelled.insert(cell.to_string());
        } else if is_event(event, "regroup") {
            layout.insert(cell.to_string(), group_of(event, "to"));
        }
    }
    let declared: HashMap<&str, &str> = entries(decl, "cells")
        .map(|e| (field(e, "cell"), field(e, "group")))
        .collect();
    let declared_cancelled: BTreeSet<String> = entries(decl, "cells")
        .filter(|e| e.get("state").and_then(Value::as_str) == Some("cancelled"))
        .map(|e| field(e, "cell").to_string())
        .collect();
    let rel = "campaign.json";
    let names: BTreeSet<&str> = declared
        .keys()
        .copied()
        .chain(layout.keys().map(String::as_str))
        .collect();
    for cell in names {
        let shown = repr_str(cell);
        match (declared.get(cell), layout.get(cell)) {
            (_, None) => add.add(
                "structure-unlogged",
                rel,
                format!("cell {shown} has no campaign-open or cell-add event"),
            ),
            (None, Some(_)) => add.add(
                "structure-unlogged",
                rel,
                format!("cell {shown} is in the journey but not in the declaration"),
            ),
            (Some(group), Some(logged)) if !py_eq(&Value::from(*group), logged) => add.add(
                "structure-unlogged",
                rel,
                format!(
                    "cell {shown} is in group {} but the journey gives {}",
                    repr_str(group),
                    py_repr(logged)
                ),
            ),
            _ => {}
        }
    }
    for cell in declared_cancelled.symmetric_difference(&cancelled) {
        add.add(
            "structure-unlogged",
            rel,
            format!(
                "cell {}: cancelled state and cell-cancel events disagree",
                repr_str(cell)
            ),
        );
    }
    Ok(())
}

/// (instance only) auto cells within the budget plus allowed auto-cell-cap answers.
fn check_auto_cap(decl: &Obj, events: &[Obj], add: &mut Findings) -> LintResult<()> {
    let default = Value::from(DEFAULT_AUTO_CELLS);
    let budget = decl
        .get("budgets")
        .and_then(Value::as_object)
        .and_then(|b| b.get("auto_cells"))
        .unwrap_or(&default);
    if !is_int(budget) {
        return Ok(());
    }
    let mut cap_ids = ValueSet::default();
    for event in events {
        if is_event(event, "hitl")
            && event.get("kind").and_then(Value::as_str) == Some("auto-cell-cap")
        {
            cap_ids.insert(event.get("id").unwrap_or(&Value::Null))?;
        }
    }
    let mut allowed: i128 = 0;
    for event in events {
        if is_event(event, "hitl-answer")
            && event.get("allow") == Some(&Value::Bool(true))
            && cap_ids.contains(event.get("id").unwrap_or(&Value::Null))?
        {
            allowed += 1;
        }
    }
    let autos = entries(decl, "cells")
        .filter(|e| e.get("origin").and_then(Value::as_str) == Some("auto"))
        .count() as i128;
    let Some(limit) = Num::of(budget) else {
        return Ok(());
    };
    let Value::Number(number) = budget else {
        return Ok(());
    };
    // `autos > budget + allowed` exactly; an int beyond i128 goes on as a float.
    if Num::Int(autos).gt(limit.plus(Num::Int(allowed))) {
        add.add(
            "auto-cap-exceeded",
            "campaign.json",
            format!(
                "{autos} auto cells, budget {} plus {allowed} allowed",
                number_text(number)
            ),
        );
    }
    Ok(())
}

/// A gate status file, if it is there.
fn load_gate(target: &Path, group: &str) -> LintResult<Option<Obj>> {
    let path = join(&target.join("gates"), &format!("{group}{SUFFIX}"));
    if !is_file(&path) {
        return Ok(None);
    }
    match load_json(&path)? {
        Value::Object(gate) => Ok(Some(gate)),
        _ => Err(LintError::Unreadable(format!(
            "gates/{}: must be a JSON object",
            file_name(&path)
        ))),
    }
}

fn passed(gate: &Obj) -> bool {
    gate.get("state").and_then(Value::as_str) == Some("passed")
}

/// (instance only) closed cells have a passed gate; a passed gate proves every check.
fn check_gates(target: &Path, decl: &Obj, add: &mut Findings) -> LintResult<()> {
    for entry in entries(decl, "cells") {
        if entry.get("state").and_then(Value::as_str) != Some("closed") {
            continue;
        }
        let group = field(entry, "group");
        let cell = field(entry, "cell");
        let gate = load_gate(target, group)?;
        let rel = format!("gates/{group}{SUFFIX}");
        match gate {
            Some(gate) if passed(&gate) => {
                if !member_text(cell, gate.get("members"))? {
                    add.add(
                        "closed-without-gate",
                        rel,
                        format!("cell {} is closed but is not a gate member", repr_str(cell)),
                    );
                }
            }
            _ => add.add(
                "closed-without-gate",
                rel,
                format!(
                    "cell {} is closed but the gate is not passed",
                    repr_str(cell)
                ),
            ),
        }
    }
    let mut declared: HashMap<&str, &[Value]> = entries(decl, "groups")
        .map(|e| (field(e, "group"), list_field(e, "checks")))
        .collect();
    let final_checks = decl
        .get("final")
        .and_then(Value::as_object)
        .map_or(&[][..], |f| list_field(f, "checks"));
    declared.insert(FINAL, final_checks);
    let gates_dir = target.join("gates");
    if !is_dir(&gates_dir) {
        return Ok(());
    }
    for path in sorted_entries(&gates_dir)? {
        let name = file_name(&path);
        let Some(group) = name.strip_suffix(SUFFIX) else {
            continue;
        };
        let Some(gate) = load_gate(target, group)? else {
            continue;
        };
        if !passed(&gate) {
            continue;
        }
        let rel = format!("gates/{name}");
        let results: Vec<Obj> = items_or_empty(gate.get("checks"))?
            .into_iter()
            .filter_map(|item| match item {
                Value::Object(item) => Some(item),
                _ => None,
            })
            .collect();
        for item in &results {
            let outcome = item.get("outcome");
            let check = repr_opt(item.get("check"));
            let proven = one_of(outcome, &["reused", "covered"])
                || (one_of(outcome, &["ran", "forced"])
                    && item.get("result").and_then(Value::as_str) == Some("PASS"));
            if !proven {
                add.add(
                    "gate-not-proven",
                    rel.clone(),
                    format!(
                        "check {check} is {}/{} in a passed gate",
                        repr_opt(outcome),
                        repr_opt(item.get("result"))
                    ),
                );
                continue;
            }
            let key = if one_of(outcome, &["covered"]) {
                "via"
            } else {
                "receipt"
            };
            let pointer = item.get(key);
            let present = text_of(pointer).is_some_and(|p| is_file(&join(target, p)));
            if !present {
                add.add(
                    "gate-not-proven",
                    rel.clone(),
                    format!(
                        "check {check} points at a missing receipt: {}",
                        repr_opt(pointer)
                    ),
                );
            }
        }
        let recorded = set_of(
            results
                .iter()
                .map(|item| item.get("check").unwrap_or(&Value::Null)),
        )?;
        for name in declared.get(group).copied().unwrap_or_default() {
            if !recorded.contains(name)? {
                add.add(
                    "gate-not-proven",
                    rel.clone(),
                    format!("check {} has no result in a passed gate", py_repr(name)),
                );
            }
        }
    }
    Ok(())
}

/// Check `target` (a campaign folder). `root` resolves relative pointer paths.
/// `template` selects template mode. Unreadable input is an error, not a finding.
pub fn check(target: &Path, root: &Path, template: bool) -> LintResult<Vec<super::Finding>> {
    if !is_dir(target) {
        return Err(LintError::Unreadable(format!(
            "{}: not a directory",
            target.display()
        )));
    }
    let mut add = Findings::default();
    let decl = super::declaration::read_declaration(target)?;
    let decl = decl.as_object().cloned().unwrap_or_default();
    let phases: Vec<String> = str_list(&decl, "phases")
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    let cells_dir = target.join("cells");
    let mut folders: Vec<String> = Vec::new();
    if is_dir(&cells_dir) {
        for path in sorted_entries(&cells_dir)? {
            if is_dir(&path) {
                folders.push(file_name(&path));
            }
        }
    }
    let expected: Vec<String> = if template {
        vec![SKELETON_CELL.to_string()]
    } else {
        if str_list(&decl, "cells").is_empty() {
            add.add("no-cells", "campaign.json", "the declaration has no cells");
        }
        entries(&decl, "cells")
            .map(|e| field(e, "cell"))
            .filter(|name| !name.contains(PLACEHOLDER))
            .map(str::to_string)
            .collect()
    };
    let instance = if template { None } else { Some(target) };
    for name in &expected {
        if folders.contains(name) {
            check_cell(
                &cells_dir.join(os_name(name)),
                &format!("cells/{name}"),
                &phases,
                &mut add,
                instance,
            )?;
        } else {
            add.add(
                "declared-missing",
                format!("cells/{name}"),
                "cell folder missing",
            );
        }
    }
    for name in &folders {
        if !expected.contains(name) && (template || name != SKELETON_CELL) {
            add.add(
                "undeclared",
                format!("cells/{name}"),
                "cell folder is not declared",
            );
        }
    }
    check_journey(&target.join("journey.jsonl"), &mut add)?;
    check_pointers(&target.join("evidence_pointers.json"), root, &mut add)?;
    if !template {
        check_placeholders(target, &mut add)?;
        let events = read_events(&target.join("journey.jsonl"))?;
        check_refs(&decl, &mut add)?;
        check_duplicates(&decl, &mut add);
        check_structure(&decl, &events, &mut add)?;
        check_auto_cap(&decl, &events, &mut add)?;
        check_gates(target, &decl, &mut add)?;
    }
    Ok(add.0)
}

#[cfg(test)]
mod tests {
    //! Verdicts pinned against the baseline (review round 1, K2 and K7).
    use super::*;
    use crate::jsonio;
    use crate::lint::control::build_valid_instance;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A fresh valid instance under the system temp folder: (root, instance).
    fn valid_instance() -> (PathBuf, PathBuf) {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("pm-lint-rules-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/campaigns/_template");
        let instance = build_valid_instance(&template, &root).ok().unwrap();
        (root, instance)
    }

    fn findings(root: &Path, instance: &Path) -> Vec<super::super::Finding> {
        let found = check(instance, root, false).ok().unwrap();
        fs::remove_dir_all(root).unwrap();
        found
    }

    /// Insert `text` right after the opening brace of a JSON object file.
    fn insert_after_brace(path: &Path, text: &str) {
        let old = fs::read_to_string(path).unwrap();
        let at = old.find('{').unwrap() + 1;
        fs::write(path, format!("{}{text}{}", &old[..at], &old[at..])).unwrap();
    }

    #[test]
    fn non_finite_numbers_in_a_declaration_pass_like_the_baseline() {
        let (root, instance) = valid_instance();
        insert_after_brace(
            &instance.join("campaign.json"),
            "\n  \"NaN\": NaN, \"_inf\": [Infinity, -Infinity],",
        );
        let status = fs::read_dir(instance.join("cells/alpha/phases"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().ends_with(".status.json"))
            .unwrap();
        insert_after_brace(&status, "\n  \"_n\": NaN,");
        assert_eq!(findings(&root, &instance), vec![]);
    }

    #[test]
    fn deep_placeholder_walk_ends_like_the_baseline() {
        // Reading and dropping a 3000-deep value recurses; give it room.
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(deep_placeholder_walk)
            .unwrap()
            .join()
            .unwrap();
    }

    fn deep_placeholder_walk() {
        let nested = |n: usize, leaf: &str| format!("{}{leaf}{}", "[".repeat(n), "]".repeat(n));
        let walk = |text: String| holds_placeholder(&jsonio::parse(&text).ok().unwrap(), 0);
        let note =
            |n: usize, leaf: &str| format!("{{\"_p\": \"<<FILL>>\", \"d\": {}}}", nested(n, leaf));
        assert!(matches!(walk(note(993, "1")), Ok(false)));
        assert!(matches!(walk(note(994, "1")), Err(LintError::Internal(_))));
        assert!(matches!(walk(nested(994, "\"<<FILL>>\"")), Ok(true)));
        assert!(matches!(
            walk(nested(995, "\"<<FILL>>\"")),
            Err(LintError::Internal(_))
        ));
        let early = format!("{{\"a\": \"<<FILL>>\", \"d\": {}}}", nested(3000, ""));
        assert!(matches!(walk(early), Ok(true)));
    }

    /// Append a copy of the last journey event with `utc` replaced.
    fn journey_verdict(utc: &str) -> Vec<String> {
        let (root, instance) = valid_instance();
        let path = instance.join("journey.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let last = text.lines().last().unwrap();
        let mut event = jsonio::parse(last).ok().unwrap();
        event["utc"] = Value::from(utc);
        fs::write(&path, format!("{text}{}\n", jsonio::compact(&event))).unwrap();
        findings(&root, &instance)
            .into_iter()
            .map(|f| format!("{} {} {}", f.rule, f.path, f.message))
            .collect()
    }

    #[test]
    fn journey_times_the_baseline_accepts() {
        for utc in [
            "2027-01-01T00:00:05 +00:00",
            "2027-01-01T00:00:05+0000",
            "2027-01-01 00:00:05Z",
            "20270101T000005Z",
            "2027-01-01T00:00:05+00",
            "2027-W01-1T00:00:05+00:00",
            "2027-01-01T00:00:05,5+00:00",
            "2027-01-01T00:00:05.1234567+00:00",
            "2027-01-01T00:00:05+00:00:30.5",
            "2027-01-01T00Z",
            "2027-01-01T00:00:05 -05:30",
        ] {
            assert_eq!(journey_verdict(utc), Vec::<String>::new(), "{utc}");
        }
    }

    #[test]
    fn journey_times_the_baseline_rejects() {
        let line = "journey-invalid journey.jsonl:8";
        for (utc, message) in [
            ("2027-01-01T00:00:05", "utc needs a time zone"),
            ("2027-01-01", "utc needs a time zone"),
            ("2027-01-01T24:00:00+00:00", "utc is not an ISO 8601 time"),
            ("2027-01-01T00:00:05+25:00", "utc is not an ISO 8601 time"),
            ("2027-02-29T00:00:00Z", "utc is not an ISO 8601 time"),
            (" 2027-01-01T00:00:05Z", "utc is not an ISO 8601 time"),
            ("2027-01-01T00:00:05z", "utc is not an ISO 8601 time"),
            (
                "\u{ff12}027-01-01T00:00:05+00:00",
                "utc is not an ISO 8601 time",
            ),
            ("2026-01-01T00:00:05 +00:00", "time goes backward"),
        ] {
            assert_eq!(
                journey_verdict(utc),
                vec![format!("{line} {message}")],
                "{utc}"
            );
        }
    }

    /// Set one key of `campaign.json` from raw JSON text.
    fn set_declared(instance: &Path, pointer: &[&str], raw: &str) {
        let path = instance.join("campaign.json");
        let mut decl = jsonio::parse(&fs::read_to_string(&path).unwrap())
            .ok()
            .unwrap();
        let mut slot = &mut decl;
        for key in pointer {
            slot = &mut slot[*key];
        }
        *slot = jsonio::parse(raw).ok().unwrap();
        fs::write(&path, jsonio::compact(&decl)).unwrap();
    }

    /// Review round 2 (r2-3): `isinstance(cap, int) and cap >= 1` for every int size.
    #[test]
    fn round_cap_is_a_positive_int_of_any_size() {
        for (raw, ok) in [
            ("-100000000000000000000", false),
            ("-9223372036854775809", false),
            ("-170141183460469231731687303715884105729", false),
            ("-1e20", false),
            ("0", false),
            ("-0", false),
            ("1.0", false),
            ("true", false),
            ("NaN", false),
            ("1", true),
            ("100000000000000000000000000000000000000000", true),
        ] {
            let (root, instance) = valid_instance();
            set_declared(&instance, &["review", "round_cap"], raw);
            let verdict = check(&instance, &root, false);
            fs::remove_dir_all(&root).unwrap();
            match verdict {
                Ok(_) => assert!(ok, "{raw} passed"),
                Err(LintError::Unreadable(m)) => {
                    assert!(!ok, "{raw}: {m}");
                    assert_eq!(
                        m,
                        "campaign.json: review.round_cap must be a positive integer"
                    );
                }
                Err(_) => panic!("{raw}: not an unreadable declaration"),
            }
        }
    }

    /// `autos > budget + allowed` exactly, whatever the int size.
    #[test]
    fn auto_cell_budget_of_any_int_size() {
        for (raw, exceeded) in [
            ("-100000000000000000000", true),
            ("-170141183460469231731687303715884105729", true),
            ("-1", true),
            ("100000000000000000000000000000000000000000", false),
            ("NaN", false),
            ("-1e20", false),
        ] {
            let (root, instance) = valid_instance();
            set_declared(&instance, &["budgets", "auto_cells"], raw);
            let rules: Vec<&str> = findings(&root, &instance)
                .into_iter()
                .map(|f| f.rule)
                .collect();
            assert_eq!(rules.contains(&"auto-cap-exceeded"), exceeded, "{raw}");
        }
    }

    /// Every JSON NaN the baseline read was one object, so set membership matched it.
    #[test]
    fn nan_matches_itself_in_a_set_and_is_a_float() {
        let nan = jsonio::parse("NaN").ok().unwrap();
        let set = set_of([&nan]).ok().unwrap();
        assert!(set
            .contains(&jsonio::parse("NaN").ok().unwrap())
            .ok()
            .unwrap());
        assert!(!set.contains(&Value::from(0)).ok().unwrap());
        for raw in ["NaN", "Infinity", "1e400"] {
            let value = jsonio::parse(raw).ok().unwrap();
            let Err(LintError::Internal(m)) = member_text("x", Some(&value)) else {
                panic!("{raw}");
            };
            assert_eq!(m, "TypeError: argument of type 'float' is not iterable");
        }
    }
}
