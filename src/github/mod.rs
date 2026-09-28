//! GitHub integration via the `gh` CLI (reuses the user's existing auth).

pub mod import;
pub mod mirror;
pub mod sync;

use std::process::Stdio;

use anyhow::{Context, bail};
use serde_json::Value;
use tokio::process::Command;

/// Run `gh` with args; returns stdout.
pub async fn gh(args: &[&str], cwd: Option<&str>) -> anyhow::Result<String> {
    let mut c = Command::new("gh");
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("GH_PROMPT_DISABLED", "1");
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().await.context("running gh (is the GitHub CLI installed?)")?;
    if !out.status.success() {
        bail!("gh {} failed: {}", args.first().copied().unwrap_or(""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// GraphQL query; `vars` are passed as typed `-F` fields. With `paginate`, returns one value per page.
pub async fn graphql(query: &str, vars: &[(&str, String)], paginate: bool) -> anyhow::Result<Vec<Value>> {
    let q = format!("query={query}");
    let mut args: Vec<String> = vec!["api".into(), "graphql".into(), "-f".into(), q];
    for (k, v) in vars {
        args.push("-F".into());
        args.push(format!("{k}={v}"));
    }
    if paginate {
        args.push("--paginate".into());
        args.push("--slurp".into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = gh(&refs, None).await?;
    let v: Value = serde_json::from_str(&out).context("parsing gh output")?;
    let pages = if paginate { v.as_array().cloned().unwrap_or_default() } else { vec![v] };
    for p in &pages {
        if let Some(errs) = p.get("errors") {
            bail!("GitHub GraphQL errors: {errs}");
        }
    }
    Ok(pages)
}

/// Split `owner/name`.
pub fn split_repo(repo: &str) -> anyhow::Result<(&str, &str)> {
    repo.split_once('/').ok_or_else(|| anyhow::anyhow!("github repo must be `owner/name`, got `{repo}`"))
}
