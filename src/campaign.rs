//! One campaign instance: its declaration, phase files, gate records, monitor,
//! receipts, journey and the input fingerprint of a check.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{PmError, Res};
use crate::jsonio::{compact, digest, read_json, write_json};
use crate::lock::{self, LockGuard};
use crate::paths::{join, Paths};
use crate::proc::{git, node_name, probe};
use crate::py::{
    as_list, as_obj, as_obj_mut, contains, eq_str, is_digits, is_int, is_plain_name, list_or_empty,
    need, num, obj, opt_truthy, py_eq, py_str, repr_str, splitlines, Num, Obj,
};
use crate::time::{now, parse_time, stamp, Moment};
use crate::{FINAL, SCHEMA, SUFFIX};

pub const DEFAULT_BUDGETS: [(&str, i64); 11] = [
    ("stall_stops", 2),
    ("stop_block_cap", 6),
    ("stall_retries", 1),
    ("phase_minutes", 90),
    ("auto_cells", 5),
    ("forced_reruns_per_check", 1),
    ("forced_reruns_total", 3),
    ("exec_retries", 2),
    ("check_timeout_s", 1800),
    ("explore_candidates", 3),
    ("explore_rounds", 6),
];

pub fn default_monitor() -> Obj {
    match json!({
        "session": null, "digest": null, "unchanged": 0, "blocks": 0, "released": false,
        "stalls": {}, "refused_since": 0, "total_blocks": 0, "last_block_utc": null, "last_brief_utc": null,
    }) {
        Value::Object(map) => map,
        _ => Obj::new(),
    }
}

/// The input fingerprint of a check (rule 1).
#[derive(Clone, Debug)]
pub struct Fingerprint {
    pub key: Value,
    pub sha: String,
    pub reusable: bool,
    pub why: Option<&'static str>,
}

impl Fingerprint {
    pub fn inputs(&self) -> Option<&Value> {
        self.key.get("inputs")
    }
}

pub struct Campaign {
    pub base: PathBuf,
    pub dir: PathBuf,
    pub executor: String,
    pub decl: Obj,
    pub root: PathBuf,
    pub paths: Paths,
    probe_cache: HashMap<String, Obj>,
    ids_cache: HashMap<(String, String), Option<String>>,
    events_cache: Option<Vec<Obj>>,
}

impl Campaign {
    pub fn open(base: &Path, executor: &str, paths: &Paths) -> Res<Campaign> {
        let active = base.join("ACTIVE");
        if !active.is_file() {
            return Err(PmError::usage(format!(
                "{}: no active campaign",
                active.display()
            )));
        }
        let raw = fs::read(&active).map_err(|e| PmError::os(&e))?;
        let text = String::from_utf8(raw)
            .map_err(|e| PmError::Internal(format!("UnicodeDecodeError: {e}")))?;
        let name = crate::py::strip(&text).to_string();
        if name.is_empty() || !is_plain_name(&name) {
            return Err(PmError::usage(format!(
                "{}: not an instance name",
                active.display()
            )));
        }
        let dir = base.join(&name);
        if !dir.is_dir() {
            return Err(PmError::usage(format!(
                "{}: instance folder missing",
                dir.display()
            )));
        }
        let mut camp = Campaign {
            base: base.to_path_buf(),
            dir,
            executor: executor.to_string(),
            decl: Obj::new(),
            root: PathBuf::new(),
            paths: paths.clone(),
            probe_cache: HashMap::new(),
            ids_cache: HashMap::new(),
            events_cache: None,
        };
        camp.reload()?;
        Ok(camp)
    }

    // -- declaration
    pub fn reload(&mut self) -> Res<()> {
        let decl = read_json(&self.dir.join("campaign.json"), None)?;
        let Value::Object(decl) = decl else {
            return Err(PmError::usage(format!(
                "campaign.json: schema_version must be {SCHEMA}"
            )));
        };
        if !decl
            .get("schema_version")
            .is_some_and(|v| py_eq(v, &Value::from(SCHEMA)))
        {
            return Err(PmError::usage(format!(
                "campaign.json: schema_version must be {SCHEMA}"
            )));
        }
        for (key, is_obj) in [
            ("target", true),
            ("phases", false),
            ("checks", false),
            ("groups", false),
            ("final", true),
            ("cells", false),
        ] {
            let ok = match decl.get(key) {
                Some(Value::Object(_)) => is_obj,
                Some(Value::Array(_)) => !is_obj,
                _ => false,
            };
            if !ok {
                return Err(PmError::usage(format!(
                    "campaign.json: {key} is missing or malformed"
                )));
            }
        }
        let root_text =
            crate::jsonio::os_text(&decl["target"].get("root").map(py_str).unwrap_or_default());
        self.root = if Path::new(&root_text).is_absolute() {
            join(Path::new("/"), &root_text)
        } else {
            join(self.base.parent().unwrap_or(Path::new("/")), &root_text)
        };
        self.decl = decl;
        self.events_cache = None;
        self.ids_cache.clear();
        Ok(())
    }

