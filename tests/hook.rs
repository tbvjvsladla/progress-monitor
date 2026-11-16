//! The Stop hook: when it keeps the agent going and when it lets go.

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::process::{Command, Stdio};

use common::{clean_command, pluck, s, utc_iso_hours_ago, Sandbox, PM};
use serde_json::{json, Value};

/// `pm` with the given arguments and stdin, stdout captured; no `--executor`.
fn run_raw(args: &[&str], stdin: &str) -> (Option<i32>, String) {
    let mut command: Command = clean_command(PM);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn pm");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let done = child.wait_with_output().unwrap();
    (
        done.status.code(),
        String::from_utf8_lossy(&done.stdout).into_owned(),
    )
}

#[test]
fn hook_no_action_when_nothing_actionable() {
    let sb = Sandbox::new();
    sb.set_cell("b1", json!({"needs": ["a1"]}));
    sb.ok(&["cell", "cancel", "a2", "--reason", "out of scope"]);
    sb.ok(&[
        "hitl",
        "open",
        "--cell",
        "a1",
        "--kind",
        "criteria",
        "--question",
        "which way?",
    ]);
    sb.ok(&["phase", "start", "b1", "prepare"]);
    sb.ok(&["phase", "close", "b1", "prepare"]);
    assert_eq!(sb.ok(&["status"])["actionable"], json!([]));
    assert_eq!(sb.hook(), None);
    assert_eq!(sb.events("hang"), Vec::<Value>::new());
}

