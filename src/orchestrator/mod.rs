//! Agent orchestration: scheduler, run lifecycle, usage limits, prompts.

pub mod limits;
pub mod permissions;
pub mod probe;
pub mod prompt;
pub mod registry;
pub mod run;
pub mod scheduler;

use crate::AppState;
use crate::db;
use crate::domain::models::Worktree;

/// Startup reconciliation after a restart or crash.
pub async fn reconcile(app: &AppState) -> anyhow::Result<()> {
    let n = sqlx::query(
        "UPDATE agent_runs SET status = 'interrupted', error = 'server restarted while the run was active', ended_at = ?
          WHERE status IN ('queued','preparing','running')",
    )
    .bind(db::now())
    .execute(&app.db)
    .await?
    .rows_affected();
    if n > 0 {
        tracing::info!("marked {n} runs interrupted");
    }
    sqlx::query("UPDATE permission_requests SET status = 'expired' WHERE status = 'pending'").execute(&app.db).await?;
    crate::container::sweep_orphans().await;

    // Remove ephemeral (detached) worktrees and any whose issue is no longer active.
    let wts = sqlx::query_as::<_, Worktree>(
        "SELECT w.* FROM worktrees w LEFT JOIN issues i ON i.id = w.issue_id
          WHERE w.removed_at IS NULL AND (w.kind = 'detached' OR i.id IS NULL OR i.state IN ('backlog','done','closed','triage'))",
    )
    .fetch_all(&app.db)
    .await?;
    for w in wts {
        if let Ok(p) = crate::services::project_by_id(&app.db, w.project_id).await {
            let _ = crate::services::cleanup::remove_worktree(app, &p, &w).await;
        }
    }
    // Rows whose directory vanished.
    let live = sqlx::query_as::<_, Worktree>("SELECT * FROM worktrees WHERE removed_at IS NULL").fetch_all(&app.db).await?;
    for w in live {
        if !std::path::Path::new(&w.path).exists() {
            sqlx::query("UPDATE worktrees SET removed_at = ? WHERE id = ?").bind(db::now()).bind(w.id).execute(&app.db).await?;
        }
    }
    let projects: Vec<String> = sqlx::query_scalar("SELECT repo_path FROM projects").fetch_all(&app.db).await?;
    for p in projects {
        let _ = crate::git::run(&p, &["worktree", "prune"]).await;
    }
    Ok(())
}

pub fn start(app: &AppState) {
    scheduler::spawn(app.clone());
    limits::spawn(app.clone());
}
