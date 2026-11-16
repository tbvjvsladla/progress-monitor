//! One check: when it runs, when it is reused, covered or refused.

mod common;

use std::collections::BTreeSet;
use std::fs;

use common::{pluck, Sandbox};
use serde_json::json;

#[test]
fn uncommitted_input_always_runs() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    fs::write(
        sb.target.join("beta").join("state.txt"),
        "beta-ok\nedited, not committed\n",
    )
    .unwrap();
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "ran"
    );
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "ran"
    );
    assert_eq!(
        pluck(&sb.receipts("beta-unit"), "reusable"),
        json!([false, false])
    );
}

#[test]
fn empty_inputs_always_runs() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"inputs": []}));
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "ran"
    );
    assert_eq!(
        pluck(&sb.receipts("beta-unit"), "reusable"),
        json!([false, false])
    );
}

#[test]
fn ignored_files_do_not_dirty() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    fs::create_dir(sb.target.join("beta").join("scratch")).unwrap();
    fs::write(
        sb.target.join("beta").join("scratch").join("cache.bin"),
        "ignored\n",
    )
    .unwrap();
    sb.ok(&["gate", "run", "g-b"]);
    assert_eq!(sb.receipts("beta-unit")[0]["reusable"], true);
    fs::write(
        sb.target.join("beta").join("new.txt"),
        "untracked, not ignored\n",
    )
    .unwrap();
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    sb.ok(&["gate", "run", "g-b"]);
    assert_eq!(sb.receipts("beta-unit")[1]["reusable"], false);
}

#[test]
fn unresolved_input_always_runs() {
    // a glob, a missing path, an ignored path
    for inputs in [["beta/*.txt"], ["beta/typo.txt"], ["scratch"]] {
        let sb = Sandbox::new();
        sb.set_check("beta-unit", json!({ "inputs": inputs }));
        sb.finish(&["b1"]);
        sb.ok(&["gate", "run", "g-b"]);
        sb.commit("beta/state.txt", "broken\n", "change");
        sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
        let data = sb.no(&["gate", "run", "g-b"]); // must run and fail, not be reused
        let entry = sb.outcome(&data, "beta-unit");
        assert_eq!(
            (entry["outcome"].clone(), entry["result"].clone()),
            (json!("ran"), json!("FAIL")),
            "{inputs:?}"
        );
        assert_eq!(
            pluck(&sb.receipts("beta-unit"), "reusable"),
            json!([false, false]),
            "{inputs:?}"
        );
    }
    let nested = Sandbox::new(); // a plain folder inside another repository
    let inner = nested.target.join("scratch").join("inner");
    fs::create_dir_all(inner.join("beta")).unwrap();
    fs::write(inner.join("beta").join("state.txt"), "beta-ok\n").unwrap();
    nested.edit_decl(|d| {
        d["target"]["root"] = json!("target/scratch/inner");
    });
    nested.finish(&["b1"]);
    nested.ok(&["gate", "run", "g-b"]);
    fs::write(inner.join("beta").join("state.txt"), "broken\n").unwrap();
    nested.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        nested.outcome(&nested.no(&["gate", "run", "g-b"]), "beta-unit")["result"],
        "FAIL"
    );
}

#[test]
fn input_changed_during_run_is_not_reusable() {
    let sb = Sandbox::new();
    sb.set_check(
        "beta-unit",
        json!({"cmd": "echo more >> beta/state.txt; git add -A; \
                       git -c user.name=t -c user.email=t@example.invalid commit -q -m during; \
                       cat beta/state.txt"}),
    );
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]); // the check's own input moved while it ran
    let receipt = &sb.receipts("beta-unit")[0];
    assert_eq!(
        (receipt["reusable"].clone(), receipt["reason"].clone()),
        (json!(false), json!("input-changed-during-run"))
    );
    let covering = Sandbox::new(); // a narrow check with uncommitted input
    covering.set_check("alpha-unit", json!({"inputs": ["beta"]}));
    covering.finish(&["a1", "a2"]);
    fs::write(
        covering.target.join("beta").join("state.txt"),
        "not committed\n",
    )
    .unwrap();
    let data = covering.ok(&["gate", "run", "g-a"]);
    assert_eq!(covering.outcome(&data, "alpha-unit")["outcome"], "ran");
    assert_eq!(covering.receipts("alpha-suite")[0]["covers"], json!({}));
}

