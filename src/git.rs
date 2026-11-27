//! Git awareness: flag projects with uncommitted work and git worktrees whose
//! branch is already merged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct GitInfo {
    /// Uncommitted changes (tracked or untracked) in the project.
    pub dirty: bool,
    /// The project is a linked `git worktree`.
    pub worktree: bool,
    /// Linked worktree whose HEAD is already contained in the default branch.
    pub merged: bool,
    pub branch: Option<String>,
}

impl GitInfo {
    pub fn badge(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.dirty {
            parts.push("⚠ uncommitted changes".to_string());
        }
        if self.worktree && self.merged {
            let b = self.branch.as_deref().unwrap_or("worktree");
            parts.push(format!("✓ worktree '{b}' already merged"));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn default_branch(root: &Path) -> Option<String> {
    if let Some(b) = git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Some(b);
    }
    ["main", "master", "develop"]
        .into_iter()
        .find(|b| git_ok(root, &["rev-parse", "--verify", "--quiet", b]))
        .map(String::from)
}

fn inspect_root(root: &Path) -> GitInfo {
    let dirty = git(root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let git_dir = git(root, &["rev-parse", "--absolute-git-dir"]);
    let common = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let worktree = matches!((&git_dir, &common), (Some(a), Some(b)) if a != b);
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    let merged = worktree
        && default_branch(root).is_some_and(|def| {
            branch.as_deref() != Some(def.trim_start_matches("origin/"))
                && git_ok(root, &["merge-base", "--is-ancestor", "HEAD", &def])
        });
    GitInfo {
        dirty,
        worktree,
        merged,
        branch,
    }
}

/// Caches results per repository root (many artifacts can share one repo).
#[derive(Default)]
pub struct GitInspector {
    cache: Mutex<HashMap<PathBuf, Option<GitInfo>>>,
}

impl GitInspector {
    pub fn inspect(&self, project: &Path) -> Option<GitInfo> {
        let root = PathBuf::from(git(project, &["rev-parse", "--show-toplevel"])?);
        if let Some(hit) = self.cache.lock().unwrap().get(&root) {
            return hit.clone();
        }
        let info = Some(inspect_root(&root));
        self.cache.lock().unwrap().insert(root, info.clone());
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn detects_dirty_and_merged_worktree() {
        if !crate::util::has_command("git") {
            return;
        }
        let base = std::env::temp_dir().join(format!("diskdiet-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["config", "user.email", "t@example.com"]);
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("a.txt"), "a").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);

        let clean = GitInspector::default().inspect(&repo).unwrap();
        assert!(!clean.dirty && !clean.worktree);

        let wt = base.join("wt");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                wt.to_str().unwrap(),
            ],
        );
        let info = GitInspector::default().inspect(&wt).unwrap();
        assert!(info.worktree, "{info:?}");
        assert!(info.merged, "{info:?}");

        fs::write(repo.join("b.txt"), "b").unwrap();
        assert!(GitInspector::default().inspect(&repo).unwrap().dirty);
        fs::remove_dir_all(&base).unwrap();
    }
}
