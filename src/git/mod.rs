//! Thin async wrappers around the `git` CLI.

pub mod diff;
pub mod linemap;
pub mod scanner;
pub mod worktree;

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, bail};
use tokio::process::Command;

pub struct GitOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

fn cmd(repo: impl AsRef<Path>) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(repo.as_ref())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    c
}

/// Run git and return output regardless of exit status.
pub async fn run_raw(repo: impl AsRef<Path>, args: &[&str]) -> anyhow::Result<GitOutput> {
    let out = cmd(repo).args(args).output().await.context("spawning git")?;
    Ok(GitOutput {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Run git, failing on non-zero exit. Returns trimmed stdout.
pub async fn run(repo: impl AsRef<Path>, args: &[&str]) -> anyhow::Result<String> {
    let o = run_raw(repo, args).await?;
    if o.status != 0 {
        bail!("git {} failed ({}): {}", args.join(" "), o.status, o.stderr.trim());
    }
    Ok(o.stdout.trim_end().to_string())
}

/// Run git with extra environment variables.
pub async fn run_env(repo: impl AsRef<Path>, args: &[&str], env: &[(&str, &str)]) -> anyhow::Result<String> {
    let mut c = cmd(repo);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().await?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub async fn rev_parse(repo: impl AsRef<Path>, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    run(repo, &["rev-parse", "--verify", "--quiet", &spec]).await.ok().filter(|s| !s.is_empty())
}

pub async fn branch_sha(repo: impl AsRef<Path>, branch: &str) -> Option<String> {
    rev_parse(repo, &format!("refs/heads/{branch}")).await
}

pub async fn commits_ahead(repo: impl AsRef<Path>, base: &str, head: &str) -> anyhow::Result<u64> {
    let range = format!("{base}..{head}");
    Ok(run(repo, &["rev-list", "--count", &range]).await?.trim().parse()?)
}

pub async fn merge_base(repo: impl AsRef<Path>, a: &str, b: &str) -> Option<String> {
    run(repo, &["merge-base", a, b]).await.ok()
}

pub async fn is_ancestor(repo: impl AsRef<Path>, a: &str, b: &str) -> bool {
    run_raw(repo, &["merge-base", "--is-ancestor", a, b]).await.map(|o| o.status == 0).unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct MergeTree {
    pub clean: bool,
    pub tree: String,
    pub conflicts: Vec<String>,
}

/// Three-way merge of `head` into `base` without touching any worktree.
pub async fn merge_tree(repo: impl AsRef<Path>, base: &str, head: &str) -> anyhow::Result<MergeTree> {
    let o = run_raw(repo, &["merge-tree", "--write-tree", "--name-only", "--no-messages", base, head]).await?;
    match o.status {
        0 | 1 => {
            let mut lines = o.stdout.lines();
            let tree = lines.next().unwrap_or_default().trim().to_string();
            let conflicts: Vec<String> =
                lines.take_while(|l| !l.is_empty()).map(str::to_string).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            Ok(MergeTree { clean: o.status == 0, tree, conflicts })
        }
        _ => bail!("git merge-tree failed: {}", o.stderr.trim()),
    }
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub date: String,
}

pub async fn log(repo: impl AsRef<Path>, range: &str, limit: usize) -> anyhow::Result<Vec<CommitInfo>> {
    let n = format!("-n{limit}");
    let out = run(repo, &["log", &n, "--format=%H%x1f%s%x1f%b%x1f%an%x1f%aI%x1e", range]).await?;
    Ok(out
        .split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.trim_start_matches('\n').split('\x1f').collect();
            (f.len() >= 5).then(|| CommitInfo {
                sha: f[0].to_string(),
                subject: f[1].to_string(),
                body: f[2].trim().to_string(),
                author: f[3].to_string(),
                date: f[4].trim().to_string(),
            })
        })
        .collect())
}

pub async fn is_dirty(worktree: impl AsRef<Path>) -> bool {
    run(worktree, &["status", "--porcelain", "--untracked-files=no"]).await.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Absolute path of the common `.git` directory (shared by all worktrees).
pub async fn common_dir(repo: impl AsRef<Path>) -> anyhow::Result<String> {
    let repo = repo.as_ref();
    let d = run(repo, &["rev-parse", "--git-common-dir"]).await?;
    let p = Path::new(&d);
    let abs = if p.is_absolute() { p.to_path_buf() } else { repo.join(p) };
    Ok(abs.canonicalize().unwrap_or(abs).to_string_lossy().into_owned())
}
