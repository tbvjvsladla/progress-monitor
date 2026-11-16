//! progress-monitor executor for a two-layer campaign.
//!
//! It runs only the checks the campaign declares, and writes phase states, gate
//! records, receipts and journey events. It also decides whether a Stop hook should
//! keep the main agent going. It never judges acceptance, and it is not a trust
//! boundary: the owner of the files can still edit them by hand.
//!
//! Exit codes: 0 ok, 1 a check failed or the request was refused, 2 usage or
//! unreadable input. `stop-hook` always exits 0.
//!
//! The rules implemented here are written in references/ (gate.md, monitor.md,
//! direction.md). File formats are in assets/campaigns/README.md.

pub mod campaign;
pub mod checks;
pub mod cli;
pub mod commands;
pub mod error;
pub mod hook;
pub mod init;
pub mod jsonio;
pub mod lock;
pub mod paths;
pub mod proc;
pub mod py;
pub mod status;
pub mod time;

pub const TOOL: &str = "progress-monitor";
pub const SCHEMA: i64 = 2;
pub const SUFFIX: &str = ".status.json";
pub const FINAL: &str = "_final";
pub const SECTIONS: [&str; 6] = [
    "목표",
    "확인된 사실",
    "적용한 변경",
    "검증",
    "막다른 길",
    "남은 계획",
];
pub const REVIEW: [&str; 4] = ["none", "user-request", "high-impact", "judgment"];

/// Events only the executor may write; `event` refuses them.
pub const RESERVED_EVENTS: [&str; 23] = [
    "campaign-open",
    "phase-start",
    "phase-retry",
    "phase-close",
    "check",
    "gate",
    "final",
    "gate-reopen",
    "cell-add",
    "cell-cancel",
    "regroup",
    "cell-reopen",
    "cell-stop",
    "cell-resume",
    "cell-close",
    "review",
    "hitl",
    "hitl-answer",
    "invalidate",
    "explore-open",
    "hang",
    "monitor-release",
    "brief",
];

pub const GROUP_CANCELLED: &str = "취소";
pub const GROUP_STOPPED: &str = "멈춤";
pub const GROUP_WAITING: &str = "기다림";
pub const GROUP_PASSED: &str = "통과";
pub const GROUP_FAILED: &str = "실패";
pub const GROUP_ACTIVE: &str = "진행";

pub(crate) fn status_cmd(camp: &mut campaign::Campaign, _args: &cli::Args) -> commands::Out {
    status::build_status(camp)
}

pub(crate) fn metrics_cmd(camp: &mut campaign::Campaign, _args: &cli::Args) -> commands::Out {
    status::build_metrics(camp)
}
