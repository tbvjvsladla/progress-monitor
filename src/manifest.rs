//! PACKAGE_MANIFEST.json: every file with path, bytes, mode and sha256.
//!
//! The manifest itself and `INVENTORY_EXCLUDES` are not listed. mode is one of the two
//! values Git keeps (0644/0755), so a manifest made from the work tree also verifies an
//! install taken with `git archive`. Format: manifest_version 1.0.0.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::jsonio::parse;
use crate::py::{py_eq, splitlines, strip};

pub const MANIFEST: &str = "PACKAGE_MANIFEST.json";
pub const MANIFEST_VERSION: &str = "1.0.0";
pub const PACKAGE_NAME: &str = "progress-monitor";
pub const INVENTORY_EXCLUDES: [&str; 5] = [MANIFEST, ".git", "__pycache__", ".cache", "target"];

fn rel_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn walk(root: &Path, folder: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let mut entries = fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    entries.sort();
    for entry in entries {
        let name = entry.file_name().unwrap_or_default();
        if INVENTORY_EXCLUDES.iter().any(|x| name == *x) {
            continue;
        }
        let kind = fs::symlink_metadata(&entry)
            .map_err(|e| format!("{}: {e}", entry.display()))?
            .file_type();
        if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
            return Err(format!(
                "symlinks and special files cannot be in the package: {}",
                rel_text(root, &entry)
            ));
        }
        if kind.is_dir() {
            walk(root, &entry, out)?;
        } else {
            out.push(rel_text(root, &entry));
        }
    }
    Ok(())
}

/// Every listed file, as `/`-joined paths relative to `root`, sorted as text.
pub fn inventory(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// The version written in SKILL.md metadata (the one place the package version lives).
pub fn package_version(root: &Path) -> Result<String, String> {
    let path = root.join("SKILL.md");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for line in splitlines(&text) {
        let stripped = strip(line);
        if let Some(rest) = stripped.strip_prefix("version:") {
            return Ok(strip(rest).trim_matches('"').to_string());
        }
    }
    Err("SKILL.md metadata has no version".to_string())
}

/// The manifest of the tree at `root`.
pub fn build(root: &Path) -> Result<Value, String> {
    let mut files = Vec::new();
    for rel in inventory(root)? {
        let path = root.join(&rel);
        let data = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mode = fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .permissions()
            .mode();
        let sha: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        files.push(json!({
            "path": rel,
            "bytes": data.len(),
            "mode": if mode & 0o111 != 0 { "0755" } else { "0644" },
            "sha256": sha,
        }));
    }
    Ok(json!({
        "manifest_version": MANIFEST_VERSION,
        "package": PACKAGE_NAME,
        "version": package_version(root)?,
        "inventory_excludes": INVENTORY_EXCLUDES,
        "file_count": files.len(),
        "files": files,
    }))
}

fn by_path(manifest: &Value) -> Result<Vec<(String, Value)>, String> {
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{MANIFEST}: no files list"))?;
    files
        .iter()
        .map(|f| match f.get("path") {
            Some(Value::String(p)) => Ok((p.clone(), f.clone())),
            _ => Err(format!("{MANIFEST}: a file entry has no path")),
        })
        .collect()
}

fn lookup<'a>(entries: &'a [(String, Value)], path: &str) -> Option<&'a Value> {
    // A later entry with the same path wins, like building a dict.
    entries
        .iter()
        .rev()
        .find(|(p, _)| p == path)
        .map(|(_, v)| v)
}

/// What differs between the written manifest and the tree: `<version>` first, then paths.
pub fn diff(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(MANIFEST);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let have = parse(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let want = build(root)?;
    let mut out = Vec::new();
    let have_version = have.get("version").unwrap_or(&Value::Null);
    if !py_eq(have_version, &want["version"]) {
        out.push("<version>".to_string());
    }
    let h = by_path(&have)?;
    let w = by_path(&want)?;
    let all: BTreeSet<&String> = h.iter().chain(w.iter()).map(|(p, _)| p).collect();
    for p in all {
        let same = match (lookup(&h, p), lookup(&w, p)) {
            (Some(a), Some(b)) => py_eq(a, b),
            _ => false,
        };
        if !same {
            out.push(p.clone());
        }
    }
    Ok(out)
}
