//! Embeds the commit and time this binary was built from, so the running server can tell how far
//! it is behind the checkout's current HEAD (see `src/deploy.rs`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let sha = git(&dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let time = git(&dir, &["log", "-1", "--format=%cI", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=AKB_BUILD_SHA={sha}");
    println!("cargo:rustc-env=AKB_BUILD_TIME={time}");
    // A fingerprint of `web/dist`'s contents, embedded but otherwise unused: Cargo only recompiles
    // this crate when a build script's *output* changes, not merely when it re-runs, and Vite
    // renames every output file (content hash in the filename) rather than editing one in place,
    // so watching the directory alone wouldn't be enough to invalidate a stale embed.
    println!("cargo:rustc-env=AKB_WEB_DIST_FINGERPRINT={}", dist_fingerprint(&Path::new(&dir).join("web/dist")));

    // Watch every file whose change should re-embed the SHA/time. `--git-path HEAD` is already
    // worktree-aware (each linked worktree has its own HEAD file), but a fast-forward merge of the
    // checked-out branch leaves that file untouched — only the branch's ref file (or, once git gc
    // packs it, `packed-refs`) changes, and those live in the *common* git dir, shared by all
    // worktrees.
    if let Some(head_path) = git(&dir, &["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={}", resolve(&dir, &head_path).display());
    }
    if let Some(common) = git(&dir, &["rev-parse", "--git-common-dir"]) {
        let common = resolve(&dir, &common);
        println!("cargo:rerun-if-changed={}", common.join("packed-refs").display());
        if let Some(head_ref) = git(&dir, &["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", common.join(head_ref).display());
        }
    }
    // Emitting any `rerun-if-changed` disables Cargo's default fallback of re-running this script
    // (and so re-embedding the above) whenever any file in the package changes; explicitly list
    // the ones we still care about.
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=build.rs");
}

fn resolve(dir: &str, git_relative: &str) -> PathBuf {
    let p = Path::new(git_relative);
    if p.is_absolute() { p.to_path_buf() } else { Path::new(dir).join(p) }
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    // Some sandboxes run build scripts as a different (effective) user than the one that owns
    // the checkout, which trips git's "dubious ownership" guard; this build-time metadata is
    // informational only, so trust whatever directory the crate is being built from.
    let out = Command::new("git").args(["-c", "safe.directory=*", "-C", dir]).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Hashes the relative path and modification time of every file under `dist`, so it changes
/// whenever a rebuild produces different (or differently-timestamped) web assets — including a
/// rebuild of an uncommitted ("dirty") checkout, where the git SHA alone wouldn't change.
fn dist_fingerprint(dist: &Path) -> String {
    let mut entries = Vec::new();
    collect_files(dist, dist, &mut entries);
    entries.sort();
    let mut hasher = DefaultHasher::new();
    entries.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
            let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            out.push(format!("{rel}:{}.{}", since_epoch.as_secs(), since_epoch.subsec_nanos()));
        }
    }
}