#[test]
fn hook_never_exits_nonzero() {
    let sb = Sandbox::new();
    sb.ok(&["phase", "start", "a1", "prepare"]);
    fs::write(sb.inst.join("monitor.json"), "{not json").unwrap();
    assert_eq!(sb.hook(), None);
    let base = s(&sb.base);
    let done = run_raw(
        &["--campaigns", &base, "stop-hook", "--harness", "codex"],
        "not json either",
    );
    assert_eq!(done, (Some(0), String::new()));
    let done = run_raw(&["--campaigns", &base, "stop-hook"], "{}");
    assert_eq!(done, (Some(0), String::new()));
    let loop_path = sb.tmp.join("loop");
    std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap(); // a campaigns path that cannot be resolved
    let done = run_raw(
        &[
            "--campaigns",
            &s(&loop_path),
            "stop-hook",
            "--harness",
            "claude",
        ],
        "{}",
    );
    assert_eq!(done, (Some(0), String::new()));
    fs::remove_file(sb.inst.join("monitor.json")).unwrap();
    // the output cannot be written
    let full = File::options().write(true).open("/dev/full").unwrap();
    let mut child = clean_command(PM)
        .args(["--campaigns", &base, "stop-hook", "--harness", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json!({"session_id": "s1"}).to_string().as_bytes())
        .unwrap();
    let done = child.wait_with_output().unwrap();
    assert_eq!(done.status.code(), Some(0));
}

#[test]
fn hook_ignores_other_session() {
    let sb = Sandbox::new();
    sb.ok(&["phase", "start", "a1", "prepare"]);
    let out = sb.hook_s("s1").expect("a block");
    assert!(out["hookSpecificOutput"].get("additionalContext").is_some());
    let before = sb.monitor();
    assert_eq!(
        (before["session"].clone(), before["blocks"].clone()),
        (json!("s1"), json!(1))
    );
    assert_eq!(sb.hook_s("s2"), None);
    assert_eq!(sb.monitor(), before);
    sb.ok(&["monitor", "reset"]);
    assert!(sb.hook_s("s2").is_some());
    let monitor = sb.monitor();
    assert_eq!(
        (monitor["session"].clone(), monitor["total_blocks"].clone()),
        (json!("s2"), json!(2))
    );
}

#[test]
fn hook_background_tasks_not_counted() {
    let sb = Sandbox::new();
    sb.ok(&["phase", "start", "a1", "prepare"]);
    let tasks = json!([{"id": "t1", "type": "shell", "status": "running"}]);
    assert_eq!(
        sb.hook_with("s1", "claude", json!({"background_tasks": tasks})),
        None
    );
    assert_eq!(sb.monitor()["blocks"], 0);
    let path = sb
        .inst
        .join("cells")
        .join("a1")
        .join("phases")
        .join("prepare.status.json");
    let mut data = Sandbox::read(&path);
    data["started"] = json!(utc_iso_hours_ago(3));
    Sandbox::write(&path, &data);
    // over the phase budget
    let out = sb
        .hook_with("s1", "claude", json!({"background_tasks": tasks}))
        .expect("a block");
    assert_eq!(out["decision"], "block");
    assert!(out["reason"]
        .as_str()
        .unwrap()
        .contains("phase retry a1 prepare"));
    assert_eq!(pluck(&sb.events("hang"), "directive"), json!(["retry"]));
}

#[test]
fn hook_refused_fail_is_stall() {
    let sb = Sandbox::new();
    sb.ok(&["cell", "cancel", "a1", "--reason", "out of scope"]);
    sb.ok(&["cell", "cancel", "a2", "--reason", "out of scope"]);
    sb.commit("beta/state.txt", "broken\n", "change");
    sb.finish(&["b1"]);
    sb.no(&["gate", "run", "g-b"]);
    assert!(sb
        .hook()
        .expect("a block")
        .get("hookSpecificOutput")
        .is_some());
    sb.no(&["gate", "run", "g-b"]); // refused: same input, already FAIL
    let out = sb.hook().expect("a block");
    assert_eq!(out["decision"], "block");
    let got: Vec<_> = sb
        .events("hang")
        .iter()
        .map(|e| (e["count"].clone(), e["directive"].clone()))
        .collect();
    assert_eq!(got, vec![(json!(1), json!("retry"))]);
}

#[test]
fn hook_release_stays_until_change() {
    let sb = Sandbox::with(json!({"stop_block_cap": 2}), true, &[]);
    sb.ok(&["phase", "start", "a1", "prepare"]);
    assert!(sb.hook().is_some());
    assert!(sb.hook().is_some());
    assert_eq!(sb.hook(), None);
    assert_eq!(pluck(&sb.events("monitor-release"), "blocks"), json!([2]));
    assert_eq!(sb.hook(), None);
    assert_eq!(sb.events("monitor-release").len(), 1);
    sb.ok(&["phase", "close", "a1", "prepare"]);
    assert!(sb.hook().is_some());
}

#[test]
fn hook_bypasses_when_retries_spent() {
    let sb = Sandbox::new();
    sb.ok(&["phase", "start", "a1", "prepare"]);
    for _ in 0..2 {
        sb.hook_s("s1");
    }
    assert!(sb.hook_s("s1").expect("a block")["reason"]
        .as_str()
        .unwrap()
        .contains("phase retry a1 prepare"));
    let expected = format!("--campaigns {}", s(&sb.base));
    assert!(
        sb.hook_s("s1").expect("a block")["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains(&expected)
    );
    sb.ok(&["phase", "retry", "a1", "prepare"]);
    sb.ok(&["monitor", "reset"]); // a new session takes over
    for _ in 0..2 {
        sb.hook_s("s2");
    }
    let out = sb.hook_s("s2").expect("a block"); // the retry is spent: do not ask for it again
    assert!(out["reason"].as_str().unwrap().contains("cell stop a1"));
    assert_eq!(
        pluck(&sb.events("hang"), "directive"),
        json!(["retry", "bypass"])
    );
}

#[test]
fn briefing_due_by_hours() {
    let sb = Sandbox::with(json!({}), true, &["--briefing-every-hours", "1"]);
    assert_eq!(sb.events("campaign-open")[0]["briefing_source"], "user");
    assert_eq!(sb.ok(&["status"])["briefing_due"]["hours"], false);
    let mut opened = sb.all_events()[0].clone();
    opened["utc"] = json!(utc_iso_hours_ago(2));
    fs::write(
        sb.inst.join("journey.jsonl"),
        serde_json::to_string(&opened).unwrap() + "\n",
    )
    .unwrap();
    assert_eq!(sb.ok(&["status"])["briefing_due"]["hours"], true);
    sb.ok(&["phase", "start", "a1", "prepare"]);
    assert!(
        sb.hook().expect("a block")["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("브리핑")
    );
    sb.ok(&["brief", "--done", "briefing.md"]);
    assert_eq!(sb.ok(&["status"])["briefing_due"]["hours"], false);
}
