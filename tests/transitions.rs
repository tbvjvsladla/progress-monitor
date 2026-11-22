//! Cell, gate and group states.

mod common;

use std::ffi::CString;
use std::process::Command;

use common::{pluck, s, Sandbox};
use serde_json::json;

#[test]
fn needs_cross_group_requires_gate() {
    let sb = Sandbox::new();
    sb.set_cell("a2", json!({"needs": ["a1"]}));
    sb.set_cell("b1", json!({"needs": ["a1"]}));
    for name in ["a2", "b1"] {
        sb.ok(&["phase", "start", name, "prepare"]);
        sb.ok(&["phase", "close", name, "prepare"]);
    }
    assert_eq!(
        sb.no(&["phase", "start", "a2", "change"])["refused"],
        "conditions-unmet"
    );
    sb.finish(&["a1"]);
    sb.ok(&["phase", "start", "a2", "change"]); // same group: a1's change is done
    sb.ok(&["phase", "close", "a2", "change"]);
    assert_eq!(
        sb.no(&["phase", "start", "b1", "change"])["refused"],
        "conditions-unmet"
    );
    sb.ok(&["gate", "run", "g-a"]);
    sb.ok(&["phase", "start", "b1", "change"]); // other group: its gate has passed
}

#[test]
fn gate_requires_members_done() {
    let sb = Sandbox::new();
    sb.finish(&["a1"]);
    assert_eq!(
        sb.no(&["gate", "run", "g-a"])["refused"],
        "members-not-done"
    );
    assert_eq!(sb.all_receipts(), Vec::<serde_json::Value>::new());
    assert_eq!(sb.ok(&["status"])["groups"][0]["gate"], "pending");
}

#[test]
fn gate_pass_closes_unmarked_only() {
    let sb = Sandbox::new();
    sb.set_cell("a2", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(
        (
            sb.cell("a1")["state"].clone(),
            sb.cell("a2")["state"].clone()
        ),
        (json!("closed"), json!("open"))
    );
    assert_eq!(sb.gate("g-a")["members"], json!(["a1", "a2"]));
}

#[test]
fn gate_reopen_keeps_invariant() {
    let sb = Sandbox::new();
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    sb.assert_lint();
    sb.ok(&[
        "gate",
        "reopen",
        "g-a",
        "--reason",
        "another group touched alpha",
    ]);
    assert_eq!(
        (
            sb.cell("a1")["state"].clone(),
            sb.cell("a2")["state"].clone(),
            sb.gate("g-a")["state"].clone()
        ),
        (json!("open"), json!("open"), json!("pending"))
    );
    sb.assert_lint();
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(
        (
            sb.cell("a1")["state"].clone(),
            sb.cell("a2")["state"].clone()
        ),
        (json!("closed"), json!("closed"))
    );
    sb.assert_lint();
}

#[test]
fn cell_reopen_after_gate_fail() {
    let sb = Sandbox::new();
    sb.commit("alpha/state.txt", "broken\n", "change");
    sb.finish(&["a1", "a2"]);
    sb.no(&["gate", "run", "g-a"]);
    assert_eq!(sb.gate("g-a")["state"], "failed");
    sb.ok(&["cell", "reopen", "a1", "--reason", "a1 broke alpha"]);
    let change = sb.phase("a1", "change");
    assert_eq!(
        (
            change["state"].clone(),
            change["attempt"].clone(),
            change.get("by").cloned()
        ),
        (json!("pending"), json!(2), Some(serde_json::Value::Null))
    );
    assert_eq!(sb.gate("g-a")["state"], "pending");
    sb.assert_lint();
}

#[test]
fn member_added_to_passed_group_reopens() {
    let sb = Sandbox::new();
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(
        sb.no(&["cell", "move", "a1", "g-b", "--reason", "x"])["refused"],
        "cell-not-open"
    );
    sb.ok(&["cell", "add", &sb.spec("a3", "g-a", json!({}))]);
    assert_eq!(
        (
            sb.gate("g-a")["state"].clone(),
            sb.cell("a1")["state"].clone()
        ),
        (json!("pending"), json!("open"))
    );
    sb.finish(&["a3"]);
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.cell("a3")["state"], "closed");
    sb.ok(&[
        "cell",
        "move",
        "b1",
        "g-a",
        "--reason",
        "it changes alpha too",
    ]);
    assert_eq!(
        (
            sb.gate("g-a")["state"].clone(),
            sb.cell("a1")["state"].clone(),
            sb.cell("b1")["group"].clone()
        ),
        (json!("pending"), json!("open"), json!("g-a"))
    );
    sb.assert_lint();
}

