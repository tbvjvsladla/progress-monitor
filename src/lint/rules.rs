//! The rules, applied to one target in a fixed order (the order of the findings).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value};

use super::{
    blank, exists, file_name, files_below, is_dir, is_file, load_json, one_of, read_text,
    read_utf8, sorted_entries, suffix, text_of, Findings, LintError, LintResult,
    DEFAULT_AUTO_CELLS, PLACEHOLDER, POINTER_KEYS, SKELETON_CELL, STATES,
};
use crate::jsonio;
use crate::paths::join;
use crate::py::{is_int, number_text, py_eq, py_repr, py_str, repr_str, splitlines, truthy};
use crate::time::{parse_time_detail, Moment, Parsed};
use crate::{FINAL, SECTIONS, SUFFIX, TOOL};

type Obj = Map<String, Value>;

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

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
        Ok(self.0.iter().any(|v| py_eq(v, value)))
    }
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
        let Ok(event) = jsonio::parse(line) else {
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

/// Whether a string value outside `_` keys holds the placeholder.
fn holds_placeholder(node: &Value) -> bool {
    match node {
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| !k.starts_with('_') && holds_placeholder(v)),
        Value::Array(items) => items.iter().any(holds_placeholder),
        Value::String(s) => s.contains(PLACEHOLDER),
        _ => false,
    }
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
            .fold(target.to_path_buf(), |p, part| p.join(part));
        let Ok(text) = read_text(&path)? else {
            continue;
        };
        if !text.contains(PLACEHOLDER) {
            continue;
        }
        let name = parts.last().map(String::as_str).unwrap_or_default();
        if suffix(name) == ".json" {
            if let Ok(value) = jsonio::parse(&text) {
                if !holds_placeholder(&value) {
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
            if let Ok(Value::Object(event)) = jsonio::parse(line) {
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
    let Value::Number(number) = budget else {
        return Ok(());
    };
    let limit = number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
        .unwrap_or(i128::MAX);
    if autos > limit + allowed {
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
                &cells_dir.join(name),
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
