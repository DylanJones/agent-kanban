//! Re-anchoring review-thread lines when a file changes between commits.

use std::path::Path;

/// A hunk header `@@ -old_start,old_len +new_start,new_len @@`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hunk {
    pub old_start: i64,
    pub old_len: i64,
    pub new_start: i64,
    pub new_len: i64,
}

pub fn parse_hunks(diff: &str) -> Vec<Hunk> {
    let re = regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    diff.lines()
        .filter_map(|l| {
            let c = re.captures(l)?;
            let n = |i: usize, d: i64| c.get(i).map(|m| m.as_str().parse().unwrap_or(d)).unwrap_or(d);
            Some(Hunk { old_start: n(1, 0), old_len: n(2, 1), new_start: n(3, 0), new_len: n(4, 1) })
        })
        .collect()
}

/// Map a line number in the old version of a file to the new version.
/// Returns `None` if the line itself was modified or deleted (the thread is outdated).
pub fn map_line(hunks: &[Hunk], line: i64) -> Option<i64> {
    let mut offset = 0i64;
    for h in hunks {
        if h.old_len == 0 {
            // Pure insertion after line `old_start`.
            if line <= h.old_start {
                return Some(line + offset);
            }
        } else {
            if line < h.old_start {
                return Some(line + offset);
            }
            if line < h.old_start + h.old_len {
                return None;
            }
        }
        offset += h.new_len - h.old_len;
    }
    Some(line + offset)
}

pub enum Remap {
    Moved(i64),
    Outdated,
    /// File no longer exists.
    Gone,
}

/// Where does `line` of `path` at `old` end up at `new`?
pub async fn remap(repo: impl AsRef<Path>, old: &str, new: &str, path: &str, line: i64) -> anyhow::Result<Remap> {
    let repo = repo.as_ref();
    let spec = format!("{new}:{path}");
    if super::run_raw(repo, &["cat-file", "-e", &spec]).await?.status != 0 {
        return Ok(Remap::Gone);
    }
    let out = super::run_raw(repo, &["diff", "--no-color", "--no-ext-diff", "-U0", old, new, "--", path]).await?;
    let hunks = parse_hunks(&out.stdout);
    Ok(match map_line(&hunks, line) {
        Some(l) => Remap::Moved(l),
        None => Remap::Outdated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lines() {
        // Line 3 replaced by two lines; one line inserted after 10.
        let h = parse_hunks("@@ -3 +3,2 @@\n-x\n+y\n+z\n@@ -10,0 +12 @@\n+w\n");
        assert_eq!(h.len(), 2);
        assert_eq!(map_line(&h, 1), Some(1));
        assert_eq!(map_line(&h, 3), None);
        assert_eq!(map_line(&h, 4), Some(5));
        assert_eq!(map_line(&h, 10), Some(11));
        assert_eq!(map_line(&h, 11), Some(13));
    }

    #[test]
    fn deletion() {
        let h = parse_hunks("@@ -5,2 +4,0 @@\n-a\n-b\n");
        assert_eq!(map_line(&h, 4), Some(4));
        assert_eq!(map_line(&h, 5), None);
        assert_eq!(map_line(&h, 7), Some(5));
    }
}
