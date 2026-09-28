//! Periodic scan of open PR branches: detects new commits and merge conflicts with the base.

use std::time::Duration;

use crate::AppState;
use crate::db;
use crate::domain::models::{Project, PullRequest};
use crate::domain::{Actor, IssueState};
use crate::services;

pub fn spawn(app: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = scan_all(&app).await {
                tracing::warn!("ref scan failed: {e:#}");
            }
            tokio::select! {
                _ = app.bus.scan.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            }
        }
    });
}

pub async fn scan_all(app: &AppState) -> anyhow::Result<()> {
    let projects = sqlx::query_as::<_, Project>("SELECT * FROM projects").fetch_all(&app.db).await?;
    for p in projects {
        if !std::path::Path::new(&p.repo_path).exists() {
            continue;
        }
        let prs = sqlx::query_as::<_, PullRequest>("SELECT * FROM pull_requests WHERE project_id = ? AND state = 'open'")
            .bind(p.id)
            .fetch_all(&app.db)
            .await?;
        for pr in prs {
            if let Err(e) = scan_pr(app, &p, &pr).await {
                tracing::debug!("scan of PR #{} failed: {e:#}", pr.number);
            }
        }
    }
    Ok(())
}

pub async fn scan_pr(app: &AppState, project: &Project, pr: &PullRequest) -> anyhow::Result<()> {
    let pr = services::pulls::refresh_head(app, project, pr).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(head) = pr.head_sha.clone() else { return Ok(()) };
    let Some(base) = super::branch_sha(&project.repo_path, &pr.base_branch).await else { return Ok(()) };
    let checked_key = format!("{base}:{head}");
    if pr.conflicts_base_sha.as_deref() == Some(checked_key.as_str()) {
        return Ok(());
    }
    let mt = super::merge_tree(&project.repo_path, &base, &head).await?;
    sqlx::query(
        "UPDATE pull_requests SET has_conflicts = ?, conflict_files = ?, conflicts_checked_at = ?, conflicts_base_sha = ? WHERE id = ?",
    )
    .bind(!mt.clean)
    .bind(serde_json::to_string(&mt.conflicts)?)
    .bind(db::now())
    .bind(&checked_key)
    .bind(pr.id)
    .execute(&app.db)
    .await?;
    if pr.has_conflicts != !mt.clean {
        app.bus.pr(&project.slug, pr.number);
    }

    for iid in services::pr_issue_ids(&app.db, pr.id).await? {
        let issue = services::issue_by_id(&app.db, iid).await?;
        if !mt.clean && matches!(issue.state, IssueState::InReview | IssueState::ReadyToMerge) {
            let msg = format!(
                "⚠️ PR #{} no longer merges cleanly into `{}` (conflicts in {}). Sending to merge prep.",
                pr.number,
                pr.base_branch,
                mt.conflicts.join(", ")
            );
            services::issues::set_state(app, project, &issue, IssueState::MergeConflict, &Actor::System, Some(&msg), None).await.ok();
        } else if mt.clean && issue.state == IssueState::MergeConflict {
            services::issues::set_state(
                app,
                project,
                &issue,
                IssueState::InReview,
                &Actor::System,
                Some(&format!("PR #{} merges cleanly again; back to review.", pr.number)),
                None,
            )
            .await
            .ok();
        }
    }
    Ok(())
}
