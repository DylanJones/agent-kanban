//! Background jobs (GitHub import/mirror, container builds) with retries and a log.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::AppState;
use crate::db;
use crate::domain::models::Job;
use crate::services;

static WAKE: std::sync::OnceLock<Notify> = std::sync::OnceLock::new();
fn wake() -> &'static Notify {
    WAKE.get_or_init(Notify::new)
}

const MAX_ATTEMPTS: i64 = 3;

pub async fn enqueue(app: &AppState, kind: &str, project_id: Option<i64>, payload: Value) -> i64 {
    // Coalesce identical queued jobs (e.g. repeated status syncs for one issue).
    if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>("SELECT id FROM jobs WHERE kind = ? AND payload = ? AND status = 'queued'")
        .bind(kind)
        .bind(payload.to_string())
        .fetch_optional(&app.db)
        .await
    {
        return id;
    }
    let id =
        sqlx::query_scalar("INSERT INTO jobs(kind, project_id, status, payload, created_at) VALUES (?, ?, 'queued', ?, ?) RETURNING id")
            .bind(kind)
            .bind(project_id)
            .bind(payload.to_string())
            .bind(db::now())
            .fetch_one(&app.db)
            .await
            .unwrap_or(0);
    wake().notify_one();
    app.bus.emit("jobs.updated", None, None, None, None);
    id
}

pub fn spawn_worker(app: AppState) {
    tokio::spawn(async move {
        // Jobs interrupted by a restart go back to the queue.
        let _ = sqlx::query("UPDATE jobs SET status = 'queued' WHERE status = 'running'").execute(&app.db).await;
        loop {
            match next_job(&app).await {
                Some(job) => run_job(&app, job).await,
                None => {
                    tokio::select! {
                        _ = wake().notified() => {}
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }
    });
}

async fn next_job(app: &AppState) -> Option<Job> {
    sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1
          WHERE id = (SELECT id FROM jobs WHERE status = 'queued' AND (finished_at IS NULL OR finished_at <= ?) ORDER BY id LIMIT 1)
          RETURNING *",
    )
    .bind(db::now())
    .fetch_optional(&app.db)
    .await
    .ok()
    .flatten()
}

async fn append_log(app: &AppState, id: i64, line: &str) {
    let _ = sqlx::query("UPDATE jobs SET log = log || ? WHERE id = ?").bind(format!("{line}\n")).bind(id).execute(&app.db).await;
}

async fn run_job(app: &AppState, job: Job) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut log = move |s: String| {
        tracing::info!("job: {s}");
        let _ = tx.send(s);
    };
    let app2 = app.clone();
    let id = job.id;
    let drain = tokio::spawn(async move {
        while let Some(l) = rx.recv().await {
            append_log(&app2, id, &l).await;
        }
    });
    let result = dispatch(app, &job, &mut log).await;
    drop(log);
    let _ = drain.await;
    match result {
        Ok(()) => {
            let _ = sqlx::query("UPDATE jobs SET status = 'succeeded', error = NULL, finished_at = ? WHERE id = ?")
                .bind(db::now())
                .bind(job.id)
                .execute(&app.db)
                .await;
        }
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::warn!("job {} ({}) failed: {msg}", job.id, job.kind);
            append_log(app, job.id, &format!("error: {msg}")).await;
            let retry = job.attempts < MAX_ATTEMPTS && job.kind != "github.import" && job.kind != "container.build";
            let status = if retry { "queued" } else { "failed" };
            let backoff = chrono::Utc::now() + chrono::Duration::seconds(30 * job.attempts);
            let _ = sqlx::query("UPDATE jobs SET status = ?, error = ?, finished_at = ? WHERE id = ?")
                .bind(status)
                .bind(&msg)
                .bind(db::fmt_time(backoff))
                .bind(job.id)
                .execute(&app.db)
                .await;
        }
    }
    app.bus.emit("jobs.updated", None, None, None, None);
}

async fn dispatch(app: &AppState, job: &Job, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    let project = match job.project_id {
        Some(p) => Some(services::project_by_id(&app.db, p).await.map_err(|e| anyhow::anyhow!("{e}"))?),
        None => None,
    };
    let p = || project.as_ref().ok_or_else(|| anyhow::anyhow!("job has no project"));
    let payload = &job.payload.0;
    let id_of = |k: &str| payload.get(k).and_then(Value::as_i64).ok_or_else(|| anyhow::anyhow!("missing {k}"));
    match job.kind.as_str() {
        "github.import" => {
            let stats = crate::github::import::run(app, p()?, log).await?;
            log(json!(stats).to_string());
            Ok(())
        }
        "github.push_pr" => {
            let create = payload.get("create_pr").and_then(Value::as_bool).unwrap_or(false);
            crate::github::sync::push_pr(app, p()?, id_of("pr_id")?, create, log).await
        }
        "github.pr_comment" => {
            crate::github::sync::pr_comment(app, p()?, id_of("pr_id")?, payload.get("body").and_then(Value::as_str).unwrap_or("")).await
        }
        "github.merged" => crate::github::sync::merged(app, p()?, id_of("pr_id")?, log).await,
        "github.sync_status" => crate::github::sync::sync_status(app, p()?, id_of("issue_id")?, log).await,
        "container.build" => crate::container::build_image(app, p()?, log).await.map(|_| ()),
        k => anyhow::bail!("unknown job kind {k}"),
    }
}