#[test]
fn clean_review_before_the_gate_closes_with_the_gate() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["review", "record", "a1", &sb.review(1, true)]); // before the gate passes
    assert_eq!(
        sb.no(&["cell", "close", "a1"])["refused"],
        "conditions-unmet"
    );
    assert_eq!(
        sb.ok(&["gate", "run", "g-a"])["closed"],
        json!(["a1", "a2"])
    );
    assert_eq!(sb.events("review").len(), 1);
    sb.assert_lint();
}

#[test]
fn next_round_only_after_a_fixed_blocking_finding() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    assert_eq!(
        sb.no(&["review", "record", "a1", &sb.review(1, true)])["refused"],
        "cell-not-settled"
    );
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.kinds_of("a1"), ["review"]);
    sb.ok(&["review", "record", "a1", &sb.review(1, false)]);
    assert_eq!(sb.kinds_of("a1"), ["fix"]);
    let refused = sb.no(&["cell", "close", "a1"]);
    assert_eq!(
        (&refused["refused"], &refused["standing"]),
        (&json!("no-standing-clean-review"), &json!("blocking"))
    );
    assert_eq!(
        sb.no(&["review", "record", "a1", &sb.review(2, true)])["refused"],
        "fix-first"
    );
    sb.ok(&["cell", "reopen", "a1", "--reason", "fix r1-1"]);
    sb.ok(&["phase", "start", "a1", "change"]);
    sb.ok(&["phase", "close", "a1", "change"]);
    // a1 has no review of the fix yet
    assert_eq!(sb.ok(&["gate", "run", "g-a"])["closed"], json!(["a2"]));
    sb.ok(&["review", "record", "a1", &sb.review(2, true)]);
    assert_eq!(sb.kinds_of("a1"), ["close"]);
    assert_eq!(
        sb.no(&["review", "record", "a1", &sb.review(3, true)])["refused"],
        "review-not-needed"
    );
    sb.ok(&["cell", "close", "a1"]);
    assert_eq!(sb.cell("a1")["state"], "closed");
    sb.assert_lint();
}

#[test]
fn gate_rerun_by_another_cell_keeps_a_standing_review() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    sb.ok(&["review", "record", "a1", &sb.review(1, true)]);
    sb.ok(&["cell", "close", "a1"]);
    sb.ok(&["cell", "reopen", "a2", "--reason", "a2 changes again"]);
    assert_eq!(sb.cell("a1")["state"], "open");
    sb.ok(&["phase", "start", "a2", "change"]);
    sb.ok(&["phase", "close", "a2", "change"]);
    assert_eq!(
        sb.ok(&["gate", "run", "g-a"])["closed"],
        json!(["a1", "a2"])
    );
    // no second review of the unchanged a1
    assert_eq!(sb.events("review").len(), 1);
    sb.assert_lint();
}

