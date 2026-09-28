//! Dispatch loop: picks eligible issues and starts agent runs within the concurrency limits.

use std::collections::HashMap;
use std::time::Duration;

use crate::AppState;
use crate::db;
use crate::domain::models::{AgentDefinition, AgentRun, Issue, Project};
use crate::domain::{IssueState, Role};
use crate::error::{ApiError, ApiResult};

pub fn spawn(app: AppState) {
    tokio::spawn(async move {
        loop {
            let enabled: bool = db::get_setting(&app.db, "scheduler_enabled").await.unwrap_or(false);
            if enabled && let Err(e) = tick(&app).await {
                tracing::warn!("scheduler tick failed: {e:#}");
            }
            tokio::select! {
                _ = app.bus.wake.notified() => {
                    // Debounce bursts of events.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

/// The agent definition that handles `role` in `project`.
pub async fn agent_for(app: &AppState, project: &Project, role: Role) -> anyhow::Result<Option<AgentDefinition>> {
    if let Some(a) = sqlx::query_as::<_, AgentDefinition>(
        "SELECT a.* FROM agent_definitions a JOIN project_role_agents r ON r.agent_definition_id = a.id WHERE r.project_id = ? AND r.role = ?",
    )
    .bind(project.id)
    .bind(role.as_str())
    .fetch_optional(&app.db)
    .await?
    {
        return Ok(Some(a));
    }
    let defaults: HashMap<String, String> = db::get_setting(&app.db, "default_role_agents").await.unwrap_or_default();
    let Some(slug) = defaults.get(role.as_str()) else { return Ok(None) };
    Ok(sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE slug = ?").bind(slug).fetch_optional(&app.db).await?)
}

async fn group_paused(app: &AppState, group: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT paused FROM limit_groups WHERE name = ?")
        .bind(group)
        .fetch_optional(&app.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// For board display: roles whose agent is currently paused, with a "resumes at" hint.
pub async fn paused_roles(app: &AppState, project: &Project) -> anyhow::Result<HashMap<Role, String>> {
    let mut m = HashMap::new();
    for role in crate::domain::actor::ALL_ROLES {
        if let Some(a) = agent_for(app, project, role).await? {
            let row: Option<(bool, Option<String>, Option<String>)> =
                sqlx::query_as("SELECT paused, paused_until, pause_kind FROM limit_groups WHERE name = ?")
                    .bind(&a.limit_group)
                    .fetch_optional(&app.db)
                    .await?;
            if let Some((true, until, kind)) = row {
                m.insert(role, until.unwrap_or_else(|| kind.unwrap_or_else(|| "paused".into())));
            } else if a.needs_auth {
                m.insert(role, "auth".into());
            }
        }
    }
    Ok(m)
}

fn priority_rank(p: &Option<String>) -> i32 {
    match p.as_deref() {
        Some("P0") => 0,
        Some("P1") => 1,
        Some("P2") => 2,
        _ => 3,
    }
}

pub async fn tick(app: &AppState) -> anyhow::Result<()> {
    let max: i64 = db::get_setting(&app.db, "max_concurrent_runs").await.unwrap_or(3);
    let mut active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')").fetch_one(&app.db).await?;
    if active >= max {
        return Ok(());
    }
    let now = db::now();
    let mut candidates = sqlx::query_as::<_, Issue>(
        "SELECT i.* FROM issues i
          WHERE i.hold IS NULL
            AND i.state IN ('triage','ready','in_progress','changes_requested','merge_conflict','in_review')
            AND (i.next_attempt_at IS NULL OR i.next_attempt_at <= ?)
            AND NOT EXISTS (SELECT 1 FROM agent_runs r WHERE r.issue_id = i.id AND r.status IN ('queued','preparing','running'))",
    )
    .bind(&now)
    .fetch_all(&app.db)
    .await?;
    candidates.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then(a.state.precedence().cmp(&b.state.precedence()))
            .then(a.rank.partial_cmp(&b.rank).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.updated_at.cmp(&b.updated_at))
    });

    let mut projects: HashMap<i64, Project> = HashMap::new();
    for issue in candidates {
        if active >= max {
            break;
        }
        let Some(role) = issue.state.dispatch_role() else { continue };
        if let std::collections::hash_map::Entry::Vacant(e) = projects.entry(issue.project_id) {
            e.insert(crate::services::project_by_id(&app.db, issue.project_id).await.map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        let project = &projects[&issue.project_id];
        if !std::path::Path::new(&project.repo_path).exists() {
            continue;
        }
        // Review and merge prep need an open PR; an in-review issue without one waits for a human.
        if matches!(role, Role::Review | Role::MergePrep) && crate::services::pulls::open_pr_for_issue(&app.db, issue.id).await?.is_none() {
            continue;
        }
        let Some(agent) = agent_for(app, project, role).await? else { continue };
        if !agent.enabled || agent.needs_auth || group_paused(app, &agent.limit_group).await {
            continue;
        }
        if let Some(cap) = project.max_concurrent_runs {
            let n: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE project_id = ? AND status IN ('queued','preparing','running')")
                    .bind(project.id)
                    .fetch_one(&app.db)
                    .await?;
            if n >= cap {
                continue;
            }
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs WHERE agent_definition_id = ? AND status IN ('queued','preparing','running')",
        )
        .bind(agent.id)
        .fetch_one(&app.db)
        .await?;
        if n >= agent.max_concurrent {
            continue;
        }
        match start_run(app, project, &issue, role, &agent).await {
            Ok(_) => active += 1,
            Err(e) => tracing::debug!("could not start run for #{}: {e}", issue.number),
        }
    }
    Ok(())
}

/// Create a run row and spawn its task. The unique index on active runs makes this race-free.
pub async fn start_run(app: &AppState, project: &Project, issue: &Issue, role: Role, agent: &AgentDefinition) -> ApiResult<AgentRun> {
    if issue.state == IssueState::Done || issue.state == IssueState::Closed {
        return Err(ApiError::conflict("issue is closed"));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO agent_runs(project_id, issue_id, role, agent_definition_id, status, created_at) VALUES (?, ?, ?, ?, 'queued', ?) RETURNING id",
    )
    .bind(project.id)
    .bind(issue.id)
    .bind(role)
    .bind(agent.id)
    .bind(db::now())
    .fetch_one(&app.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::conflict(format!("issue #{} already has an active run", issue.number)),
        e => e.into(),
    })?;
    tracing::info!("run #{id}: {} #{} with {}", role.as_str(), issue.number, agent.slug);
    let app2 = app.clone();
    tokio::spawn(super::run::execute(app2, id));
    app.bus.run(Some(&project.slug), id);
    app.bus.issue(&project.slug, issue.number);
    Ok(sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE id = ?").bind(id).fetch_one(&app.db).await?)
}
