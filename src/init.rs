//! `init`: lay the campaigns/ skeleton and the Stop hook into a project. Nothing is overwritten.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{PmError, Res};
use crate::jsonio::pretty;
use crate::paths::Paths;
use crate::py::{obj, Obj};

pub const IGNORE_HINT: [&str; 3] = [
    "campaigns/*",
    "!campaigns/README.md",
    "!campaigns/_template/",
];

fn hook_file(harness: &str) -> &'static str {
    if harness == "claude" {
        ".claude/settings.local.json"
    } else {
        ".codex/hooks.json"
    }
}

/// The Stop hook entry for one harness, pointing at this executable.
pub fn hook_snippet(paths: &Paths, harness: &str) -> Value {
    let rel = paths.pm_command();
    let command = if harness == "claude" {
        format!("\"${{CLAUDE_PROJECT_DIR}}/{rel}\" stop-hook --harness claude")
    } else {
        format!("{rel} stop-hook --harness codex")
    };
    serde_json::json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": command, "timeout": 30}]}]}})
}

/// Every path under `dir` (directory links are not entered), as relative components.
fn walk(dir: &Path, prefix: &mut Vec<OsString>, out: &mut Vec<Vec<OsString>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        prefix.push(entry.file_name());
        out.push(prefix.clone());
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&entry.path(), prefix, out);
        }
        prefix.pop();
    }
}

fn shown(paths: &Paths, path: &Path) -> String {
    paths.shown(path)
}

pub fn cmd_init(base: &Path, paths: &Paths, harness: &str, apply: bool) -> Res<Obj> {
    let source = paths.skill_dir.join("assets").join("campaigns");
    if !source.join("README.md").is_file() || !source.join("_template").is_dir() {
        return Err(PmError::usage(format!(
            "package assets are missing: {}",
            source.display()
        )));
    }
    let mut found = Vec::new();
    walk(&source, &mut Vec::new(), &mut found);
    found.sort();
    let (mut created, mut kept) = (Vec::new(), Vec::new());
    for parts in found {
        let rel: PathBuf = parts.iter().collect();
        let item = source.join(&rel);
        if !item.is_file() {
            continue;
        }
        let dest = base.join(&rel);
        let text = shown(paths, &dest);
        if dest.exists() {
            kept.push(Value::from(text));
            continue;
        }
        created.push(Value::from(text));
        if apply {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| PmError::os(&e))?;
            }
            let bytes = fs::read(&item).map_err(|e| PmError::os(&e))?;
            fs::write(&dest, bytes).map_err(|e| PmError::os(&e))?;
        }
    }
    let harnesses: Vec<&str> = if harness == "both" {
        vec!["claude", "codex"]
    } else {
        vec![harness]
    };
    let mut hooks = Vec::new();
    let mut manual = Vec::new();
    for h in harnesses {
        let file = hook_file(h);
        let path = paths.repo.join(file);
        let snippet = hook_snippet(paths, h);
        let marker = format!("stop-hook --harness {h}");
        let state = if !path.exists() {
            if apply {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| PmError::os(&e))?;
                }
                fs::write(&path, pretty(&snippet) + "\n").map_err(|e| PmError::os(&e))?;
                "created"
            } else {
                "would-create"
            }
        } else {
            let bytes = fs::read(&path).map_err(|e| PmError::os(&e))?;
            if String::from_utf8_lossy(&bytes).contains(&marker) {
                "present"
            } else {
                "exists-not-modified"
            }
        };
        if state == "exists-not-modified" {
            manual.push(file.to_string());
        }
        hooks.push(Value::Object(obj([
            ("harness", Value::from(h)),
            ("file", Value::from(file)),
            ("state", Value::from(state)),
            (
                "snippet",
                if state == "exists-not-modified" {
                    snippet
                } else {
                    Value::Null
                },
            ),
        ])));
    }
    let mut next = Vec::new();
    if !apply {
        next.push("다시 --apply로 부르면 위 created 목록을 만든다.".to_string());
    }
    if !manual.is_empty() {
        next.push(format!(
            "기존 파일은 고치지 않았다. snippet의 Stop 항목을 직접 더한다: {}",
            manual.join(", ")
        ));
    }
    next.push(
        "훅 파일은 Git이 무시하게 둔다. 캠페인 인스턴스를 추적하지 않으려면 .gitignore에 ignore_hint를 더한다."
            .to_string(),
    );
    next.push(format!("틀 검사: {}", paths.tool_command("check_campaign")));
    Ok(obj([
        ("ok", Value::Bool(true)),
        ("mode", Value::from(if apply { "apply" } else { "dry-run" })),
        ("root", Value::from(paths.repo.display().to_string())),
        ("campaigns", Value::from(shown(paths, base))),
        ("created", Value::Array(created)),
        ("kept", Value::Array(kept)),
        ("hooks", Value::Array(hooks)),
        (
            "ignore_hint",
            Value::Array(IGNORE_HINT.iter().map(|s| Value::from(*s)).collect()),
        ),
        (
            "next",
            Value::Array(next.into_iter().map(Value::from).collect()),
        ),
    ]))
}