#[test]
fn person_accepts_blocking_findings() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(
        sb.no(&["cell", "close", "a1", "--accept-ref", "docs/x.md"])["refused"],
        "nothing-to-accept"
    );
    sb.ok(&["review", "record", "a1", &sb.review(1, false)]);
    sb.no_code(&["cell", "close", "a1", "--accept-ref", " "], 2);
    let closed = sb.ok(&["cell", "close", "a1", "--accept-ref", "docs/decision.md"]);
    assert_eq!(
        (&closed["state"], &closed["round"]),
        (&json!("closed"), &json!(1))
    );
    let event = sb.events("cell-close").last().unwrap().clone();
    assert_eq!(event["accepted_ref"], "docs/decision.md");
    assert_eq!(
        pluck(event["residual"].as_array().unwrap(), "id"),
        json!(["r1-1"])
    );
    assert_eq!(sb.events("review-accept").len(), 1);
    sb.refix("a2", None); // another cell re-runs the gate
    assert_eq!(
        sb.no(&["review", "record", "a1", &sb.review(2, true)])["refused"],
        "review-not-needed"
    );
    assert_eq!(
        sb.ok(&["gate", "run", "g-a"])["closed"],
        json!(["a1", "a2"])
    );
    sb.assert_lint();
}

#[test]
fn accept_a_cell_stopped_while_its_gate_is_pending() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    for number in [1, 2] {
        sb.ok(&["review", "record", "a1", &sb.review(number, false)]);
        sb.refix("a1", None); // no gate run in between
    }
    assert_eq!(
        sb.ok(&["review", "record", "a1", &sb.review(3, false)])["stopped"],
        true
    );
    let groups = sb.ok(&["status"])["groups"].clone();
    let g_a = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["group"] == "g-a")
        .unwrap()
        .clone();
    assert_eq!(g_a["gate"], "pending");
    let accepted = sb.ok(&["cell", "close", "a1", "--accept-ref", "docs/accept.md"]);
    assert_eq!(
        (&accepted["state"], &accepted["round"]),
        (&json!("open"), &json!(3))
    );
    assert_eq!(
        sb.ok(&["gate", "run", "g-a"])["closed"],
        json!(["a1", "a2"])
    );
    sb.assert_lint();
}

#[test]
fn review_cap_stops_only_if_blocking() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    for number in [1, 2] {
        assert_eq!(
            sb.ok(&["review", "record", "a1", &sb.review(number, false)])["stopped"],
            false
        );
        sb.refix("a1", Some("g-a"));
    }
    assert_eq!(
        sb.ok(&["review", "record", "a1", &sb.review(3, false)])["stopped"],
        true
    );
    assert_eq!(sb.cell("a1")["state"], "stopped");
    sb.ok(&["cell", "resume", "a1", "--ref", "docs/approval.md"]);
    sb.refix("a1", Some("g-a"));
    assert_eq!(
        sb.ok(&["review", "record", "a1", &sb.review(4, false)])["stopped"],
        true
    );
    // a stopped cell a person accepts
    sb.ok(&["cell", "close", "a1", "--accept-ref", "docs/accept.md"]);
    assert_eq!(sb.cell("a1")["state"], "closed");
    sb.assert_lint();

    let clean = Sandbox::new();
    clean.set_cell("a1", json!({"review": "judgment"}));
    clean.finish(&["a1", "a2"]);
    clean.ok(&["gate", "run", "g-a"]);
    for number in [1, 2] {
        clean.ok(&["review", "record", "a1", &clean.review(number, false)]);
        clean.refix("a1", Some("g-a"));
    }
    assert_eq!(
        clean.ok(&["review", "record", "a1", &clean.review(3, true)])["stopped"],
        false
    );
    assert_eq!(clean.cell("a1")["state"], "open");
}

#[test]
fn auto_cell_cap_hands_over() {
    let sb = Sandbox::with(json!({"auto_cells": 1}), true, &[]);
    sb.ok(&["cell", "add", &sb.spec("x1", "g-a", json!({}))]);
    let refused = sb.no(&["cell", "add", &sb.spec("x2", "g-a", json!({}))]);
    assert_eq!(refused["refused"], "auto-cell-cap");
    assert_eq!(pluck(&sb.events("hitl"), "kind"), json!(["auto-cell-cap"]));
    assert!(!sb.inst.join("cells").join("x2").exists());
}