#[test]
fn same_key_pass_is_reused() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    let entry = sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit");
    assert_eq!(
        (entry["outcome"].clone(), entry["receipt"].clone()),
        (json!("reused"), json!("receipts/beta-unit/1.json"))
    );
    assert_eq!(sb.receipts("beta-unit").len(), 1);
    assert_eq!(sb.gate("g-b")["state"], "passed");
}

#[test]
fn same_key_fail_is_refused() {
    let sb = Sandbox::new();
    sb.commit("beta/state.txt", "broken\n", "change");
    sb.finish(&["b1"]);
    let first = sb.no(&["gate", "run", "g-b"]);
    let entry = sb.outcome(&first, "beta-unit");
    assert_eq!(
        (entry["outcome"].clone(), entry["result"].clone()),
        (json!("ran"), json!("FAIL"))
    );
    assert_eq!(sb.gate("g-b")["state"], "failed");
    let second = sb.no(&["gate", "run", "g-b"]);
    assert_eq!(sb.outcome(&second, "beta-unit")["outcome"], "refused");
    assert_eq!(sb.all_receipts().len(), 1);
}

#[test]
fn latest_verdict_wins() {
    let sb = Sandbox::with(
        json!({"forced_reruns_per_check": 5, "forced_reruns_total": 5}),
        true,
        &[],
    );
    sb.set_group("g-b", &["flaky"]);
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]); // PASS
    sb.ok(&["gate", "reopen", "g-b", "--reason", "doubt"]);
    sb.flag("bad");
    let forced = sb.no(&[
        "gate", "run", "g-b", "--force", "flaky", "--reason", "doubt",
    ]);
    let entry = sb.outcome(&forced, "flaky");
    assert_eq!(
        (entry["outcome"].clone(), entry["result"].clone()),
        (json!("forced"), json!("FAIL"))
    );
    let again = sb.no(&["gate", "run", "g-b"]); // the old PASS must not be reused
    assert_eq!(sb.outcome(&again, "flaky")["outcome"], "refused");
    sb.flag("flag-ok");
    let fixed = sb.ok(&[
        "gate",
        "run",
        "g-b",
        "--force",
        "flaky",
        "--reason",
        "environment fixed",
    ]);
    assert_eq!(sb.outcome(&fixed, "flaky")["result"], "PASS");
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "flaky")["outcome"],
        "reused"
    );
    assert_eq!(sb.receipts("flaky").len(), 3);
}

#[test]
fn own_fail_beats_cover() {
    let sb = Sandbox::new();
    sb.set_check("alpha-unit", json!({"cmd": "cat scratch/flag.txt"}));
    sb.set_check("alpha-unit", json!({"expect": "flag-ok"}));
    sb.set_group("g-a", &["alpha-unit"]);
    sb.flag("bad");
    sb.finish(&["a1", "a2"]);
    sb.no(&["gate", "run", "g-a"]); // alpha-unit fails on its own key
    sb.set_group("g-a", &["alpha-suite", "alpha-unit"]);
    let data = sb.no(&["gate", "run", "g-a"]);
    let suite = sb.outcome(&data, "alpha-suite");
    assert_eq!(
        (suite["outcome"].clone(), suite["result"].clone()),
        (json!("ran"), json!("PASS"))
    );
    assert_eq!(sb.outcome(&data, "alpha-unit")["outcome"], "refused");
    assert_eq!(sb.gate("g-a")["state"], "failed");
}

#[test]
fn cover_requires_same_inputs() {
    let sb = Sandbox::new();
    sb.set_check("alpha-unit", json!({"inputs": ["alpha", "beta"]}));
    sb.finish(&["a1", "a2"]);
    let first = sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.outcome(&first, "alpha-unit")["outcome"], "covered");
    assert_eq!(
        sb.outcome(&first, "alpha-unit")["via"],
        "receipts/alpha-suite/1.json"
    );
    sb.commit("beta/state.txt", "beta-ok\nmore\n", "change");
    sb.ok(&["gate", "reopen", "g-a", "--reason", "input changed"]);
    let second = sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.outcome(&second, "alpha-suite")["outcome"], "reused");
    assert_eq!(sb.outcome(&second, "alpha-unit")["outcome"], "ran");
}

