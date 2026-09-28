//! Worktree management.

use std::path::{Path, PathBuf};

use anyhow::bail;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
}

pub async fn list(repo: impl AsRef<Path>) -> anyhow::Result<Vec<WorktreeInfo>> {
    let out = super::run(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut res = Vec::new();
    let mut cur: Option<WorktreeInfo> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(c) = cur.take() {
                res.push(c);
            }
            cur = Some(WorktreeInfo { path: PathBuf::from(p), head: None, branch: None });
        } else if let Some(c) = cur.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                c.head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                c.branch = Some(b.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    if let Some(c) = cur.take() {
        res.push(c);
    }
    Ok(res)
}

/// Worktree path where `branch` is checked out, if any.
pub async fn checked_out_at(repo: impl AsRef<Path>, branch: &str) -> anyhow::Result<Option<PathBuf>> {
    Ok(list(repo).await?.into_iter().find(|w| w.branch.as_deref() == Some(branch)).map(|w| w.path))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/// Ensure a worktree at `path` with `branch` checked out, creating the branch from `start` if needed.
pub async fn ensure_branch(repo: impl AsRef<Path>, path: &Path, branch: &str, start: &str) -> anyhow::Result<bool> {
    let repo = repo.as_ref();
    if let Some(at) = checked_out_at(repo, branch).await? {
        if same_path(&at, path) && path.exists() {
            return Ok(false);
        }
        bail!(
            "branch `{branch}` is already checked out at {}; switch that checkout to another branch (or remove the worktree) so an agent can use it",
            at.display()
        );
    }
    if path.exists() {
        // Stale directory from an interrupted run; prune and recreate.
        let _ = super::run(repo, &["worktree", "remove", "--force", &path.to_string_lossy()]).await;
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.ok();
        }
        super::run(repo, &["worktree", "prune"]).await.ok();
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let p = path.to_string_lossy();
    if super::branch_sha(repo, branch).await.is_some() {
        super::run(repo, &["worktree", "add", &p, branch]).await?;
    } else {
        super::run(repo, &["worktree", "add", "-b", branch, &p, start]).await?;
    }
    Ok(true)
}

/// Create a detached worktree at `rev` (replacing any existing one at `path`).
pub async fn add_detached(repo: impl AsRef<Path>, path: &Path, rev: &str) -> anyhow::Result<()> {
    let repo = repo.as_ref();
    if path.exists() {
        remove(repo, path).await?;
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    super::run(repo, &["worktree", "add", "--detach", &path.to_string_lossy(), rev]).await?;
    Ok(())
}

pub async fn remove(repo: impl AsRef<Path>, path: &Path) -> anyhow::Result<()> {
    let repo = repo.as_ref();
    let p = path.to_string_lossy();
    let o = super::run_raw(repo, &["worktree", "remove", "--force", "--force", &p]).await?;
    if o.status != 0 && path.exists() {
        tokio::fs::remove_dir_all(path).await.ok();
    }
    super::run(repo, &["worktree", "prune"]).await.ok();
    Ok(())
}
