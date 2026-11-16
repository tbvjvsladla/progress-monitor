//! Tests for the distribution side: project root detection, init, hygiene, manifest,
//! install smoke.
//!
//! An install is the package tree copied without its build output, with the executables
//! of this build (`CARGO_BIN_EXE_*`) placed in the copy's `target/release/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use progress_monitor::install::{copy_tree, make_temp_dir, SKIP};
use progress_monitor::manifest::package_version;

const PACKAGE: &str = env!("CARGO_MANIFEST_DIR");
const PM: &str = env!("CARGO_BIN_EXE_pm");
const LINT: &str = env!("CARGO_BIN_EXE_check_campaign");
const SANITIZE: &str = env!("CARGO_BIN_EXE_sanitize_check");
const MANIFEST_TOOL: &str = env!("CARGO_BIN_EXE_package_manifest");
const SMOKE: &str = env!("CARGO_BIN_EXE_smoke_install");

fn package() -> PathBuf {
    PathBuf::from(PACKAGE)
}

/// `program args` in `cwd` with every `PM_*` variable removed, then `env` added.
fn run(program: &Path, args: &[&str], cwd: &Path, env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PM_") {
            command.env_remove(&name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run the program")
}

fn stdout(done: &Output) -> String {
    String::from_utf8_lossy(&done.stdout).into_owned()
}

fn json_of(done: &Output) -> Value {
    serde_json::from_str(&stdout(done))
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", stdout(done)))
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

/// A temporary project with the package copied to `<root>/<place>/progress-monitor`.
struct Installed {
    tmp: PathBuf,
    root: PathBuf,
    skill: PathBuf,
}

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

impl Installed {
    fn new(place: &str) -> Installed {
        let tmp = make_temp_dir("pm-pkg-").expect("temporary folder");
        let root = tmp.join("project");
        let skill = root.join(place).join("progress-monitor");
        copy_tree(&package(), &skill, &SKIP).expect("copy the package");
        let release = skill.join("target").join("release");
        fs::create_dir_all(&release).unwrap();
        fs::copy(PM, release.join("pm")).unwrap();
        fs::copy(LINT, release.join("check_campaign")).unwrap();
        Installed { tmp, root, skill }
    }

    fn claude() -> Installed {
        Installed::new(".claude/skills")
    }

    fn tool(&self, name: &str) -> PathBuf {
        self.skill.join("target").join("release").join(name)
    }

    fn pm_in(&self, args: &[&str], cwd: &Path) -> (i32, Value) {
        let done = run(&self.tool("pm"), args, cwd, &[]);
        (done.status.code().unwrap_or(-1), json_of(&done))
    }

    fn pm(&self, args: &[&str]) -> (i32, Value) {
        self.pm_in(args, &self.root)
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|v| v.as_str().expect("a string").to_string())
        .collect()
}

fn states(hooks: &Value) -> Vec<String> {
    let mut out: Vec<String> = hooks
        .as_array()
        .expect("a list of hooks")
        .iter()
        .map(|h| h["state"].as_str().expect("a state").to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn stop_command(hooks_doc: &Value) -> String {
    hooks_doc["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .expect("a hook command")
        .to_string()
}

// -- RootDetection

#[test]
fn root_for_each_install_place() {
    for place in [".claude/skills", ".agents/skills", "skills"] {
        let inst = Installed::new(place);
        let (code, out) = inst.pm_in(&["init"], &inst.tmp);
        assert_eq!(code, 0, "{place}: {out}");
        assert_eq!(
            PathBuf::from(out["root"].as_str().unwrap()),
            canonical(&inst.root),
            "{place}"
        );
        assert_eq!(out["campaigns"], "campaigns", "{place}");
    }
}

#[test]
fn env_overrides_root() {
    let inst = Installed::claude();
    let other = inst.tmp.join("other");
    fs::create_dir(&other).unwrap();
    let done = run(
        &inst.tool("pm"),
        &["init"],
        &inst.root,
        &[("PM_PROJECT_ROOT", &other)],
    );
    let out = json_of(&done);
    assert_eq!(
        PathBuf::from(out["root"].as_str().unwrap()),
        canonical(&other)
    );
}

// -- Init

#[test]
fn dry_run_writes_nothing() {
    let inst = Installed::claude();
    let (code, out) = inst.pm(&["init"]);
    assert_eq!((code, out["mode"].as_str()), (0, Some("dry-run")));
    let created = strings(&out["created"]);
    assert!(created.contains(&"campaigns/README.md".to_string()));
    assert!(created.contains(&"campaigns/_template/campaign.json".to_string()));
    assert!(!inst.root.join("campaigns").exists());
    assert!(!inst
        .root
        .join(".claude")
        .join("settings.local.json")
        .exists());
}

#[test]
fn apply_then_again_keeps_everything() {
    let inst = Installed::claude();
    let (_, first) = inst.pm(&["init", "--apply"]);
    let created = strings(&first["created"]);
    for rel in &created {
        let source = inst.skill.join("assets").join(rel);
        assert_eq!(
            fs::read(inst.root.join(rel)).unwrap(),
            fs::read(source).unwrap(),
            "{rel}"
        );
    }
    assert_eq!(states(&first["hooks"]), ["created"]);
    let readme = inst.root.join("campaigns").join("README.md");
    fs::write(&readme, "mine\n").unwrap();
    let (_, second) = inst.pm(&["init", "--apply"]);
    assert_eq!(second["created"], serde_json::json!([]));
    let mut kept = strings(&second["kept"]);
    kept.sort();
    let mut want = created.clone();
    want.sort();
    assert_eq!(kept, want);
    assert_eq!(states(&second["hooks"]), ["present"]);
    assert_eq!(fs::read_to_string(&readme).unwrap(), "mine\n");
}

#[test]
fn existing_hook_file_is_not_modified() {
    let inst = Installed::claude();
    let settings = inst.root.join(".claude").join("settings.local.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(&settings, "{\"permissions\": {\"allow\": []}}\n").unwrap();
    let (_, out) = inst.pm(&["init", "--apply", "--harness", "claude"]);
    assert_eq!(
        fs::read_to_string(&settings).unwrap(),
        "{\"permissions\": {\"allow\": []}}\n"
    );
    let hook = &out["hooks"][0];
    assert_eq!(hook["state"], "exists-not-modified");
    let command = stop_command(&hook["snippet"]);
    assert!(
        command.contains("${CLAUDE_PROJECT_DIR}/.claude/skills/progress-monitor/target/release/pm"),
        "{command}"
    );
    assert!(command.ends_with("stop-hook --harness claude"), "{command}");
}

#[test]
fn codex_hook_points_at_the_skill() {
    let inst = Installed::new(".agents/skills");
    let (_, out) = inst.pm(&["init", "--apply", "--harness", "codex"]);
    let text = fs::read_to_string(inst.root.join(".codex").join("hooks.json")).unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        stop_command(&data),
        ".agents/skills/progress-monitor/target/release/pm stop-hook --harness codex"
    );
    let harnesses: Vec<&str> = out["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["harness"].as_str().unwrap())
        .collect();
    assert_eq!(harnesses, ["codex"]);
}

#[test]
fn laid_template_passes_lint() {
    let inst = Installed::claude();
    inst.pm(&["init", "--apply"]);
    let done = run(&inst.tool("check_campaign"), &[], &inst.root, &[]);
    assert_eq!(done.status.code(), Some(0), "{}", stdout(&done));
    let out = json_of(&done);
    assert_eq!(
        (
            out["status"].as_str(),
            out["mode"].as_str(),
            out["target"].as_str()
        ),
        (Some("PASS"), Some("template"), Some("campaigns/_template"))
    );
}

// -- Distribution

#[test]
fn sanitize_has_no_hits() {
    let done = run(Path::new(SANITIZE), &[], &package(), &[]);
    assert_eq!(done.status.code(), Some(0), "{}", stdout(&done));
    assert!(stdout(&done).contains("SANITIZE OK (0 hits)"));
}

#[test]
fn sanitize_catches_planted_identifiers() {
    let tmp = make_temp_dir("pm-san-").unwrap();
    // Built from pieces so the hygiene scan of this file stays clean.
    let planted = [
        ["/", "home/someone/x"].concat(),
        ["10.", "1.2.3"].concat(),
        ["me", "@", "corp.io"].concat(),
        ["val", "_agent"].concat(),
    ];
    fs::write(tmp.join("a.md"), planted.join("\n") + "\n").unwrap();
    let done = run(
        Path::new(SANITIZE),
        &[tmp.to_str().unwrap()],
        &package(),
        &[],
    );
    let _ = fs::remove_dir_all(&tmp);
    assert_eq!(done.status.code(), Some(1));
    for name in ["home_path", "private_ip", "email", "project_identifier"] {
        assert!(stdout(&done).contains(name), "{name}: {}", stdout(&done));
    }
}

#[test]
fn manifest_matches_tree() {
    if !package().join("PACKAGE_MANIFEST.json").is_file() {
        eprintln!("skipped: PACKAGE_MANIFEST.json is written at release time");
        return;
    }
    let done = run(Path::new(MANIFEST_TOOL), &["--check"], &package(), &[]);
    assert_eq!(done.status.code(), Some(0), "{}", stdout(&done));
}

/// `version = "..."` of the `[package]` table in Cargo.toml.
fn cargo_toml_version(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(rest) = line.strip_prefix("version") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
}

#[test]
fn versions_agree() {
    let version = package_version(&package()).unwrap();
    let parts: Vec<&str> = version.split('.').collect();
    assert!(
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "{version}"
    );
    let readme = fs::read_to_string(package().join("README.md")).unwrap();
    assert!(
        readme.contains(&format!("배포 버전 **{version}**")),
        "{version}"
    );
    let manifest = package().join("PACKAGE_MANIFEST.json");
    if manifest.is_file() {
        let data: Value = serde_json::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
        assert_eq!(data["version"].as_str(), Some(version.as_str()));
    }
    let cargo = fs::read_to_string(package().join("Cargo.toml")).unwrap();
    assert_eq!(
        cargo_toml_version(&cargo).as_deref(),
        Some(version.as_str())
    );
    assert_eq!(env!("CARGO_PKG_VERSION"), version);
}

#[test]
fn install_smoke() {
    let done = run(Path::new(SMOKE), &[], &package(), &[]);
    let out = stdout(&done);
    let count = out.chars().count();
    let last: String = out.chars().skip(count.saturating_sub(3000)).collect();
    assert_eq!(done.status.code(), Some(0), "{last}");
    assert_eq!(out.trim().lines().last(), Some("SMOKE-PASS"));
}
