//! Unified diff retrieval and per-file splitting.

use std::path::Path;

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileDiff {
    /// Path in the new tree (old path for deletions).
    pub path: String,
    /// Previous path for renames.
    pub old_path: Option<String>,
    /// `added`, `deleted`, `modified`, `renamed` or `binary`.
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// The file's section of the unified diff, including its `diff --git` header.
    pub patch: String,
}

pub async fn diff(repo: impl AsRef<Path>, from: &str, to: &str, context: u32) -> anyhow::Result<Vec<FileDiff>> {
    let u = format!("-U{context}");
    let out = super::run_raw(repo, &["diff", "--no-color", "--no-ext-diff", "-M", &u, from, to]).await?;
    if out.status != 0 {
        anyhow::bail!("git diff failed: {}", out.stderr.trim());
    }
    Ok(split(&out.stdout))
}

/// Split a multi-file unified diff into per-file sections.
pub fn split(diff: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut cur: Option<(Vec<&str>,)> = None;
    let flush = |lines: &[&str], files: &mut Vec<FileDiff>| {
        if lines.is_empty() {
            return;
        }
        files.push(parse_file(lines));
    };
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some((l,)) = cur.take() {
                flush(&l, &mut files);
            }
            cur = Some((vec![line],));
        } else if let Some((l,)) = cur.as_mut() {
            l.push(line);
        }
    }
    if let Some((l,)) = cur.take() {
        flush(&l, &mut files);
    }
    files
}

fn strip_prefix(p: &str) -> String {
    let p = p.trim_end_matches(['\n', '\r']);
    let p = p.trim_matches('"');
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string()
}

fn parse_file(lines: &[&str]) -> FileDiff {
    let mut old_path = None;
    let mut new_path = None;
    let mut status = "modified";
    let mut additions = 0;
    let mut deletions = 0;
    let mut in_hunk = false;
    for l in lines {
        if in_hunk {
            if l.starts_with('+') {
                additions += 1;
            } else if l.starts_with('-') {
                deletions += 1;
            } else if l.starts_with("@@") {
            }
            continue;
        }
        if let Some(p) = l.strip_prefix("--- ") {
            if p.trim() != "/dev/null" {
                old_path = Some(strip_prefix(p));
            }
        } else if let Some(p) = l.strip_prefix("+++ ") {
            if p.trim() != "/dev/null" {
                new_path = Some(strip_prefix(p));
            }
        } else if l.starts_with("new file mode") {
            status = "added";
        } else if l.starts_with("deleted file mode") {
            status = "deleted";
        } else if let Some(p) = l.strip_prefix("rename from ") {
            status = "renamed";
            old_path = Some(p.trim_end().to_string());
        } else if let Some(p) = l.strip_prefix("rename to ") {
            new_path = Some(p.trim_end().to_string());
        } else if l.starts_with("Binary files") {
            status = "binary";
        } else if l.starts_with("@@") {
            in_hunk = true;
        }
    }
    // Fall back to the `diff --git a/x b/y` header (e.g. mode-only or binary changes).
    if old_path.is_none()
        && new_path.is_none()
        && let Some(h) = lines.first().and_then(|h| h.strip_prefix("diff --git "))
        && let Some((a, b)) = h.trim_end().split_once(" b/")
    {
        old_path = Some(strip_prefix(a));
        new_path = Some(b.to_string());
    }
    let path = new_path.clone().or_else(|| old_path.clone()).unwrap_or_default();
    FileDiff {
        old_path: if status == "renamed" { old_path } else { None },
        path,
        status: status.to_string(),
        additions,
        deletions,
        patch: lines.concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_files() {
        let d = "diff --git a/x.txt b/x.txt\nindex 1..2 100644\n--- a/x.txt\n+++ b/x.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+c\ndiff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi\n";
        let f = split(d);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "x.txt");
        assert_eq!((f[0].additions, f[0].deletions), (1, 1));
        assert_eq!(f[1].status, "added");
        assert_eq!(f[1].path, "n.txt");
    }
}