#[test]
fn cover_ignores_command_difference() {
    let sb = Sandbox::new();
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    sb.ok(&["gate", "reopen", "g-a", "--reason", "again"]);
    sb.set_check(
        "alpha-unit",
        json!({"cmd": "cat alpha/state.txt; echo another build"}),
    );
    let data = sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.outcome(&data, "alpha-unit")["outcome"], "covered");
    assert_eq!(sb.receipts("alpha-unit").len(), 0);
}

#[test]
fn error_is_not_fail() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"cmd": "sleep 5", "timeout_s": 1}));
    sb.finish(&["b1"]);
    let data = sb.no(&["gate", "run", "g-b"]);
    assert_eq!(sb.outcome(&data, "beta-unit")["outcome"], "error");
    sb.set_check("beta-unit", json!({"cmd": "exit 127"}));
    sb.no(&["gate", "run", "g-b"]);
    // the check shell kills itself with SIGKILL
    sb.set_check("beta-unit", json!({"cmd": "kill -KILL $$"}));
    sb.no(&["gate", "run", "g-b"]);
    let got: Vec<_> = sb
        .receipts("beta-unit")
        .iter()
        .map(|r| (r["result"].clone(), r["error"].clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (json!("ERROR"), json!("timeout")),
            (json!("ERROR"), json!("not-found")),
            (json!("ERROR"), json!("signal")),
        ]
    );
    assert_eq!(sb.gate("g-b")["state"], "pending");
}

#[test]
fn expect_marker_required() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"cmd": "echo hello"}));
    sb.finish(&["b1"]);
    let data = sb.no(&["gate", "run", "g-b"]);
    assert_eq!(sb.outcome(&data, "beta-unit")["result"], "FAIL");
    assert_eq!(sb.receipts("beta-unit")[0]["exit_code"], 0);
}

#[test]
fn exec_retry_cap() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"cmd": "exit 127"}));
    sb.finish(&["b1"]);
    for _ in 0..3 {
        let data = sb.no(&["gate", "run", "g-b"]);
        assert_eq!(sb.outcome(&data, "beta-unit")["outcome"], "error");
        assert_eq!(sb.gate("g-b")["state"], "pending");
    }
    let data = sb.no(&["gate", "run", "g-b"]);
    assert_eq!(sb.outcome(&data, "beta-unit")["outcome"], "refused");
    assert_eq!(sb.receipts("beta-unit").len(), 3);
    assert_eq!(pluck(&sb.events("hitl"), "kind"), json!(["exec-retry-cap"]));
    assert_eq!(sb.gate("g-b")["state"], "waiting");
    let fails: Vec<_> = sb
        .all_receipts()
        .into_iter()
        .filter(|r| r["result"] == "FAIL")
        .collect();
    assert_eq!(fails, Vec::<serde_json::Value>::new());
}

#[test]
fn forced_rerun_caps() {
    let sb = Sandbox::new(); // per check 1, total 3
    sb.set_group("g-b", &["flaky"]);
    sb.flag("bad");
    sb.finish(&["b1"]);
    sb.no(&["gate", "run", "g-b"]);
    assert_eq!(
        sb.outcome(
            &sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r1"]),
            "flaky"
        )["outcome"],
        "forced"
    );
    let refused = sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r2"]);
    assert_eq!(
        (refused["refused"].clone(), refused["cap"].clone()),
        (json!("forced-rerun-cap"), json!("per-check"))
    );
    assert_eq!(sb.gate("g-b")["state"], "failed"); // a refused run leaves the gate alone
    assert_eq!(sb.receipts("flaky").len(), 2);
    assert_eq!(
        pluck(&sb.events("hitl"), "kind"),
        json!(["forced-rerun-cap"])
    );
    sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r2"]);
    assert_eq!(sb.events("hitl").len(), 1); // the open question is not repeated
    let hitl = refused["hitl"].as_str().unwrap();
    sb.ok(&["hitl", "answer", hitl, "--ref", "docs/answer.md", "--allow"]);
    sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r3"]);
    assert_eq!(sb.receipts("flaky").len(), 3);
    assert_eq!(
        sb.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r4"])["cap"],
        "per-check"
    );

    let total = Sandbox::with(
        json!({"forced_reruns_per_check": 9, "forced_reruns_total": 2}),
        true,
        &[],
    );
    total.set_group("g-b", &["flaky"]);
    total.flag("bad");
    total.finish(&["b1"]);
    total.no(&["gate", "run", "g-b"]);
    for _ in 0..2 {
        total.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r"]);
    }
    let refused = total.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r"]);
    assert_eq!(refused["cap"], "total");
    let hitl = refused["hitl"].as_str().unwrap();
    total.ok(&["hitl", "answer", hitl, "--ref", "docs/answer.md", "--allow"]);
    total.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r"]);
    assert_eq!(total.receipts("flaky").len(), 4);
    assert_eq!(
        total.no(&["gate", "run", "g-b", "--force", "flaky", "--reason", "r"])["cap"],
        "total"
    );
}

