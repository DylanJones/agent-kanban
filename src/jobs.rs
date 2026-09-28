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

/// Resets jobs interrupted by a restart, and restores deployment guards for any `server.build`
/// among them, before starting the worker loop. Called (and awaited) from `AppState::start_background`
/// before the scheduler starts or the server accepts requests, so a fresh rebuild request or an
/// automatic scheduler tick can never be admitted in the gap while this is still running — see
/// `recover_deploy_state` and thread #118 on issue #12, where recovery running in a fire-and-forget
/// spawned task let exactly that race skip a recovered drain's dispatch pause.
pub async fn spawn_worker(app: AppState) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = 'queued' WHERE status = 'running'").execute(&app.db).await?;
    recover_deploy_state(&app).await?;
    tokio::spawn(async move {
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
    Ok(())
}

/// Restores the deployment guards (`DeployState::deploying`, and `dispatch_paused` for a `drain`)
/// for any `server.build` job the reset above just put back on the queue. `DeployState` starts
/// fresh on every process start (it's in-memory, not persisted), but a build interrupted by the
/// very restart it was orchestrating comes back as an ordinary queued job and will run again
/// through `deploy::run_build_job` — without this, it would do so without the admission/drain
/// protection a fresh `rebuild` request normally gets, allowing an overlapping rebuild or a run
/// dispatched mid-drain. Must complete (its caller must await it) before anything else can admit a
/// new rebuild or dispatch a run; see `spawn_worker`.
async fn recover_deploy_state(app: &AppState) -> anyhow::Result<()> {
    let jobs: Vec<Job> = sqlx::query_as("SELECT * FROM jobs WHERE kind = 'server.build' AND status = 'queued'").fetch_all(&app.db).await?;
    for job in jobs {
        // Admission only ever lets one deployment be in flight, so there should be at most one of
        // these; if a second one somehow exists, only its own eventual failure/success (not a
        // recovery guard collision) should free the pipeline back up.
        if !crate::deploy::try_begin_deploy(app) {
            continue;
        }
        if job.payload.0.get("mode").and_then(Value::as_str) == Some("drain") {
            crate::deploy::begin_drain(app).await;
        }
    }
    Ok(())
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

async fn append_log(app: &AppState, id: i64, line: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE jobs SET log = log || ? WHERE id = ?").bind(format!("{line}\n")).bind(id).execute(&app.db).await?;
    Ok(())
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
        let mut ok = true;
        while let Some(l) = rx.recv().await {
            if append_log(&app2, id, &l).await.is_err() {
                ok = false;
            }
        }
        ok
    });
    let result = dispatch(app, &job, &mut log).await;
    drop(log);
    let log_persisted = drain.await.unwrap_or(false);
    match result {
        Ok(()) if !log_persisted => {
            // The log write (not the terminal status write below) is what failed here — e.g. a
            // disk issue affecting only that `UPDATE`. Mark the job explicitly `failed` rather
            // than writing `succeeded` over a job whose log never made it: a `succeeded` row can't
            // be retried (only `running` jobs are requeued at startup) and never restarts, so the
            // page that triggered it would otherwise be left waiting on it forever with no error
            // and no way to retry (see thread #122 on issue #12).
            let msg = "build succeeded but its log could not be saved; treating it as failed rather than restarting without a durable log";
            tracing::error!("job {} ({}): {msg}", job.id, job.kind);
            let persisted = sqlx::query("UPDATE jobs SET status = 'failed', error = ?, finished_at = ? WHERE id = ?")
                .bind(msg)
                .bind(db::now())
                .bind(job.id)
                .execute(&app.db)
                .await
                .is_ok();
            if job.kind == "server.build" {
                crate::deploy::fail_deploy(app, msg);
            }
            if !persisted {
                tracing::error!("job {} ({}): could not even record the log-write failure; it will be retried at the next startup", job.id, job.kind);
            }
        }
        Ok(()) => {
            let status_persisted = sqlx::query("UPDATE jobs SET status = 'succeeded', error = NULL, finished_at = ? WHERE id = ?")
                .bind(db::now())
                .bind(job.id)
                .execute(&app.db)
                .await
                .is_ok();
            if status_persisted {
                // Only now that the success (and its log) are durable: a restart requested any
                // earlier could exit (or exec) the process before this write lands, and
                // `spawn_worker` would then find the job still `running` on the next startup and
                // requeue (and re-run) it.
                if job.kind == "server.build" {
                    let mode = match job.payload.0.get("mode").and_then(Value::as_str) {
                        Some("drain") => crate::deploy::RestartMode::Drain,
                        _ => crate::deploy::RestartMode::Now,
                    };
                    crate::deploy::schedule_restart(app.clone(), mode);
                }
            } else {
                // Don't restart into a build the server can't prove succeeded. Try recording this
                // as an ordinary failure instead of leaving the row `running` with no error: the
                // 'succeeded' write can fail on its own (e.g. a constraint or trigger specific to
                // that value) without every write being down, so a 'failed' write attempted here
                // often still lands and gives the page a normal, retryable job to look at.
                let msg = "build succeeded but its result could not be durably recorded; treating it as failed rather than restarting without a durable record";
                tracing::error!("job {} ({}): {msg}", job.id, job.kind);
                let failed_persisted = sqlx::query("UPDATE jobs SET status = 'failed', error = ?, finished_at = ? WHERE id = ?")
                    .bind(msg)
                    .bind(db::now())
                    .bind(job.id)
                    .execute(&app.db)
                    .await
                    .is_ok();
                if job.kind == "server.build" {
                    // Independent of whether the write above landed: `deploy_error` is in-memory
                    // and can't itself fail to persist, so the page awaiting this deploy can always
                    // learn it failed and stop waiting, even if the DB is down for every write (see
                    // thread #122 on issue #12). `recover_deploy_state` re-reserves these guards on
                    // the next startup if the job row is still `running` and gets requeued.
                    crate::deploy::fail_deploy(app, msg);
                }
                if !failed_persisted {
                    tracing::error!("job {} ({}): could not even record the completion-write failure; it will be retried at the next startup", job.id, job.kind);
                }
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::warn!("job {} ({}) failed: {msg}", job.id, job.kind);
            let _ = append_log(app, job.id, &format!("error: {msg}")).await;
            let retry = job.attempts < MAX_ATTEMPTS && job.kind != "github.import" && job.kind != "container.build" && job.kind != "server.build";
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
        "server.build" => crate::deploy::run_build_job(app, log).await,
        k => anyhow::bail!("unknown job kind {k}"),
    }
}
