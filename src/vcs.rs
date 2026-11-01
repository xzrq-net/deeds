//! Git and checkout plumbing: autocommit for a tracker that is its own repo,
//! and where a `deeds note` came from.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A tracker that is the root of its own git repo commits itself; an inline
/// `.deeds/` leaves commits to whoever works in the project.
pub fn autocommits(deeds: &Path) -> bool {
    deeds.join(".git").exists()
}

/// Holds the tracker repo's lock from the snapshot through the command's own
/// commit, so concurrent calls don't race git or interleave commits.
pub struct Autocommit {
    dir: PathBuf,
    _lock: File,
}

impl Autocommit {
    /// Lock, then commit whatever is pending (direct edits since the last
    /// call). None for trackers that don't autocommit.
    pub fn begin(deeds: &Path) -> Result<Option<Self>> {
        let Some(this) = Self::lock(deeds)? else {
            return Ok(None);
        };
        this.snapshot()?;
        Ok(Some(this))
    }

    fn lock(deeds: &Path) -> Result<Option<Self>> {
        if !autocommits(deeds) {
            return Ok(None);
        }
        // flock on `.git` itself, file or directory: no lock file to clean
        // up, and released on exit however the process ends.
        let lock = File::open(deeds.join(".git")).context("open .git for locking")?;
        lock.lock().context("lock tracker repo")?;
        Ok(Some(Self {
            dir: deeds.to_path_buf(),
            _lock: lock,
        }))
    }

    fn snapshot(&self) -> Result<Option<String>> {
        let changed = stage(&self.dir)?;
        if changed.is_empty() {
            return Ok(None);
        }
        let message = snapshot_message(&changed);
        git(&self.dir, &["commit", "-q", "-m", &message])?;
        Ok(Some(message))
    }

    /// Commit the command's change, if it made one.
    pub fn finish(self, message: &str) -> Result<()> {
        if !stage(&self.dir)?.is_empty() {
            git(&self.dir, &["commit", "-q", "-m", message])?;
        }
        Ok(())
    }
}

/// `deeds commit`: just the snapshot. The commit message, if one was made.
pub fn commit_pending(deeds: &Path) -> Result<Option<String>> {
    match Autocommit::lock(deeds)? {
        Some(session) => session.snapshot(),
        None => Ok(None),
    }
}

/// Stage everything; the paths that differ from HEAD.
fn stage(dir: &Path) -> Result<Vec<String>> {
    git(dir, &["add", "-A"])?;
    let out = git(
        dir,
        &["diff", "--cached", "--name-only", "--no-renames", "-z"],
    )?;
    Ok(String::from_utf8_lossy(&out)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect())
}

/// "snapshot: <ids>", an issue's id standing for its file, open or closed.
fn snapshot_message(paths: &[String]) -> String {
    let mut names: Vec<&str> = paths
        .iter()
        .map(|p| {
            let name = p.strip_prefix("closed/").unwrap_or(p);
            name.strip_suffix(".md")
                .filter(|id| !id.contains('/'))
                .unwrap_or(p)
        })
        .collect();
    names.sort();
    names.dedup();
    const SHOWN: usize = 8;
    let mut message = format!("snapshot: {}", names[..names.len().min(SHOWN)].join(", "));
    if names.len() > SHOWN {
        message += &format!(" and {} more", names.len() - SHOWN);
    }
    message
}

/// `git rev-parse --local-env-vars`: what makes git act on another repo.
const LOCAL_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    // Inside a git hook these would point at some other repo.
    for var in LOCAL_ENV {
        cmd.env_remove(var);
    }
    let out = cmd.output().context("run git")?;
    if !out.status.success() {
        bail!(
            "git {} in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// `git init` the tracker unless it is already inside a git repo.
pub fn init_repo(dir: &Path) -> Result<()> {
    match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(top) => {
            let top = PathBuf::from(String::from_utf8_lossy(&top).trim());
            if fs::canonicalize(&top).ok() != fs::canonicalize(dir).ok() {
                println!(
                    "inside the git repo at {}: deeds won't autocommit here",
                    top.display()
                );
            }
        }
        Err(_) => {
            git(dir, &["init", "-q"])?;
            println!("initialized git repo in {}", dir.display());
        }
    }
    Ok(())
}

/// Where a note comes from: `repo@workspace` in a jj workspace or git
/// worktree, `repo` in a main checkout, None outside any repo. A jj
/// workspace's name is taken to be its directory's, as `jj workspace add`
/// defaults it.
pub fn origin(cwd: &Path) -> Option<String> {
    let root = cwd
        .ancestors()
        .find(|d| d.join(".jj").is_dir() || d.join(".git").exists())?;
    let name = |p: &Path| p.file_name()?.to_str().map(String::from);
    let jj_repo = root.join(".jj/repo");
    if jj_repo.is_file() {
        // A secondary workspace: `.jj/repo` names the main one's, relative
        // to `.jj`.
        let target = fs::read_to_string(&jj_repo).ok()?;
        let target = fs::canonicalize(root.join(".jj").join(target.trim())).ok()?;
        let main = target.parent()?.parent()?;
        return Some(format!("{}@{}", name(main)?, name(root)?));
    }
    let dot_git = root.join(".git");
    if !root.join(".jj").is_dir() && dot_git.is_file() {
        // A worktree's `.git` points at `<main>/.git/worktrees/<name>`;
        // anything else (a submodule) counts as a main checkout.
        let text = fs::read_to_string(&dot_git).ok()?;
        let gitdir = text.strip_prefix("gitdir:")?.trim();
        let gitdir = fs::canonicalize(root.join(gitdir)).ok()?;
        let parent = gitdir.parent()?;
        if parent.file_name()? == "worktrees" {
            let main = parent.parent()?.parent()?;
            return Some(format!("{}@{}", name(main)?, name(&gitdir)?));
        }
    }
    name(root)
}

/// Today's local date, YYYY-MM-DD.
pub fn today() -> Result<String> {
    let out = Command::new("date")
        .arg("+%F")
        .output()
        .context("run date")?;
    if !out.status.success() {
        bail!("date: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_ids() {
        let paths = [
            "ab2cde.md",
            "closed/ab2cde.md",
            "closed/x9f2mk.md",
            "notes/a.md",
        ];
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            snapshot_message(&paths),
            "snapshot: ab2cde, notes/a.md, x9f2mk"
        );
        let many: Vec<String> = (0..10).map(|k| format!("id{k:04}.md")).collect();
        assert!(snapshot_message(&many).ends_with("id0007 and 2 more"));
    }
}