    /// Take the instance lock and re-read the declaration.
    pub fn lock(&mut self, wait: Option<f64>) -> Res<LockGuard> {
        let guard = lock::take(&self.dir, wait)?;
        self.reload()?;
        Ok(guard)
    }

    /// The command prefix printed in guidance, with --campaigns when it is not the default.
    pub fn pm_prefix(&self) -> String {
        let pm = self.paths.pm_command();
        let default = crate::paths::resolve(&self.paths.repo.join("campaigns")).ok();
        if default.as_deref() == Some(self.base.as_path()) {
            return pm;
        }
        format!("{pm} --campaigns {}", self.paths.shown(&self.base))
    }

    pub fn save_decl(&self) -> Res<()> {
        write_json(
            &self.dir.join("campaign.json"),
            &Value::Object(self.decl.clone()),
        )
    }

    /// A budget: the declared int (any size, bool excluded), else the default.
    pub fn budget(&self, name: &str) -> Num {
        let fallback = DEFAULT_BUDGETS
            .iter()
            .find(|(k, _)| *k == name)
            .map_or(Num::Int(0), |(_, v)| Num::Int(i128::from(*v)));
        match self.decl.get("budgets") {
            Some(Value::Object(b)) => match b.get(name) {
                Some(v) if is_int(v) => Num::of(v).unwrap_or(fallback),
                _ => fallback,
            },
            _ => fallback,
        }
    }

    pub fn phases(&self) -> Vec<String> {
        self.decl["phases"]
            .as_array()
            .map(|a| a.iter().map(py_str).collect())
            .unwrap_or_default()
    }

    pub fn last_phase(&self) -> Res<String> {
        self.phases()
            .pop()
            .ok_or_else(|| PmError::Internal("IndexError: list index out of range".into()))
    }

    fn cell_list(&self) -> &Vec<Value> {
        self.decl["cells"]
            .as_array()
            .expect("cells is a list after reload")
    }

    fn cell_list_mut(&mut self) -> &mut Vec<Value> {
        self.decl
            .get_mut("cells")
            .and_then(Value::as_array_mut)
            .expect("cells is a list after reload")
    }

    /// Index of the first cell entry with this name, defaults filled in.
    pub fn cell_index(&mut self, name: &Value) -> Res<usize> {
        let mut found = None;
        for (i, entry) in self.cell_list().iter().enumerate() {
            let entry = as_obj(entry)?;
            if entry.get("cell").is_some_and(|v| py_eq(v, name)) {
                found = Some(i);
                break;
            }
        }
        let Some(i) = found else {
            return Err(PmError::usage(format!(
                "unknown cell {}",
                crate::py::py_repr(name)
            )));
        };
        let entry = as_obj_mut(&mut self.cell_list_mut()[i])?;
        for (key, value) in [
            ("state", json!("open")),
            ("origin", json!("plan")),
            ("parent", Value::Null),
            ("review", json!("none")),
            ("needs", json!([])),
        ] {
            entry.entry(key).or_insert(value);
        }
        Ok(i)
    }

    pub fn cell_at(&self, i: usize) -> &Obj {
        self.cell_list()[i]
            .as_object()
            .expect("checked by cell_index")
    }

    pub fn cell_at_mut(&mut self, i: usize) -> &mut Obj {
        self.cell_list_mut()[i]
            .as_object_mut()
            .expect("checked by cell_index")
    }

    pub fn cell(&mut self, name: &str) -> Res<Obj> {
        let i = self.cell_index(&Value::from(name))?;
        Ok(self.cell_at(i).clone())
    }

    /// Indices of every declared cell (a repeated name maps to its first entry).
    pub fn cell_indices(&mut self) -> Res<Vec<usize>> {
        let names: Vec<Value> = self
            .cell_list()
            .iter()
            .map(|entry| Ok(need(as_obj(entry)?, "cell")?.clone()))
            .collect::<Res<_>>()?;
        names.iter().map(|n| self.cell_index(n)).collect()
    }

