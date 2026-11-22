//! `package_manifest`: write or verify PACKAGE_MANIFEST.json.
//!
//!     target/release/package_manifest [package root]           # write (overwrite)
//!     target/release/package_manifest --check [package root]   # file list and sizes; exit 1 on any difference
//!     target/release/package_manifest --check <installed root> --against <repository> --ref <tag>
//!         # also compare every installed file byte for byte with the tag's blob
//!
//! The manifest holds no digests. The default root is the skill folder of this executable.

use std::path::PathBuf;

use progress_monitor::jsonio::pretty;
use progress_monitor::manifest::{against, build, diff, MANIFEST};
use progress_monitor::paths::{normalise, resolve, Paths};

fn root_of(rest: &[String]) -> Result<PathBuf, String> {
    match rest.first() {
        Some(arg) => resolve(&normalise(arg)),
        None => Paths::discover().map(|p| p.skill_dir),
    }
}

fn run(args: &[String]) -> Result<i32, String> {
    let mut check = false;
    let mut repo = None;
    let mut reference = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--against" | "--ref" => {
                let Some(value) = it.next() else {
                    eprintln!("{arg} needs a value");
                    return Ok(2);
                };
                if arg == "--against" {
                    repo = Some(value.clone());
                } else {
                    reference = Some(value.clone());
                }
            }
            _ => rest.push(arg.clone()),
        }
    }
    if repo.is_some() != reference.is_some() {
        eprintln!("--against and --ref go together");
        return Ok(2);
    }
    if repo.is_some() && !check {
        eprintln!("--against compares an installed copy; it needs --check");
        return Ok(2);
    }
    let root = root_of(&rest)?;
    if check {
        if !root.join(MANIFEST).is_file() {
            println!("MANIFEST MISSING: {MANIFEST}");
            return Ok(1);
        }
        let mut found = diff(&root)?;
        let mut same = String::new();
        if let (Some(repo), Some(reference)) = (&repo, &reference) {
            found.extend(against(&root, &resolve(&normalise(repo))?, reference)?);
            same = format!(", same bytes as {reference}");
        }
        if found.is_empty() {
            println!("MANIFEST OK ({} files{same})", build(&root)?["file_count"]);
            return Ok(0);
        }
        println!("MANIFEST MISMATCH: {}", found.join(", "));
        return Ok(1);
    }
    let manifest = build(&root)?;
    let path = root.join(MANIFEST);
    std::fs::write(&path, pretty(&manifest) + "\n")
        .map_err(|e| format!("{}: {e}", path.display()))?;
    println!("MANIFEST written ({} files)", manifest["file_count"]);
    Ok(0)
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