#[test]
fn parentless_cell_review_is_none() {
    let sb = Sandbox::new();
    sb.set_cell("a1", json!({"review": "judgment"}));
    sb.finish(&["a1"]);
    sb.ok(&["review", "record", "a1", &sb.review(1, false)]);
    assert_eq!(
        sb.ok(&["cell", "add", &sb.spec("orphan", "g-a", json!({}))])["review"],
        "none"
    );
    assert_eq!(
        sb.ok(&[
            "cell",
            "add",
            &sb.spec("child", "g-a", json!({"parent": "a1"}))
        ])["review"],
        "judgment"
    );
    sb.finish(&["child"]);
    assert_eq!(
        sb.no(&["review", "record", "child", &sb.review(1, true)])["expected"],
        2
    );
    sb.ok(&["review", "record", "child", &sb.review(2, true)]);
    assert_eq!(sb.cell("child")["origin"], "auto");
}

#[test]
fn cancel_rules() {
    let sb = Sandbox::new();
    sb.ok(&["cell", "stop", "a1", "--reason", "handed over"]);
    sb.ok(&[
        "hitl",
        "open",
        "--cell",
        "a2",
        "--kind",
        "criteria",
        "--question",
        "which way?",
    ]);
    assert_eq!(
        sb.no(&["cell", "cancel", "a1", "--reason", "x"])["state"],
        "stopped"
    );
    assert_eq!(
        sb.no(&["cell", "cancel", "a2", "--reason", "x"])["state"],
        "waiting"
    );
    sb.ok(&["cell", "cancel", "b1", "--reason", "not needed"]);
    assert_eq!(sb.cell("b1")["state"], "cancelled");
    assert!(sb.inst.join("cells").join("b1").join("cell.md").is_file());
    assert_eq!(sb.group_state("g-b"), "취소");
}

#[test]
fn regroup_keeps_counts() {
    let sb = Sandbox::new();
    sb.set_group("g-b", &["flaky"]);
    sb.flag("bad");
    sb.finish(&["b1"]);
    sb.no(&["gate", "run", "g-b"]);
    sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "once"]);
    sb.ok(&[
        "cell",
        "move",
        "b1",
        "g-c",
        "--reason",
        "verified separately",
        "--checks",
        "flaky",
    ]);
    assert_eq!(sb.cell("b1")["group"], "g-c");
    let refused = sb.no(&[
        "gate", "run", "g-c", "--force", "flaky", "--reason", "twice",
    ]);
    assert_eq!(refused["refused"], "forced-rerun-cap"); // the count followed the check
    sb.assert_lint();
}

#[test]
fn waiting_cell_does_not_block_others() {
    let sb = Sandbox::new();
    let asked = sb.ok(&[
        "hitl",
        "open",
        "--cell",
        "a1",
        "--kind",
        "criteria",
        "--question",
        "which way?",
    ]);
    assert_eq!(sb.cell("a1")["state"], "waiting");
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    assert_eq!(
        (sb.group_state("g-a"), sb.group_state("g-b")),
        (json!("기다림"), json!("통과"))
    );
    let id = asked["id"].as_str().unwrap();
    sb.ok(&["hitl", "answer", id, "--ref", "docs/answer.md"]);
    assert_eq!(sb.cell("a1")["state"], "open");
}

#[test]
fn cell_resume_requires_ref() {
    let sb = Sandbox::new();
    sb.ok(&["cell", "stop", "a1", "--reason", "handed over"]);
    sb.no_code(&["cell", "resume", "a1"], 2);
    assert_eq!(sb.cell("a1")["state"], "stopped");
    sb.ok(&["cell", "resume", "a1", "--ref", "docs/approval.md"]);
    assert_eq!(sb.cell("a1")["state"], "open");
}

