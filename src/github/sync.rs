//! Mirror job handlers: push branches, open/close GitHub PRs, sync board status.

use serde_json::Value;

use super::gh;
use crate::AppState;
use crate::domain::models::{Issue, Project, PullRequest};
use crate::git;
use crate::services;

fn repo_of(project: &Project) -> anyhow::Result<&str> {
    project.github_repo.as_deref().ok_or_else(|| anyhow::anyhow!("no github_repo"))
}

pub async fn push_pr(
    app: &AppState,
    project: &Project,
    pr_id: i64,
    create_pr: bool,
    log: &mut (dyn FnMut(String) + Send),
) -> anyhow::Result<()> {
    let repo = repo_of(project)?;
    let pr: PullRequest = services::pull_by_id(&app.db, pr_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if pr.state != "open" {
        log("PR no longer open; skipping".into());
        return Ok(());
    }
    let spec = format!("refs/heads/{0}:refs/heads/{0}", pr.branch);
    git::run(&project.repo_path, &["push", "--force-with-lease", "origin", &spec]).await?;
    log(format!("pushed {}", pr.branch));
    if create_pr && pr.github_number.is_none() {
        let mut body = pr.body.clone();
        for n in services::pulls::linked_issue_numbers(&app.db, pr.id).await? {
            let gh_num: Option<i64> = sqlx::query_scalar("SELECT github_number FROM issues WHERE project_id = ? AND number = ?")
                .bind(project.id)
                .bind(n)
                .fetch_one(&app.db)
                .await?;
            if let Some(g) = gh_num {
                body.push_str(&format!("\n\nFixes #{g}"));
            }
        }
        let url = gh(
            &["pr", "create", "--repo", repo, "--head", &pr.branch, "--base", &pr.base_branch, "--title", &pr.title, "--body", &body],
            None,
        )
        .await?;
        let url = url.trim().to_string();
        let num = url.rsplit('/').next().and_then(|n| n.parse::<i64>().ok());
        sqlx::query("UPDATE pull_requests SET github_url = ?, github_number = ? WHERE id = ?")
            .bind(&url)
            .bind(num)
            .bind(pr.id)
            .execute(&app.db)
            .await?;
        log(format!("opened {url}"));
        app.bus.pr(&project.slug, pr.number);
    }
    Ok(())
}

pub async fn pr_comment(app: &AppState, project: &Project, pr_id: i64, body: &str) -> anyhow::Result<()> {
    let repo = repo_of(project)?;
    let pr = services::pull_by_id(&app.db, pr_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(n) = pr.github_number {
        gh(&["pr", "comment", &n.to_string(), "--repo", repo, "--body", body], None).await?;
    }
    Ok(())
}

pub async fn merged(app: &AppState, project: &Project, pr_id: i64, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    let repo = repo_of(project)?;
    let pr = services::pull_by_id(&app.db, pr_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let spec = format!("refs/heads/{0}:refs/heads/{0}", pr.base_branch);
    git::run(&project.repo_path, &["push", "origin", &spec]).await?;
    log(format!("pushed {}", pr.base_branch));
    let sha = pr.merged_sha.clone().unwrap_or_default();
    if let Some(n) = pr.github_number {
        let msg = format!("Merged locally into `{}` as {sha}.", pr.base_branch);
        let _ = gh(&["pr", "close", &n.to_string(), "--repo", repo, "--comment", &msg], None).await;
        let _ = gh(&["api", "-X", "DELETE", &format!("repos/{repo}/git/refs/heads/{}", pr.branch)], None).await;
        log(format!("closed GitHub PR #{n}"));
    }
    // Parked batch members stay open, on GitHub as on the board.
    let closing = services::pr_closing_issues(&app.db, pr.id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    for g in closing.into_iter().filter_map(|i| i.github_number) {
        let _ = gh(&["issue", "close", &g.to_string(), "--repo", repo, "--comment", &format!("Fixed in {sha}.")], None).await;
        log(format!("closed GitHub issue #{g}"));
    }
    Ok(())
}

pub async fn sync_status(app: &AppState, project: &Project, issue_id: i64, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    let repo = repo_of(project)?;
    let (Some(pid), Some(fid)) = (&project.github_project_id, &project.github_status_field_id) else {
        anyhow::bail!("project board ids unknown; run the GitHub import first");
    };
    let mut issue: Issue = services::issue_by_id(&app.db, issue_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if issue.github_node_id.as_deref().is_none_or(|n| n.starts_with("stub:")) {
        if !project.mirror_create_issues || issue.github_node_id.is_some() {
            return Ok(());
        }
        let url = gh(&["issue", "create", "--repo", repo, "--title", &issue.title, "--body", &issue.body], None).await?;
        let num: i64 = url.trim().rsplit('/').next().and_then(|n| n.parse().ok()).ok_or_else(|| anyhow::anyhow!("bad url {url}"))?;
        let node = gh(&["issue", "view", &num.to_string(), "--repo", repo, "--json", "id", "-q", ".id"], None).await?;
        sqlx::query("UPDATE issues SET github_node_id = ?, github_number = ? WHERE id = ?")
            .bind(node.trim())
            .bind(num)
            .bind(issue.id)
            .execute(&app.db)
            .await?;
        log(format!("created GitHub issue #{num}"));
        issue = services::issue_by_id(&app.db, issue_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let item_id = match &issue.github_project_item_id {
        Some(i) => i.clone(),
        None => {
            let q = "mutation($p:ID!,$c:ID!){addProjectV2ItemById(input:{projectId:$p,contentId:$c}){item{id}}}";
            let r = super::graphql(q, &[("p", pid.clone()), ("c", issue.github_node_id.clone().unwrap_or_default())], false).await?;
            let id = r[0]["data"]["addProjectV2ItemById"]["item"]["id"].as_str().unwrap_or_default().to_string();
            sqlx::query("UPDATE issues SET github_project_item_id = ? WHERE id = ?").bind(&id).bind(issue.id).execute(&app.db).await?;
            id
        }
    };
    let name = super::import::status_name(issue.state, issue.hold);
    let Some(opt) = project.github_status_options.0.get(name).and_then(Value::as_str) else {
        anyhow::bail!("board has no Status option named `{name}`");
    };
    let q = "mutation($p:ID!,$i:ID!,$f:ID!,$o:String!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{singleSelectOptionId:$o}}){projectV2Item{id}}}";
    super::graphql(q, &[("p", pid.clone()), ("i", item_id), ("f", fid.clone()), ("o", opt.to_string())], false).await?;
    log(format!("set #{} status to {name}", issue.github_number.unwrap_or(issue.number)));
    Ok(())
}
