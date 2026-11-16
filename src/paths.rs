//! Where the skill and the project are, and how paths and commands are shown.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

pub const HARNESS_DIRS: [&str; 3] = [".claude", ".agents", ".codex"];

/// Lexical path joining like the baseline's path objects: empty and `.` parts drop out,
/// `..` stays, an absolute right side replaces the left.
pub fn join(base: &Path, rel: &str) -> PathBuf {
    let rel_path = Path::new(rel);
    let mut out = if rel_path.is_absolute() {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for part in rel_path.components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::ParentDir => out.push(".."),
            _ => {}
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// A path given on the command line, normalised lexically (`a//b/./` → `a/b`, `` → `.`).
pub fn normalise(text: &str) -> PathBuf {
    if text.is_empty() {
        return PathBuf::from(".");
    }
    let joined = join(Path::new(""), text);
    if joined.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        joined
    }
}

fn is_loop(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(libc::ELOOP)
}

/// Non-strict resolve: an absolute path with symbolic links of the existing part
/// followed and `..` folded. A symbolic link loop is an error.
pub fn resolve(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let parts: Vec<Component> = absolute.components().collect();
    for keep in (1..=parts.len()).rev() {
        let prefix: PathBuf = parts[..keep].iter().collect();
        match std::fs::canonicalize(&prefix) {
            Ok(mut found) => {
                for part in &parts[keep..] {
                    match part {
                        Component::ParentDir => {
                            found.pop();
                        }
                        Component::Normal(p) => found.push(p),
                        _ => {}
                    }
                }
                if let Err(err) = std::fs::metadata(&found) {
                    if is_loop(&err) {
                        return Err(format!("Symlink loop from '{}'", found.display()));
                    }
                }
                return Ok(found);
            }
            Err(err) if is_loop(&err) => {
                return Err(format!("Symlink loop from '{}'", prefix.display()));
            }
            Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(absolute)
}

/// The fixed places of one run.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The running executable (symbolic links resolved).
    pub exe: PathBuf,
    /// `<skill>/target/<profile>/<bin>` → `<skill>`.
    pub skill_dir: PathBuf,
    /// The project the skill is installed in.
    pub repo: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Paths, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let skill_dir = exe
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let repo = project_root(&skill_dir)?;
        Ok(Paths {
            exe,
            skill_dir,
            repo,
        })
    }

    /// A path relative to the project when it lies inside, else as is.
    pub fn shown(&self, path: &Path) -> String {
        match path.strip_prefix(&self.repo) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }

    /// The command that runs this executable, as printed in guidance.
    pub fn pm_command(&self) -> String {
        self.shown(&self.exe)
    }

    /// A sibling executable of this one (same build folder), as printed in guidance.
    pub fn tool_command(&self, name: &str) -> String {
        let dir = self.exe.parent().unwrap_or(Path::new("/"));
        self.shown(&dir.join(name))
    }
}

/// `PM_PROJECT_ROOT`; else `<root>` of `<root>/skills/<name>`, `<root>/.claude/skills/<name>`
/// or `<root>/.agents/skills/<name>`; else the current directory.
pub fn project_root(skill_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(env) = std::env::var("PM_PROJECT_ROOT") {
        if !env.is_empty() {
            return resolve(&normalise(&env));
        }
    }
    if let Some(parent) = skill_dir.parent() {
        if parent.file_name().is_some_and(|n| n == "skills") {
            if let Some(root) = parent.parent() {
                let harness = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| HARNESS_DIRS.contains(&n));
                return Ok(if harness {
                    root.parent().unwrap_or(root).to_path_buf()
                } else {
                    root.to_path_buf()
                });
            }
        }
    }
    resolve(Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joining_is_lexical() {
        assert_eq!(join(Path::new("/a"), "."), PathBuf::from("/a"));
        assert_eq!(join(Path::new("/a"), "b//c/./"), PathBuf::from("/a/b/c"));
        assert_eq!(join(Path::new("/a"), "/x"), PathBuf::from("/x"));
        assert_eq!(join(Path::new("/a"), "../t"), PathBuf::from("/a/../t"));
        assert_eq!(normalise(""), PathBuf::from("."));
    }
}