#[test]
fn explore_caps() {
    let sb = Sandbox::with(
        json!({"explore_candidates": 1, "explore_rounds": 1}),
        true,
        &[],
    );
    let target = s(&sb.target);
    assert_eq!(
        sb.no(&[
            "check",
            "run",
            "beta-unit",
            "--explore",
            "x1",
            "--target",
            &target
        ])["refused"],
        "no-open-exploration"
    );
    let opened = sb.ok(&[
        "explore",
        "open",
        "--cell",
        "b1",
        "--reason",
        "two ways to do it",
    ]);
    let id = opened["id"].as_str().unwrap().to_string();
    let mut trees = Vec::new();
    for name in ["one", "two"] {
        let tree = sb.tmp.join("worktrees").join(name);
        sb.git(&["worktree", "add", "-q", "--detach", &s(&tree)]);
        sb.commit_in(
            "beta/state.txt",
            &format!("beta-ok {name}\n"),
            &format!("candidate {name}"),
            &tree,
        );
        trees.push(s(&tree));
    }
    // baseline
    sb.ok(&[
        "check",
        "run",
        "beta-unit",
        "--explore",
        &id,
        "--target",
        &target,
    ]);
    sb.ok(&[
        "check",
        "run",
        "beta-unit",
        "--explore",
        &id,
        "--target",
        &trees[0],
    ]);
    let refused = sb.no(&[
        "check",
        "run",
        "beta-unit",
        "--explore",
        &id,
        "--target",
        &trees[1],
    ]);
    assert_eq!(refused["refused"], "explore-candidates-spent");
    assert_eq!(
        pluck(&sb.receipts("beta-unit"), "scope"),
        json!(["explore", "explore"])
    );
    sb.commit_in(
        "beta/state.txt",
        "broken\n",
        "candidate two, broken",
        std::path::Path::new(&trees[0]),
    );
    let failed = sb.no(&[
        "check",
        "run",
        "beta-unit",
        "--explore",
        &id,
        "--target",
        &trees[0],
    ]);
    // exit 1, not a crash
    assert_eq!(
        (failed["refused"].clone(), failed["result"].clone()),
        (json!("check-ran"), json!("FAIL"))
    );
    let again = sb.no(&[
        "check",
        "run",
        "beta-unit",
        "--explore",
        &id,
        "--target",
        &trees[0],
    ]);
    assert_eq!(again["refused"], "check-refused");
    sb.ok(&[
        "explore",
        "close",
        &id,
        "--selected",
        "one",
        "--ref",
        "notes/explore.md",
    ]);
    assert_eq!(
        sb.no(&["explore", "open", "--cell", "b1", "--reason", "again"])["refused"],
        "explore-rounds-spent"
    );
    assert_eq!(sb.events("direction")[0]["selected"], "one");
}

#[test]
fn hitl_group_refused_on_passed_gate() {
    let sb = Sandbox::new();
    sb.set_cell("a2", json!({"review": "judgment"}));
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    let refused = sb.no(&[
        "hitl",
        "open",
        "--group",
        "g-a",
        "--kind",
        "hang",
        "--question",
        "?",
    ]);
    assert_eq!(refused["refused"], "gate-passed");
    sb.ok(&["cell", "stop", "a2", "--reason", "hang"]); // the stalled review stops its cell
    assert_eq!(
        (
            sb.cell("a1")["state"].clone(),
            sb.gate("g-a")["state"].clone(),
            sb.group_state("g-a")
        ),
        (json!("closed"), json!("passed"), json!("멈춤"))
    );
    sb.assert_lint();
    let asked = sb.ok(&[
        "hitl",
        "open",
        "--group",
        "g-b",
        "--kind",
        "hang",
        "--question",
        "?",
    ]);
    assert_eq!(sb.gate("g-b")["state"], "waiting");
    sb.finish(&["b1"]);
    assert_eq!(sb.no(&["gate", "run", "g-b"])["refused"], "gate-waiting");
    let id = asked["id"].as_str().unwrap();
    sb.ok(&["hitl", "answer", id, "--ref", "docs/answer.md"]);
    assert_eq!(sb.gate("g-b")["state"], "pending");
    sb.ok(&["gate", "run", "g-b"]);
    sb.assert_lint();
}

#[test]
fn gate_error_keeps_pending() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"cmd": "exit 127"}));
    sb.finish(&["b1"]);
    assert_eq!(sb.no(&["gate", "run", "g-b"])["state"], "pending");
    sb.commit("alpha/state.txt", "broken\n", "change");
    sb.finish(&["a1", "a2"]);
    let data = sb.no(&["gate", "run", "g-a"]);
    let got: Vec<_> = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["check"].clone(),
                item["outcome"].clone(),
                item["result"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (json!("alpha-suite"), json!("ran"), json!("FAIL")),
            (json!("alpha-unit"), json!("ran"), json!("FAIL")),
        ]
    );
    assert_eq!(data["state"], "failed");
}

