//! Installing the package into a project: the copy, the built executables, a temporary folder.
//! Used by `smoke_install` and the distribution tests.

use std::ffi::{CString, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// Names never copied into an install (at any depth). `target` is the build output.
pub const SKIP: [&str; 5] = [".git", "__pycache__", ".cache", "node_modules", "target"];

/// The executables an install needs in `<skill>/target/release/`.
pub const BINARIES: [&str; 2] = ["pm", "check_campaign"];

/// Copy the tree `src` to the new folder `dst`, leaving out entries named in `skip`.
/// Links are followed: a link to a file is copied as the file, a link to a folder as the folder.
pub fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<(), String> {
    let fail = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    fs::create_dir_all(dst).map_err(|e| fail(dst, e))?;
    let mut entries = fs::read_dir(src)
        .map_err(|e| fail(src, e))?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<Vec<OsString>, _>>()
        .map_err(|e| fail(src, e))?;
    entries.sort();
    for name in entries {
        if skip.iter().any(|s| name == *s) {
            continue;
        }
        let from = src.join(&name);
        let to = dst.join(&name);
        if fs::metadata(&from).map_err(|e| fail(&from, e))?.is_dir() {
            copy_tree(&from, &to, skip)?;
        } else {
            fs::copy(&from, &to).map_err(|e| fail(&from, e))?;
        }
    }
    Ok(())
}

/// Copy the built executables found in `from_dir` to `<skill>/target/release/`,
/// the place a `cargo build --release` inside the install would put them.
pub fn copy_binaries(from_dir: &Path, skill: &Path) -> Result<(), String> {
    let dest = skill.join("target").join("release");
    fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    for name in BINARIES {
        let from = from_dir.join(name);
        if !from.is_file() {
            return Err(format!("built executable not found: {}", from.display()));
        }
        fs::copy(&from, dest.join(name)).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(())
}

/// A new empty folder `<temp dir>/<prefix>XXXXXX`, made with `mkdtemp`.
pub fn make_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let mut template = std::env::temp_dir().join(prefix).into_os_string();
    template.push("XXXXXX");
    let c = CString::new(template.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = c.into_bytes_with_nul();
    // SAFETY: `raw` is a writable NUL-terminated buffer that mkdtemp fills in place.
    let made = unsafe { libc::mkdtemp(raw.as_mut_ptr().cast()) };
    if made.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    raw.pop();
    Ok(PathBuf::from(OsString::from_vec(raw)))
}
