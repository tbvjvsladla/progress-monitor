//! The final run: its own receipts, when it may run, when it resets.

mod common;

use common::{pluck, Sandbox};
use serde_json::json;

fn settle(sb: &Sandbox) {
    sb.ok(&["cell", "cancel", "a1", "--reason", "out of scope"]);
    sb.ok(&["cell", "cancel", "a2", "--reason", "out of scope"]);
}

#[test]
fn final_ignores_gate_receipts() {
    let sb = Sandbox::new();
    settle(&sb);
    sb.edit_decl(|d| d["final"]["checks"] = json!(["beta-unit"]));
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    let data = sb.ok(&["final", "run"]);
    assert_eq!(sb.outcome(&data, "beta-unit")["outcome"], "ran"); // the gate PASS is not reused
    assert_eq!(
        pluck(&sb.receipts("beta-unit"), "scope"),
        json!(["gate", "final"])
    );

    let other = Sandbox::new();
    settle(&other);
    other.set_group("g-b", &["flaky"]);
    other.edit_decl(|d| d["final"]["checks"] = json!(["flaky"]));
    other.flag("bad");
    other.finish(&["b1"]);
    other.no(&["gate", "run", "g-b"]); // FAIL in the gate
    other.ok(&["cell", "stop", "b1", "--reason", "handed over"]);
    other.flag("flag-ok");
    let data = other.ok(&["final", "run"]); // not refused by the gate FAIL
    let entry = other.outcome(&data, "flaky");
    assert_eq!(
        (entry["outcome"].clone(), entry["result"].clone()),
        (json!("ran"), json!("PASS"))
    );
}

#[test]
fn final_already_passed() {
    let sb = Sandbox::new();
    settle(&sb);
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    let first = sb.ok(&["final", "run"]);
    assert_eq!(sb.outcome(&first, "full")["outcome"], "ran");
    let second = sb.ok(&["final", "run"]);
    assert_eq!(second["note"], "already-passed");
    assert_eq!(sb.receipts("full").len(), 1);
    sb.commit("beta/state.txt", "beta-ok\nlater change\n", "change");
    let third = sb.ok(&["final", "run"]);
    assert_eq!(sb.outcome(&third, "full")["outcome"], "ran");
    assert_eq!(sb.receipts("full").len(), 2);
}

#[test]
fn final_requires_groups_settled() {
    let sb = Sandbox::new();
    assert_eq!(sb.no(&["final", "run"])["refused"], "groups-not-settled"); // groups in progress
    settle(&sb);
    sb.set_group("g-b", &["flaky"]);
    sb.flag("bad");
    sb.finish(&["b1"]);
    sb.no(&["gate", "run", "g-b"]);
    assert_eq!(sb.group_state("g-b"), "실패");
    assert_eq!(sb.no(&["final", "run"])["refused"], "groups-not-settled");
    sb.ok(&["cell", "stop", "b1", "--reason", "handed over"]);
    assert_eq!(
        (sb.group_state("g-a"), sb.group_state("g-b")),
        (json!("취소"), json!("멈춤"))
    );
    assert_eq!(sb.ok(&["final", "run"])["state"], "passed");
}

#[test]
fn final_reset_on_gate_reopen() {
    let sb = Sandbox::new();
    settle(&sb);
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    sb.ok(&["final", "run"]);
    assert_eq!(sb.gate("_final")["state"], "passed");
    sb.ok(&["gate", "reopen", "g-b", "--reason", "look again"]);
    assert_eq!(sb.gate("_final")["state"], "pending");
    sb.ok(&["gate", "run", "g-b"]);
    assert!(sb.kinds().iter().any(|k| k == "final"));
    let asked = sb.ok(&[
        "hitl",
        "open",
        "--final",
        "--kind",
        "criteria",
        "--question",
        "is this enough?",
    ]);
    assert_eq!(sb.gate("_final")["state"], "waiting");
    assert!(!sb.kinds().iter().any(|k| k == "final"));
    let id = asked["id"].as_str().unwrap();
    sb.ok(&["hitl", "answer", id, "--ref", "docs/answer.md"]);
    sb.ok(&["final", "run"]);
    sb.commit("beta/state.txt", "beta-ok\nlater change\n", "change"); // passed, but the input moved on
    assert!(sb.kinds().iter().any(|k| k == "final"));
    // the bypass the hook asks for
    sb.ok(&[
        "hitl",
        "open",
        "--final",
        "--kind",
        "hang",
        "--question",
        "?",
    ]);
    assert_eq!(sb.gate("_final")["state"], "waiting");
}
