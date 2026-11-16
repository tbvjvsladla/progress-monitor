//! `sanitize_check`: distribution hygiene check (for the publisher).
//!
//! Scans the package tree for site names, personal home paths, private IPs, e-mail
//! addresses and the identifiers of the projects this package was extracted from.
//! Usage: target/release/sanitize_check [package root]
//! Exit 0 = no hit, 1 = hits found. The patterns live in `progress_monitor::sanitize` only.

use std::path::PathBuf;

use progress_monitor::paths::{normalise, resolve, Paths};
use progress_monitor::sanitize::scan;

fn run(args: &[String]) -> Result<i32, String> {
    let root: PathBuf = match args.first() {
        Some(arg) => resolve(&normalise(arg))?,
        None => Paths::discover()?.skill_dir,
    };
    let hits = scan(&root)?;
    for hit in &hits {
        println!("{}:{}: {}", hit.rel, hit.line, hit.name);
    }
    let verdict = if hits.is_empty() { "OK" } else { "FAIL" };
    println!("SANITIZE {verdict} ({} hits)", hits.len());
    Ok(if hits.is_empty() { 0 } else { 1 })
}

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = run(&args).unwrap_or_else(|err| {
        eprintln!("{err}");
        1
    });
    std::process::exit(code);
}
