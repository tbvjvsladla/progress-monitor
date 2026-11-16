//! `campaign.json`: the shape every later rule relies on. A declaration that does
//! not have it is unreadable (exit 2), checked in the baseline's order.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

use super::{
    blank, load_json, LintError, LintResult, CELL_STATES, DECLARATION_TEXT, ORIGINS, PLACEHOLDER,
};
use crate::py::{is_int, is_plain_name, py_eq, py_repr, py_str, repr_str};
use crate::{REVIEW, SCHEMA};

fn unreadable(message: impl Into<String>) -> LintError {
    LintError::Unreadable(format!("campaign.json: {}", message.into()))
}

/// A key that is absent or holds a list.
fn list_or_absent(entry: &serde_json::Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(Value::is_array)
}

fn string_in(value: Option<&Value>, default: &str, names: &[&str]) -> bool {
    match value {
        None => names.contains(&default),
        Some(Value::String(s)) => names.contains(&s.as_str()),
        Some(_) => false,
    }
}

/// Read and shape-check the declaration of `target`.
pub fn read_declaration(target: &Path) -> LintResult<Value> {
    let decl = load_json(&target.join("campaign.json"))?;
    let Value::Object(map) = &decl else {
        return Err(unreadable("must be a JSON object"));
    };
    let version = map.get("schema_version");
    if !version.is_some_and(|v| py_eq(v, &Value::from(SCHEMA))) {
        return Err(unreadable(format!(
            "schema_version must be {SCHEMA}, got {}",
            version.map_or("None".to_string(), py_repr)
        )));
    }
    for key in DECLARATION_TEXT {
        if blank(map.get(key)) {
            return Err(unreadable(format!("{key} must be a non-empty string")));
        }
    }
    let phases_ok = match map.get("phases") {
        Some(Value::Array(phases)) if !phases.is_empty() => {
            let distinct: HashSet<String> = phases.iter().map(py_str).collect();
            distinct.len() == phases.len()
                && phases
                    .iter()
                    .all(|p| matches!(p, Value::String(s) if is_plain_name(s)))
        }
        _ => false,
    };
    if !phases_ok {
        return Err(unreadable(
            "phases must be a non-empty list of distinct names",
        ));
    }
    let cap = map
        .get("review")
        .and_then(Value::as_object)
        .and_then(|r| r.get("round_cap"));
    let cap_ok = cap.is_some_and(|c| is_int(c) && c.as_i64().is_none_or(|n| n >= 1));
    if !cap_ok {
        return Err(unreadable("review.round_cap must be a positive integer"));
    }
    if map.get("budgets").is_some_and(|b| !b.is_object()) {
        return Err(unreadable("budgets must be an object"));
    }
    let target_ok = map
        .get("target")
        .and_then(Value::as_object)
        .is_some_and(|t| !blank(t.get("root")) && list_or_absent(t, "env_probe"));
    if !target_ok {
        return Err(unreadable("target needs root and an env_probe list"));
    }
    let Some(Value::Array(checks)) = map.get("checks") else {
        return Err(unreadable("checks must be a list"));
    };
    for entry in checks {
        let entry = match entry {
            Value::Object(e) if ["id", "cmd", "expect"].iter().all(|k| !blank(e.get(*k))) => e,
            _ => return Err(unreadable("each check needs id, cmd and expect")),
        };
        let id = repr_str(entry["id"].as_str().unwrap_or_default());
        if !entry.get("cwd").is_none_or(Value::is_string) {
            return Err(unreadable(format!("check {id} cwd must be a string")));
        }
        for key in ["inputs", "env", "covers"] {
            if !list_or_absent(entry, key) {
                return Err(unreadable(format!("check {id} {key} must be a list")));
            }
        }
    }
    let Some(Value::Array(groups)) = map.get("groups") else {
        return Err(unreadable("groups must be a list"));
    };
    let mut group_names = Vec::new();
    for entry in groups {
        let name = match entry {
            Value::Object(e)
                if !blank(e.get("group")) && e.get("checks").is_some_and(Value::is_array) =>
            {
                e["group"].as_str().unwrap_or_default()
            }
            _ => return Err(unreadable("each group needs group and a checks list")),
        };
        if !name.contains(PLACEHOLDER) && !is_plain_name(name) {
            return Err(unreadable(format!(
                "group name {} is not a plain file name",
                repr_str(name)
            )));
        }
        group_names.push(name);
    }
    if group_names.iter().collect::<HashSet<_>>().len() != group_names.len() {
        return Err(unreadable("group names must be distinct"));
    }
    let final_ok = map
        .get("final")
        .and_then(Value::as_object)
        .is_some_and(|f| f.get("checks").is_some_and(Value::is_array));
    if !final_ok {
        return Err(unreadable("final needs a checks list"));
    }
    let Some(Value::Array(cells)) = map.get("cells") else {
        return Err(unreadable("cells must be a list"));
    };
    let mut real = Vec::new();
    for entry in cells {
        let entry = match entry {
            Value::Object(e) if ["cell", "goal", "group"].iter().all(|k| !blank(e.get(*k))) => e,
            _ => return Err(unreadable("each cell needs cell, goal and group")),
        };
        let name = entry["cell"].as_str().unwrap_or_default();
        let shown = repr_str(name);
        if !string_in(entry.get("state"), "open", &CELL_STATES) {
            return Err(unreadable(format!(
                "cell {shown} state must be one of {}",
                CELL_STATES.join(", ")
            )));
        }
        if !string_in(entry.get("origin"), "plan", &ORIGINS) {
            return Err(unreadable(format!(
                "cell {shown} origin must be plan or auto"
            )));
        }
        if !matches!(
            entry.get("parent"),
            None | Some(Value::Null) | Some(Value::String(_))
        ) {
            return Err(unreadable(format!(
                "cell {shown} parent must be a cell name or null"
            )));
        }
        if !name.contains(PLACEHOLDER) && !is_plain_name(name) {
            return Err(unreadable(format!(
                "cell name {shown} is not a plain folder name"
            )));
        }
        if !string_in(entry.get("review"), "none", &REVIEW) {
            return Err(unreadable(format!(
                "cell {shown} review must be one of {}",
                REVIEW.join(", ")
            )));
        }
        if !list_or_absent(entry, "needs") {
            return Err(unreadable(format!("cell {shown} needs must be a list")));
        }
        if !name.contains(PLACEHOLDER) {
            real.push(name);
        }
    }
    if real.iter().collect::<HashSet<_>>().len() != real.len() {
        return Err(unreadable("cell names must be distinct"));
    }
    Ok(decl)
}