#[test]
fn invalidate_blocks_reuse_and_cover() {
    let sb = Sandbox::new();
    sb.finish(&["a1", "a2"]);
    sb.ok(&["gate", "run", "g-a"]);
    sb.ok(&[
        "check",
        "invalidate",
        "alpha-unit",
        "--reason",
        "an input was missing",
    ]);
    sb.ok(&["gate", "reopen", "g-a", "--reason", "invalidated"]);
    let data = sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.outcome(&data, "alpha-suite")["outcome"], "reused");
    assert_eq!(sb.outcome(&data, "alpha-unit")["outcome"], "ran"); // the earlier cover is not used
    sb.ok(&[
        "check",
        "invalidate",
        "alpha-suite",
        "--reason",
        "an input was missing",
    ]);
    sb.ok(&["gate", "reopen", "g-a", "--reason", "invalidated"]);
    let data = sb.ok(&["gate", "run", "g-a"]);
    assert_eq!(sb.outcome(&data, "alpha-suite")["outcome"], "ran"); // the earlier PASS is not reused
    assert_eq!(sb.outcome(&data, "alpha-unit")["outcome"], "reused");
}

#[test]
fn env_value_changes_key() {
    let sb = Sandbox::new();
    sb.set_check("beta-unit", json!({"env": ["PM_TEST_FLAVOR"]}));
    // per call, not the test process environment: tests run in parallel threads
    sb.set_env("PM_TEST_FLAVOR", "one");
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "reused"
    );
    sb.set_env("PM_TEST_FLAVOR", "two");
    sb.ok(&["gate", "reopen", "g-b", "--reason", "again"]);
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "ran"
    );
    let flavors: Vec<_> = sb
        .receipts("beta-unit")
        .iter()
        .map(|r| r["key"]["env"]["PM_TEST_FLAVOR"].clone())
        .collect();
    assert_eq!(flavors, vec![json!("one"), json!("two")]);
}

#[test]
fn expect_changes_key() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    sb.ok(&["gate", "reopen", "g-b", "--reason", "criterion changed"]);
    sb.set_check("beta-unit", json!({"expect": "beta"}));
    assert_eq!(
        sb.outcome(&sb.ok(&["gate", "run", "g-b"]), "beta-unit")["outcome"],
        "ran"
    );
    let keys: BTreeSet<String> = sb
        .receipts("beta-unit")
        .iter()
        .map(|r| r["key_sha256"].to_string())
        .collect();
    assert_eq!(keys.len(), 2);
}

#[test]
fn stale_display() {
    let sb = Sandbox::new();
    sb.finish(&["b1"]);
    sb.ok(&["gate", "run", "g-b"]);
    let group_of = |sb: &Sandbox| {
        sb.ok(&["status"])["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["group"] == "g-b")
            .cloned()
            .unwrap()
    };
    let group = group_of(&sb);
    assert_eq!(group.get("stale"), Some(&serde_json::Value::Null));
    sb.commit("beta/state.txt", "beta-ok\nchanged elsewhere\n", "change");
    let group = group_of(&sb);
    assert_eq!(
        (
            group["stale"].clone(),
            group["gate"].clone(),
            group["stale_checks"].clone()
        ),
        (json!("STALE"), json!("passed"), json!(["beta-unit"]))
    );
    assert_eq!(sb.gate("g-b")["state"], "passed");
}