/// A child process that takes `flock(LOCK_EX)` on `path`, reports it on a pipe, and waits.
/// Returns its pid after reading the report.
fn spawn_lock_holder(path: &std::path::Path) -> (libc::pid_t, String) {
    let c_path = CString::new(s(path)).unwrap();
    let mut fds = [0 as libc::c_int; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
    let (read_end, write_end) = (fds[0], fds[1]);
    let message = b"held\n";
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork");
    if pid == 0 {
        // Child: async-signal-safe calls only.
        unsafe {
            libc::close(read_end);
            let fd = libc::open(
                c_path.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
                0o644,
            );
            if fd < 0 || libc::flock(fd, libc::LOCK_EX) != 0 {
                libc::_exit(3);
            }
            libc::write(write_end, message.as_ptr().cast(), message.len());
            loop {
                libc::pause();
            }
        }
    }
    unsafe { libc::close(write_end) };
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = unsafe { libc::read(read_end, byte.as_mut_ptr().cast(), 1) };
        if n <= 0 || byte[0] == b'\n' {
            break;
        }
        got.push(byte[0]);
    }
    unsafe { libc::close(read_end) };
    (pid, String::from_utf8_lossy(&got).trim().to_string())
}

/// Kills the holder if the test ends before it does.
struct Holder(libc::pid_t);

impl Holder {
    fn kill(&mut self) {
        if self.0 > 0 {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.0, &mut status, 0);
            }
            self.0 = 0;
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.kill();
    }
}

#[test]
fn dead_gate_run_recovers() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    let mut dead = Command::new("true").spawn().expect("spawn true");
    dead.wait().unwrap();
    let dead_pid = dead.id();
    let record = json!({"schema_version": 2, "group": "g-b", "state": "pending", "runs": 1,
                        "started": "2026-01-01T00:00:00+00:00", "ended": null, "members": [],
                        "hitl": null, "checks": [], "pid": 1});
    let _ = std::fs::create_dir(sb.inst.join("gates"));
    Sandbox::write(&sb.inst.join("gates").join("g-b.status.json"), &record);
    assert_eq!(sb.no(&["gate", "run", "g-b"])["refused"], "gate-running"); // pid 1 is alive
    let mut with_dead = record.clone();
    with_dead["pid"] = json!(dead_pid);
    Sandbox::write(&sb.inst.join("gates").join("g-b.status.json"), &with_dead);
    let (pid, report) = spawn_lock_holder(&sb.inst.join(".lock"));
    let mut holder = Holder(pid);
    assert_eq!(report, "held");
    holder.kill(); // the process dies holding the lock
    assert_eq!(sb.ok(&["gate", "run", "g-b"])["state"], "passed");
    let gate = sb.gate("g-b");
    assert!(
        matches!(gate.get("pid"), None | Some(serde_json::Value::Null)),
        "{gate}"
    );
}

#[test]
fn allow_raises_cap() {
    let sb = Sandbox::with(json!({"auto_cells": 1}), true, &[]);
    sb.ok(&["cell", "add", &sb.spec("x1", "g-a", json!({}))]);
    let refused = sb.no(&["cell", "add", &sb.spec("x2", "g-a", json!({}))]);
    let hitl = refused["hitl"].as_str().unwrap();
    sb.ok(&["hitl", "answer", hitl, "--ref", "docs/answer.md", "--allow"]);
    sb.ok(&["cell", "add", &sb.spec("x2", "g-a", json!({}))]);
    assert_eq!(
        sb.no(&["cell", "add", &sb.spec("x3", "g-a", json!({}))])["refused"],
        "auto-cell-cap"
    );
    sb.assert_lint(); // auto-cap-exceeded counts the answer
}
