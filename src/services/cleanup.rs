//! Side effects of issues leaving active work: cancel runs, remove worktrees, delete merged branches.

use std::path::Path;
use std::time::Duration;

use crate::AppState;
use crate::db;
use crate::domain::IssueState;
use crate::domain::models::{Issue, Project, Worktree};
use crate::git;

pub async fn on_state_change(app: &AppState, project: &Project, issue: &Issue, from: IssueState, to: IssueState) {
    if from != to && matches!(to, IssueState::Backlog | IssueState::Done | IssueState::Closed) {
        cancel_active_runs(app, issue.id, &format!("issue moved to {}", to.as_str())).await;
        if let Err(e) = remove_issue_worktrees(app, project, issue.id, to == IssueState::Done).await {
            tracing::warn!("cleanup of issue #{} failed: {e:#}", issue.number);
        }
    }
    crate::github::mirror::on_issue_state(app, project, issue.id).await;
}

/// Cancel every active run on the issue and wait (briefly) for them to stop.
pub async fn cancel_active_runs(app: &AppState, issue_id: i64, reason: &str) {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM agent_runs WHERE issue_id = ? AND status IN ('queued','preparing','running')")
        .bind(issue_id)
        .fetch_all(&app.db)
        .await
        .unwrap_or_default();
    for id in ids {
        if !app.runs.cancel_and_wait(id, reason, Duration::from_secs(20)).await {
            // No live task (or it didn't stop): mark it ourselves so the issue is free.
            let _ = sqlx::query(
                "UPDATE agent_runs SET status = 'cancelled', error = COALESCE(error, ?), ended_at = ?
                  WHERE id = ? AND status IN ('queued','preparing','running')",
            )
            .bind(reason)
            .bind(db::now())
            .bind(id)
            .execute(&app.db)
            .await;
            app.bus.run(None, id);
        }
    }
}

pub async fn remove_issue_worktrees(app: &AppState, project: &Project, issue_id: i64, delete_merged_branch: bool) -> anyhow::Result<()> {
    let wts = sqlx::query_as::<_, Worktree>("SELECT * FROM worktrees WHERE issue_id = ? AND removed_at IS NULL")
        .bind(issue_id)
        .fetch_all(&app.db)
        .await?;
    for wt in wts {
        remove_worktree(app, project, &wt).await?;
        if delete_merged_branch && let Some(branch) = &wt.branch {
            let merged: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pull_requests WHERE project_id = ? AND branch = ? AND state = 'merged')")
                    .bind(project.id)
                    .bind(branch)
                    .fetch_one(&app.db)
                    .await?;
            let still_open: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pull_requests WHERE project_id = ? AND branch = ? AND state = 'open')")
                    .bind(project.id)
                    .bind(branch)
                    .fetch_one(&app.db)
                    .await?;
            if merged && !still_open && git::worktree::checked_out_at(&project.repo_path, branch).await?.is_none() {
                let _ = git::run(&project.repo_path, &["branch", "-D", branch]).await;
            }
        }
    }
    app.bus.emit("worktrees.updated", Some(&project.slug), None, None, None);
    Ok(())
}

pub async fn remove_worktree(app: &AppState, project: &Project, wt: &Worktree) -> anyhow::Result<()> {
    git::worktree::remove(&project.repo_path, Path::new(&wt.path)).await?;
    sqlx::query("UPDATE worktrees SET removed_at = ? WHERE id = ?").bind(db::now()).bind(wt.id).execute(&app.db).await?;
    Ok(())
}