    pub fn cells(&mut self) -> Res<Vec<Obj>> {
        let idx = self.cell_indices()?;
        Ok(idx.into_iter().map(|i| self.cell_at(i).clone()).collect())
    }

    pub fn group_entries(&self) -> &Vec<Value> {
        self.decl["groups"]
            .as_array()
            .expect("groups is a list after reload")
    }

    /// The names of the declared groups, in order.
    pub fn group_names(&self) -> Res<Vec<String>> {
        self.group_entries()
            .iter()
            .map(|g| Ok(py_str(need(as_obj(g)?, "group")?)))
            .collect()
    }

    pub fn group(&self, name: &str) -> Res<Obj> {
        for entry in self.group_entries() {
            let entry = as_obj(entry)?;
            if eq_str(entry.get("group"), name) {
                return Ok(entry.clone());
            }
        }
        Err(PmError::usage(format!("unknown group {}", repr_str(name))))
    }

    /// The check names of a group.
    pub fn group_checks(&self, name: &str) -> Res<Vec<String>> {
        let group = self.group(name)?;
        Ok(as_list(need(&group, "checks")?)?
            .iter()
            .map(py_str)
            .collect())
    }

    pub fn has_group(&self, name: &str) -> Res<bool> {
        for entry in self.group_entries() {
            if eq_str(as_obj(entry)?.get("group"), name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn check_entries(&self) -> &Vec<Value> {
        self.decl["checks"]
            .as_array()
            .expect("checks is a list after reload")
    }

    pub fn check(&self, name: &str) -> Res<Obj> {
        for entry in self.check_entries() {
            let entry = as_obj(entry)?;
            if eq_str(entry.get("id"), name) {
                return Ok(entry.clone());
            }
        }
        Err(PmError::usage(format!("unknown check {}", repr_str(name))))
    }

    /// Indices of the cells of a group (live: not cancelled).
    pub fn member_indices(&mut self, group: &str, live: bool) -> Res<Vec<usize>> {
        let mut out = Vec::new();
        for i in self.cell_indices()? {
            let cell = self.cell_at(i);
            if eq_str(Some(need(cell, "group")?), group)
                && (!live || !eq_str(Some(need(cell, "state")?), "cancelled"))
            {
                out.push(i);
            }
        }
        Ok(out)
    }

    pub fn members(&mut self, group: &str) -> Res<Vec<Obj>> {
        let idx = self.member_indices(group, true)?;
        Ok(idx.into_iter().map(|i| self.cell_at(i).clone()).collect())
    }

    // -- journey
    pub fn events(&mut self) -> Res<&mut Vec<Obj>> {
        if self.events_cache.is_none() {
            let mut found = Vec::new();
            let path = self.dir.join("journey.jsonl");
            if path.is_file() {
                let raw = fs::read(&path).map_err(|e| PmError::os(&e))?;
                let text = String::from_utf8(raw).map_err(|e| {
                    PmError::Internal(format!(
                        "UnicodeDecodeError: {}",
                        crate::py::decode_error(&e)
                    ))
                })?;
                for line in splitlines(&text) {
                    if let Some(Value::Object(event)) = crate::jsonio::parse_or_skip(line)? {
                        found.push(event);
                    }
                }
            }
            self.events_cache = Some(found);
        }
        Ok(self.events_cache.as_mut().expect("filled above"))
    }

    pub fn events_named(&mut self, name: &str) -> Res<Vec<Obj>> {
        Ok(self
            .events()?
            .iter()
            .filter(|e| eq_str(e.get("event"), name))
            .cloned()
            .collect())
    }

    /// Append one journey event; its time never goes back before the last event's.
    pub fn emit(&mut self, name: &str, fields: Vec<(&str, Value)>) -> Res<Obj> {
        let path = self.dir.join("journey.jsonl");
        let events = self.events()?;
        let mut moment = now();
        let last: Option<Moment> = events
            .last()
            .and_then(|e| e.get("utc"))
            .and_then(Value::as_str)
            .and_then(parse_time);
        if let Some(last) = last {
            if moment < last {
                moment = last;
            }
        }
        let mut event = obj([
            ("utc", Value::from(stamp(moment))),
            ("event", Value::from(name)),
        ]);
        for (k, v) in fields {
            event.insert(k.to_string(), v);
        }
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|e| PmError::os(&e))?;
        let line = crate::jsonio::encode(&(compact(&Value::Object(event.clone())) + "\n"))?;
        file.write_all(line.as_bytes())
            .map_err(|e| PmError::os(&e))?;
        events.push(event.clone());
        Ok(event)
    }

    // -- phase files
    pub fn phase_path(&self, cell: &str, phase: &str) -> PathBuf {
        self.dir
            .join("cells")
            .join(crate::jsonio::os_text(cell))
            .join("phases")
            .join(crate::jsonio::os_text(&format!("{phase}{SUFFIX}")))
    }

    pub fn phase(&self, cell: &str, phase: &str) -> Res<Obj> {
        if !self.phases().iter().any(|p| p == phase) {
            return Err(PmError::usage(format!("unknown phase {}", repr_str(phase))));
        }
        let path = self.phase_path(cell, phase);
        let data = read_json(&path, None)?;
        let mut data = match data {
            Value::Object(m) if matches!(m.get("proof"), Some(Value::Object(_))) => m,
            _ => {
                return Err(PmError::usage(format!(
                    "{}: malformed phase file",
                    path.display()
                )));
            }
        };
        data.entry("attempt").or_insert(Value::from(1));
        data.entry("retries").or_insert(Value::from(0));
        Ok(data)
    }

    pub fn phase_state(&self, cell: &str, phase: &str) -> Res<Value> {
        Ok(need(&self.phase(cell, phase)?, "state")?.clone())
    }

    pub fn save_phase(&self, cell: &str, phase: &str, data: &Obj) -> Res<()> {
        write_json(&self.phase_path(cell, phase), &Value::Object(data.clone()))
    }

    // -- gates
    fn gate_path(&self, group: &str) -> PathBuf {
        self.dir
            .join("gates")
            .join(crate::jsonio::os_text(&format!("{group}{SUFFIX}")))
    }

    pub fn gate(&self, group: &str) -> Res<Obj> {
        let blank = json!({"schema_version": SCHEMA, "group": group, "state": "pending", "runs": 0,
            "started": null, "ended": null, "members": [], "hitl": null, "checks": []});
        match read_json(&self.gate_path(group), Some(&blank))? {
            Value::Object(m) => Ok(m),
            _ => Err(PmError::usage(format!(
                "gates/{group}{SUFFIX}: malformed gate record"
            ))),
        }
    }

    pub fn gate_state(&self, group: &str) -> Res<Value> {
        Ok(need(&self.gate(group)?, "state")?.clone())
    }

    pub fn save_gate(&self, group: &str, data: &Obj) -> Res<()> {
        write_json(&self.gate_path(group), &Value::Object(data.clone()))
    }

    pub fn group_state(&mut self, group: &str) -> Res<&'static str> {
        let cells = self.members(group)?;
        if cells.is_empty() {
            return Ok(crate::GROUP_CANCELLED);
        }
        if cells.iter().any(|c| eq_str(c.get("state"), "stopped")) {
            return Ok(crate::GROUP_STOPPED);
        }
        let gate = self.gate_state(group)?;
        if eq_str(Some(&gate), "waiting") || cells.iter().any(|c| eq_str(c.get("state"), "waiting"))
        {
            return Ok(crate::GROUP_WAITING);
        }
        Ok(match gate.as_str() {
            Some("passed") => crate::GROUP_PASSED,
            Some("failed") => crate::GROUP_FAILED,
            _ => crate::GROUP_ACTIVE,
        })
    }

    // -- monitor
    pub fn monitor(&self) -> Res<Obj> {
        let data = read_json(
            &self.dir.join("monitor.json"),
            Some(&Value::Object(default_monitor())),
        )?;
        let Value::Object(data) = data else {
            return Err(PmError::usage("monitor.json: malformed"));
        };
        let mut merged = default_monitor();
        for (k, v) in data {
            merged.insert(k, v);
        }
        Ok(merged)
    }

    pub fn save_monitor(&self, data: &Obj) -> Res<()> {
        write_json(&self.dir.join("monitor.json"), &Value::Object(data.clone()))
    }

    // -- receipts
    pub fn receipts(&self, check: &str) -> Res<Vec<Obj>> {
        let folder = self
            .dir
            .join("receipts")
            .join(crate::jsonio::os_text(check));
        let mut found = Vec::new();
        if folder.is_dir() {
            let entries = fs::read_dir(&folder).map_err(|e| PmError::os(&e))?;
            for entry in entries {
                let entry = entry.map_err(|e| PmError::os(&e))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(stem) = name.strip_suffix(".json") else {
                    continue;
                };
                if !is_digits(stem) {
                    continue;
                }
                if let Value::Object(mut data) = read_json(&entry.path(), None)? {
                    data.insert(
                        "_path".into(),
                        Value::from(format!("receipts/{check}/{name}")),
                    );
                    found.push(data);
                }
            }
        }
        let zero = Value::from(0);
        crate::py::py_sort(&mut found, |a, b| {
            let x = num(a.get("n").unwrap_or(&zero))?;
            let y = num(b.get("n").unwrap_or(&zero))?;
            Ok(x.compare(y) == Some(std::cmp::Ordering::Less))
        })?;
        Ok(found)
    }

    pub fn all_receipts(&self) -> Res<Vec<Obj>> {
        let folder = self.dir.join("receipts");
        let mut names = Vec::new();
        if folder.is_dir() {
            for entry in fs::read_dir(&folder).map_err(|e| PmError::os(&e))? {
                let entry = entry.map_err(|e| PmError::os(&e))?;
                if entry.path().is_dir() {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        names.sort();
        let mut out = Vec::new();
        for name in names {
            out.extend(self.receipts(&name)?);
        }
        Ok(out)
    }

    pub fn invalidated_at(&self, check: &str) -> Res<Option<Moment>> {
        let path = self
            .dir
            .join("receipts")
            .join(crate::jsonio::os_text(check))
            .join("invalidated.json");
        if !path.is_file() {
            return Ok(None);
        }
        Ok(match read_json(&path, None)? {
            Value::Object(m) => m.get("utc").and_then(Value::as_str).and_then(parse_time),
            _ => None,
        })
    }

    /// A receipt (or its covers entry for `check`) written before an invalidation is not used.
    pub fn usable(&self, receipt: &Obj, check: Option<&str>) -> Res<bool> {
        let name = match check {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => receipt.get("check").map(py_str).unwrap_or_default(),
        };
        let cut = self.invalidated_at(&name)?;
        let ended = receipt
            .get("ended")
            .and_then(Value::as_str)
            .and_then(parse_time);
        Ok(match cut {
            None => true,
            Some(cut) => ended.is_some_and(|e| e >= cut),
        })
    }

    // -- fingerprints
    pub fn input_ids(&mut self, root: &Path, inputs: &[Value]) -> Res<Obj> {
        let mut ids = Obj::new();
        for item in inputs {
            let text = py_str(item);
            let trimmed = crate::py::strip(&text).trim_matches('/');
            let path = if trimmed.is_empty() { "." } else { trimmed }.to_string();
            let key = (root.display().to_string(), path.clone());
            if !self.ids_cache.contains_key(&key) {
                let spec = format!("HEAD:./{}", if path == "." { "" } else { path.as_str() });
                let (code, out) = git(root, &["rev-parse", "--verify", "--quiet", &spec])?;
                let id = (code == 0 && !out.is_empty()).then_some(out);
                self.ids_cache.insert(key.clone(), id);
            }
            let id = self.ids_cache[&key].clone();
            ids.insert(text, id.map_or(Value::Null, Value::from));
        }
        Ok(ids)
    }

    pub fn probe(&mut self, root: &Path) -> Res<Obj> {
        let key = root.display().to_string();
        if !self.probe_cache.contains_key(&key) {
            let mut outputs = Obj::new();
            let cmds = list_or_empty(self.decl["target"].get("env_probe")).unwrap_or_default();
            for cmd in cmds {
                let cmd = py_str(&cmd);
                outputs.insert(cmd.clone(), Value::from(probe(&cmd, root)?));
            }
            self.probe_cache.insert(key.clone(), outputs);
        }
        Ok(self.probe_cache[&key].clone())
    }

    /// Rule 1: whether a result at this input can be reused, and the key.
    pub fn fingerprint(&mut self, check: &Obj, root: &Path) -> Res<Fingerprint> {
        let inputs = list_or_empty(check.get("inputs"))?;
        let mut why = None;
        if inputs.is_empty() {
            why = Some("no-inputs");
        } else if git(root, &["rev-parse", "--is-inside-work-tree"])?.0 != 0 {
            why = Some("not-git");
        } else {
            let texts: Vec<String> = inputs.iter().map(py_str).collect();
            let mut args = vec!["status", "--porcelain", "--"];
            args.extend(texts.iter().map(String::as_str));
            let (code, out) = git(root, &args)?;
            if code != 0 || !out.is_empty() {
                why = Some("uncommitted-input");
            } else if self.input_ids(root, &inputs)?.values().any(Value::is_null) {
                // not in HEAD: a glob, a missing or ignored path, a folder of another repo
                why = Some("unresolved-input");
            }
        }
        let mut env = Obj::new();
        for name in list_or_empty(check.get("env"))? {
            let name = py_str(&name);
            let value = std::env::var(crate::jsonio::os_text(&name))
                .ok()
                .map_or(Value::Null, |v| Value::from(crate::jsonio::mark(&v)));
            env.insert(name, value);
        }
        let ids = if why == Some("not-git") {
            inputs.iter().map(|i| (py_str(i), Value::Null)).collect()
        } else {
            self.input_ids(root, &inputs)?
        };
        let probe = if root.is_dir() {
            self.probe(root)?
        } else {
            Obj::new()
        };
        let key = Value::Object(obj([
            ("cmd", need(check, "cmd")?.clone()),
            (
                "cwd",
                check
                    .get("cwd")
                    .cloned()
                    .unwrap_or_else(|| Value::from(".")),
            ),
            ("expect", need(check, "expect")?.clone()),
            ("env", Value::Object(env)),
            ("inputs", Value::Object(ids)),
            ("host", Value::from(node_name())),
            ("env_probe", Value::Object(probe)),
        ]));
        let sha = digest(&key)?;
        Ok(Fingerprint {
            key,
            sha,
            reusable: why.is_none(),
            why,
        })
    }

    // -- hitl bookkeeping
    /// Every hand-over with its answer (the last answer event with the same id), or null.
    pub fn hitls(&mut self) -> Res<Vec<Obj>> {
        let events = self.events()?.clone();
        let answers: Vec<&Obj> = events
            .iter()
            .filter(|e| eq_str(e.get("event"), "hitl-answer"))
            .collect();
        let mut out = Vec::new();
        for e in events.iter().filter(|e| eq_str(e.get("event"), "hitl")) {
            let id = e.get("id").cloned().unwrap_or(Value::Null);
            let answer = answers
                .iter()
                .rev()
                .find(|a| py_eq(a.get("id").unwrap_or(&Value::Null), &id))
                .map_or(Value::Null, |a| Value::Object((*a).clone()));
            let mut item = e.clone();
            item.insert("answer".into(), answer);
            out.push(item);
        }
        Ok(out)
    }

    fn matches_target(item: &Obj, target: &[(&str, Value)]) -> bool {
        target
            .iter()
            .all(|(k, v)| py_eq(item.get(*k).unwrap_or(&Value::Null), v))
    }

    /// The open hand-over of this kind and target, or a new one.
    pub fn open_hitl(
        &mut self,
        kind: &str,
        question: &str,
        target: Vec<(&str, Value)>,
    ) -> Res<Value> {
        for item in self.hitls()? {
            if item["answer"].is_null()
                && eq_str(item.get("kind"), kind)
                && Self::matches_target(&item, &target)
            {
                return Ok(item.get("id").cloned().unwrap_or(Value::Null));
            }
        }
        let count = self.events_named("hitl")?.len();
        let ident = Value::from(format!("h{}", count + 1));
        let mut fields = vec![
            ("id", ident.clone()),
            ("kind", Value::from(kind)),
            ("question", Value::from(question)),
        ];
        fields.extend(target);
        self.emit("hitl", fields)?;
        Ok(ident)
    }

    /// How many answered hand-overs of this kind and target allowed one more.
    pub fn allows(&mut self, kind: &str, target: Vec<(&str, Value)>) -> Res<Num> {
        let mut count = 0;
        for item in self.hitls()? {
            let allowed = item["answer"]
                .as_object()
                .is_some_and(|a| a.get("allow") == Some(&Value::Bool(true)));
            if eq_str(item.get("kind"), kind) && allowed && Self::matches_target(&item, &target) {
                count += 1;
            }
        }
        Ok(Num::Int(count))
    }

    pub fn final_names(&self) -> Res<Vec<String>> {
        let fin = as_obj(&self.decl["final"])?;
        Ok(as_list(need(fin, "checks")?)?.iter().map(py_str).collect())
    }

    pub fn final_gate(&self) -> Res<Obj> {
        self.gate(FINAL)
    }

    /// Whether a receipt is truthy-reusable.
    pub fn receipt_reusable(receipt: &Obj) -> bool {
        opt_truthy(receipt.get("reusable"))
    }

    pub fn contains_name(list: &[Value], name: &str) -> bool {
        contains(list, &Value::from(name))
    }
}
