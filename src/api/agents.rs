use std::collections::BTreeMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::Stream;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Human;
use crate::db;
use crate::domain::models::{AgentDefinition, AgentRun, LimitGroup, PermissionRequest, RunEvent};
use crate::domain::{Actor, Role};
use crate::error::{ApiError, ApiResult};
use crate::orchestrator::{limits, probe, scheduler};
use crate::services;

#[derive(Debug, Deserialize, ToSchema)]
pub struct AgentInput {
    pub slug: Option<String>,
    pub name: Option<String>,
    /// `claude`, `codex`, `opencode` or `custom`.
    pub harness: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub env: Option<BTreeMap<String, String>>,
    pub container_command: Option<Vec<String>>,
    pub limit_group: Option<String>,
    pub max_concurrent: Option<i64>,
    /// Host policy: `allowlist`, `auto_allow`, `ask` or `deny`.
    pub permission_policy: Option<String>,
    /// Container policy (default `auto_allow`).
    pub container_permission_policy: Option<String>,
    /// Rules for the `allowlist` policy (first match wins; unmatched prompts go to the Inbox).
    pub permission_rules: Option<Vec<crate::orchestrator::permissions::PermissionRule>>,
    pub session_mode_id: Option<String>,
    pub container_session_mode_id: Option<String>,
    pub enabled: Option<bool>,
}

async fn agent(app: &AppState, slug: &str) -> ApiResult<AgentDefinition> {
    sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE slug = ?")
        .bind(slug)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent `{slug}`")))
}

/// List agent definitions.
#[utoipa::path(operation_id = "agents_list", get, path = "/api/agents", tag = "agents", responses((status = 200, body = Vec<AgentDefinition>)))]
pub async fn list(State(app): State<AppState>, _a: Actor) -> ApiResult<Json<Vec<AgentDefinition>>> {
    Ok(Json(sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions ORDER BY id").fetch_all(&app.db).await?))
}

/// Add an agent definition (any ACP-speaking command).
#[utoipa::path(operation_id = "agents_create", post, path = "/api/agents", tag = "agents", request_body = AgentInput, responses((status = 201, body = AgentDefinition)))]
pub async fn create(
    State(app): State<AppState>,
    Human(_a): Human,
    Json(req): Json<AgentInput>,
) -> ApiResult<(StatusCode, Json<AgentDefinition>)> {
    let slug = req.slug.clone().ok_or_else(|| ApiError::bad("slug is required"))?;
    let command = req.command.clone().ok_or_else(|| ApiError::bad("command is required"))?;
    let harness = req.harness.clone().unwrap_or_else(|| "custom".into());
    let now = db::now();
    let group = req.limit_group.clone().unwrap_or_else(|| if harness == "custom" { slug.clone() } else { harness.clone() });
    sqlx::query(
        "INSERT INTO agent_definitions(slug, name, harness, command, args, env, container_command, limit_group, max_concurrent,
             permission_policy, container_permission_policy, permission_rules, session_mode_id, container_session_mode_id,
             enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&slug)
    .bind(req.name.clone().unwrap_or_else(|| slug.clone()))
    .bind(&harness)
    .bind(&command)
    .bind(serde_json::to_string(&req.args.clone().unwrap_or_default()).unwrap())
    .bind(serde_json::to_string(&req.env.clone().unwrap_or_default()).unwrap())
    .bind(req.container_command.as_ref().map(|c| serde_json::to_string(c).unwrap()))
    .bind(&group)
    .bind(req.max_concurrent.unwrap_or(2))
    .bind(req.permission_policy.clone().unwrap_or_else(|| "allowlist".into()))
    .bind(req.container_permission_policy.clone().unwrap_or_else(|| "auto_allow".into()))
    .bind(serde_json::to_string(&req.permission_rules.clone().unwrap_or_else(crate::orchestrator::permissions::default_rules)).unwrap())
    .bind(&req.session_mode_id)
    .bind(&req.container_session_mode_id)
    .bind(req.enabled.unwrap_or(true))
    .bind(&now)
    .bind(&now)
    .execute(&app.db)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO limit_groups(name, updated_at) VALUES (?, ?)").bind(&group).bind(&now).execute(&app.db).await?;
    app.bus.emit("agents.updated", None, None, None, None);
    Ok((StatusCode::CREATED, Json(agent(&app, &slug).await?)))
}

/// Get an agent definition.
#[utoipa::path(operation_id = "agents_get", get, path = "/api/agents/{slug}", tag = "agents", params(("slug" = String, Path)), responses((status = 200, body = AgentDefinition)))]
pub async fn get(State(app): State<AppState>, _a: Actor, Path(slug): Path<String>) -> ApiResult<Json<AgentDefinition>> {
    Ok(Json(agent(&app, &slug).await?))
}

/// Update an agent definition.
#[utoipa::path(operation_id = "agents_patch", patch, path = "/api/agents/{slug}", tag = "agents", params(("slug" = String, Path)), request_body = AgentInput, responses((status = 200, body = AgentDefinition)))]
pub async fn patch(
    State(app): State<AppState>,
    Human(_a): Human,
    Path(slug): Path<String>,
    Json(req): Json<AgentInput>,
) -> ApiResult<Json<AgentDefinition>> {
    let a = agent(&app, &slug).await?;
    for p in [&req.permission_policy, &req.container_permission_policy].into_iter().flatten() {
        if !matches!(p.as_str(), "allowlist" | "auto_allow" | "ask" | "deny") {
            return Err(ApiError::bad("permission policies must be allowlist, auto_allow, ask or deny"));
        }
    }
    if let Some(rules) = &req.permission_rules {
        for r in rules {
            if let Some(p) = &r.pattern {
                regex::Regex::new(&p.replace("{worktree}", "x")).map_err(|e| ApiError::bad(format!("invalid pattern `{p}`: {e}")))?;
            }
        }
    }
    macro_rules! set {
        ($v:expr, $col:literal) => {
            if let Some(v) = $v {
                sqlx::query(concat!("UPDATE agent_definitions SET ", $col, " = ? WHERE id = ?"))
                    .bind(v)
                    .bind(a.id)
                    .execute(&app.db)
                    .await?;
            }
        };
    }
    set!(&req.name, "name");
    set!(&req.harness, "harness");
    set!(&req.command, "command");
    set!(req.args.as_ref().map(|v| serde_json::to_string(v).unwrap()), "args");
    set!(req.env.as_ref().map(|v| serde_json::to_string(v).unwrap()), "env");
    set!(req.container_command.as_ref().map(|v| serde_json::to_string(v).unwrap()), "container_command");
    set!(&req.limit_group, "limit_group");
    set!(req.max_concurrent, "max_concurrent");
    set!(&req.permission_policy, "permission_policy");
    set!(&req.container_permission_policy, "container_permission_policy");
    set!(req.permission_rules.as_ref().map(|v| serde_json::to_string(v).unwrap()), "permission_rules");
    set!(req.session_mode_id.as_ref().map(|s| (!s.is_empty()).then(|| s.clone())), "session_mode_id");
    set!(req.container_session_mode_id.as_ref().map(|s| (!s.is_empty()).then(|| s.clone())), "container_session_mode_id");
    set!(req.enabled, "enabled");
    if let Some(g) = &req.limit_group {
        sqlx::query("INSERT OR IGNORE INTO limit_groups(name, updated_at) VALUES (?, ?)").bind(g).bind(db::now()).execute(&app.db).await?;
    }
    sqlx::query("UPDATE agent_definitions SET updated_at = ? WHERE id = ?").bind(db::now()).bind(a.id).execute(&app.db).await?;
    app.bus.emit("agents.updated", None, None, None, None);
    Ok(Json(agent(&app, &slug).await?))
}

/// Delete an agent definition (only if it has no runs; otherwise disable it).
#[utoipa::path(operation_id = "agents_delete", delete, path = "/api/agents/{slug}", tag = "agents", params(("slug" = String, Path)), responses((status = 204)))]
pub async fn delete(State(app): State<AppState>, Human(_a): Human, Path(slug): Path<String>) -> ApiResult<StatusCode> {
    let a = agent(&app, &slug).await?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE agent_definition_id = ?)").bind(a.id).fetch_one(&app.db).await?;
    if used {
        return Err(ApiError::conflict("agent has run history; disable it instead"));
    }
    sqlx::query("DELETE FROM agent_definitions WHERE id = ?").bind(a.id).execute(&app.db).await?;
    app.bus.emit("agents.updated", None, None, None, None);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct TestRequest {
    /// Also send a tiny prompt (uses a little quota) to check the subscription works.
    #[serde(default)]
    pub prompt: bool,
}

/// Start the adapter, perform the ACP handshake (and optionally a one-line prompt), and report what happened.
#[utoipa::path(operation_id = "agents_test", post, path = "/api/agents/{slug}/test", tag = "agents", params(("slug" = String, Path)), request_body(content = Option<TestRequest>), responses((status = 200, body = probe::AgentTestResult)))]
pub async fn test(
    State(app): State<AppState>,
    Human(_a): Human,
    Path(slug): Path<String>,
    body: Option<Json<TestRequest>>,
) -> ApiResult<Json<probe::AgentTestResult>> {
    let a = agent(&app, &slug).await?;
    let with_prompt = body.map(|b| b.0.prompt).unwrap_or(false);
    let r = probe::session_check(
        &app,
        &a,
        with_prompt.then_some("Reply with exactly: OK"),
        Duration::from_secs(if with_prompt { 180 } else { 120 }),
    )
    .await?;
    if r.ok && a.needs_auth {
        sqlx::query("UPDATE agent_definitions SET needs_auth = 0 WHERE id = ?").bind(a.id).execute(&app.db).await?;
        app.bus.emit("agents.updated", None, None, None, None);
    }
    Ok(Json(r))
}

/// Subscription limit groups and their pause state.
#[utoipa::path(operation_id = "agents_limit_groups", get, path = "/api/limit-groups", tag = "agents", responses((status = 200, body = Vec<LimitGroup>)))]
pub async fn limit_groups(State(app): State<AppState>, _a: Actor) -> ApiResult<Json<Vec<LimitGroup>>> {
    Ok(Json(sqlx::query_as::<_, LimitGroup>("SELECT * FROM limit_groups ORDER BY name").fetch_all(&app.db).await?))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct PauseRequest {
    /// RFC3339 time to resume automatically; omit to pause until resumed.
    pub until: Option<String>,
}

/// Pause every agent in a limit group.
#[utoipa::path(operation_id = "agents_pause", post, path = "/api/limit-groups/{name}/pause", tag = "agents", params(("name" = String, Path)), request_body(content = Option<PauseRequest>), responses((status = 200, body = Vec<LimitGroup>)))]
pub async fn pause(
    State(app): State<AppState>,
    Human(a): Human,
    Path(name): Path<String>,
    body: Option<Json<PauseRequest>>,
) -> ApiResult<Json<Vec<LimitGroup>>> {
    let until = body.and_then(|b| b.0.until).and_then(|u| db::parse_time(&u));
    limits::pause_manual(&app, &name, until).await?;
    limit_groups(State(app), a).await
}

/// Resume a paused limit group now.
#[utoipa::path(operation_id = "agents_resume", post, path = "/api/limit-groups/{name}/resume", tag = "agents", params(("name" = String, Path)), responses((status = 200, body = Vec<LimitGroup>)))]
pub async fn resume(State(app): State<AppState>, Human(a): Human, Path(name): Path<String>) -> ApiResult<Json<Vec<LimitGroup>>> {
    limits::resume(&app, &name).await?;
    app.bus.wake.notify_one();
    limit_groups(State(app), a).await
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProbeResult {
    pub available: bool,
}

/// Check now whether a paused group's subscription is usable again (sends a one-line prompt).
#[utoipa::path(operation_id = "agents_probe_now", post, path = "/api/limit-groups/{name}/probe", tag = "agents", params(("name" = String, Path)), responses((status = 200, body = ProbeResult)))]
pub async fn probe_now(State(app): State<AppState>, Human(_a): Human, Path(name): Path<String>) -> ApiResult<Json<ProbeResult>> {
    let ok = probe::probe_group(&app, &name).await?;
    if ok {
        limits::resume(&app, &name).await?;
    }
    Ok(Json(ProbeResult { available: ok }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunView {
    #[serde(flatten)]
    pub run: AgentRun,
    pub agent: String,
    pub project: String,
    pub issue: Option<i64>,
    pub issue_title: Option<String>,
    pub pr: Option<i64>,
    /// True while the run's task is alive in this server process.
    pub live: bool,
}

async fn run_view(app: &AppState, run: AgentRun) -> ApiResult<RunView> {
    let agent: String =
        sqlx::query_scalar("SELECT slug FROM agent_definitions WHERE id = ?").bind(run.agent_definition_id).fetch_one(&app.db).await?;
    let project: String = sqlx::query_scalar("SELECT slug FROM projects WHERE id = ?").bind(run.project_id).fetch_one(&app.db).await?;
    let (issue, issue_title) = match run.issue_id {
        Some(i) => {
            let r: Option<(i64, String)> =
                sqlx::query_as("SELECT number, title FROM issues WHERE id = ?").bind(i).fetch_optional(&app.db).await?;
            r.map(|(n, t)| (Some(n), Some(t))).unwrap_or((None, None))
        }
        None => (None, None),
    };
    let pr = match run.pr_id {
        Some(p) => sqlx::query_scalar("SELECT number FROM pull_requests WHERE id = ?").bind(p).fetch_optional(&app.db).await?,
        None => None,
    };
    Ok(RunView { live: app.runs.is_live(run.id), agent, project, issue, issue_title, pr, run })
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct RunQuery {
    /// Comma-separated statuses, or `active`.
    pub status: Option<String>,
    pub project: Option<String>,
    pub issue: Option<i64>,
    pub limit: Option<i64>,
}

/// List agent runs.
#[utoipa::path(operation_id = "agents_list_runs", get, path = "/api/runs", tag = "runs", params(RunQuery), responses((status = 200, body = Vec<RunView>)))]
pub async fn list_runs(State(app): State<AppState>, _a: Actor, Query(q): Query<RunQuery>) -> ApiResult<Json<Vec<RunView>>> {
    let statuses: Option<Vec<String>> = q.status.as_ref().map(|s| {
        if s == "active" { vec!["queued".into(), "preparing".into(), "running".into()] } else { s.split(',').map(str::to_string).collect() }
    });
    let project_id: Option<i64> = match &q.project {
        Some(p) => Some(services::project(&app.db, p).await?.id),
        None => None,
    };
    let issue_id: Option<i64> = match (q.issue, project_id) {
        (Some(n), Some(pid)) => Some(services::issue(&app.db, pid, n).await?.id),
        _ => None,
    };
    let rows = sqlx::query_as::<_, AgentRun>(
        "SELECT * FROM agent_runs WHERE (? IS NULL OR project_id = ?) AND (? IS NULL OR issue_id = ?)
           AND (? IS NULL OR status IN (SELECT value FROM json_each(?))) ORDER BY id DESC LIMIT ?",
    )
    .bind(project_id)
    .bind(project_id)
    .bind(issue_id)
    .bind(issue_id)
    .bind(statuses.as_ref().map(|_| 1))
    .bind(statuses.as_ref().map(|s| serde_json::to_string(s).unwrap()))
    .bind(q.limit.unwrap_or(100))
    .fetch_all(&app.db)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        out.push(run_view(&app, r).await?);
    }
    Ok(Json(out))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentRun {
    pub run_id: i64,
    pub role: Role,
    pub project: String,
    pub issue: Option<i64>,
    pub issue_title: Option<String>,
    pub issue_state: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub pr: Option<i64>,
    pub pr_head_sha: Option<String>,
    /// What the board expects from this run before it ends.
    pub expected: String,
}

/// For agents: the run this token belongs to and what's expected of it.
#[utoipa::path(operation_id = "agents_current", get, path = "/api/runs/current", tag = "runs", responses((status = 200, body = CurrentRun)))]
pub async fn current(State(app): State<AppState>, actor: Actor) -> ApiResult<Json<CurrentRun>> {
    let Actor::Agent { run_id, role, .. } = actor else {
        return Err(ApiError::bad("only agent run tokens have a current run"));
    };
    let run = sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE id = ?").bind(run_id).fetch_one(&app.db).await?;
    let view = run_view(&app, run.clone()).await?;
    let issue = match run.issue_id {
        Some(i) => Some(services::issue_by_id(&app.db, i).await?),
        None => None,
    };
    let pr = match &issue {
        Some(i) => services::pulls::open_pr_for_issue(&app.db, i.id).await?,
        None => None,
    };
    let branch = match &run.worktree_path {
        Some(w) => crate::git::run(w, &["symbolic-ref", "--short", "-q", "HEAD"]).await.ok(),
        None => None,
    };
    let expected = match role {
        Role::Triage => "Move the issue out of `triage` (to ready, backlog or closed) after posting a triage comment.",
        Role::Fix => "Commit the fix, open a PR if none exists, then move the issue to `in_review`.",
        Role::Review => "Post exactly one review verdict for the current PR head.",
        Role::MergePrep => "Merge the base branch into the PR branch, resolve conflicts, commit, then move the issue to `in_review`.",
    };
    Ok(Json(CurrentRun {
        run_id,
        role,
        project: view.project,
        issue: issue.as_ref().map(|i| i.number),
        issue_title: issue.as_ref().map(|i| i.title.clone()),
        issue_state: issue.as_ref().map(|i| i.state.as_str().to_string()),
        worktree: run.worktree_path.clone(),
        branch,
        pr: pr.as_ref().map(|p| p.number),
        pr_head_sha: pr.and_then(|p| p.head_sha),
        expected: expected.into(),
    }))
}

/// Get a run.
#[utoipa::path(operation_id = "agents_get_run", get, path = "/api/runs/{id}", tag = "runs", params(("id" = i64, Path)), responses((status = 200, body = RunView)))]
pub async fn get_run(State(app): State<AppState>, _a: Actor, Path(id): Path<i64>) -> ApiResult<Json<RunView>> {
    let r = sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE id = ?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("run {id}")))?;
    Ok(Json(run_view(&app, r).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct EventsQuery {
    /// Only events with `seq` greater than this.
    pub after: Option<i64>,
}

/// Transcript events of a run (messages, thoughts, tool calls, plan, status...).
#[utoipa::path(operation_id = "agents_run_events", get, path = "/api/runs/{id}/events", tag = "runs", params(("id" = i64, Path), EventsQuery), responses((status = 200, body = Vec<RunEvent>)))]
pub async fn run_events(
    State(app): State<AppState>,
    _a: Actor,
    Path(id): Path<i64>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<RunEvent>>> {
    Ok(Json(
        sqlx::query_as::<_, RunEvent>("SELECT * FROM run_events WHERE run_id = ? AND seq > ? ORDER BY seq")
            .bind(id)
            .bind(q.after.unwrap_or(0))
            .fetch_all(&app.db)
            .await?,
    ))
}

/// Live transcript as server-sent events (`event: run_event`, data = RunEvent). Events with a
/// `seq` you've already seen are updates to that event (e.g. a tool call finishing).
#[utoipa::path(operation_id = "agents_run_stream", get, path = "/api/runs/{id}/stream", tag = "runs", params(("id" = i64, Path)), responses((status = 200, content_type = "text/event-stream", description = "RunEvent stream")))]
pub async fn run_stream(
    State(app): State<AppState>,
    _a: Actor,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let after: i64 = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let rx = app.runs.subscribe(id);
    let stream = async_stream::stream! {
        let backlog = sqlx::query_as::<_, RunEvent>("SELECT * FROM run_events WHERE run_id = ? AND seq > ? ORDER BY seq")
            .bind(id).bind(after).fetch_all(&app.db).await.unwrap_or_default();
        for ev in backlog {
            yield Ok(SseEvent::default().event("run_event").id(ev.seq.to_string()).json_data(&ev).unwrap());
        }
        if let Some(mut rx) = rx {
            loop {
                match rx.recv().await {
                    Ok(ev) => yield Ok(SseEvent::default().event("run_event").id(ev.seq.to_string()).json_data(&ev).unwrap()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
        yield Ok(SseEvent::default().event("end").data("{}"));
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Stop a run.
#[utoipa::path(operation_id = "agents_cancel", post, path = "/api/runs/{id}/cancel", tag = "runs", params(("id" = i64, Path)), responses((status = 200, body = RunView)))]
pub async fn cancel(State(app): State<AppState>, Human(a): Human, Path(id): Path<i64>) -> ApiResult<Json<RunView>> {
    if !app.runs.cancel_and_wait(id, &format!("stopped by {}", a.name()), Duration::from_secs(20)).await {
        sqlx::query("UPDATE agent_runs SET status = 'cancelled', ended_at = ? WHERE id = ? AND status IN ('queued','preparing','running')")
            .bind(db::now())
            .bind(id)
            .execute(&app.db)
            .await?;
    }
    app.bus.run(None, id);
    get_run(State(app), a, Path(id)).await
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RunMessage {
    pub text: String,
}

/// Send a follow-up message to a running agent (delivered as its next prompt turn).
#[utoipa::path(operation_id = "agents_message", post, path = "/api/runs/{id}/messages", tag = "runs", params(("id" = i64, Path)), request_body = RunMessage, responses((status = 202)))]
pub async fn message(
    State(app): State<AppState>,
    Human(_a): Human,
    Path(id): Path<i64>,
    Json(req): Json<RunMessage>,
) -> ApiResult<StatusCode> {
    if app.runs.send_followup(id, req.text) { Ok(StatusCode::ACCEPTED) } else { Err(ApiError::conflict("run is not active")) }
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct StartRun {
    /// Defaults to the role implied by the issue's state.
    pub role: Option<Role>,
    /// Agent slug; defaults to the project's agent for the role.
    pub agent: Option<String>,
}

/// Start an agent run on an issue now (ignores the scheduler switch, but not holds or concurrency).
#[utoipa::path(operation_id = "agents_start", post, path = "/api/projects/{p}/issues/{n}/runs", tag = "runs", params(("p" = String, Path), ("n" = i64, Path)), request_body(content = Option<StartRun>), responses((status = 201, body = RunView)))]
pub async fn start(
    State(app): State<AppState>,
    Human(_a): Human,
    Path((p, n)): Path<(String, i64)>,
    body: Option<Json<StartRun>>,
) -> ApiResult<(StatusCode, Json<RunView>)> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let project = services::project(&app.db, &p).await?;
    let issue = services::issue(&app.db, project.id, n).await?;
    if issue.hold.is_some() {
        return Err(ApiError::conflict("issue is on hold; clear the hold first"));
    }
    let role = req
        .role
        .or(issue.state.dispatch_role())
        .ok_or_else(|| ApiError::bad(format!("no agent role for state {}", issue.state.as_str())))?;
    let a = match &req.agent {
        Some(s) => agent(&app, s).await?,
        None => scheduler::agent_for(&app, &project, role).await?.ok_or_else(|| ApiError::bad("no agent configured for this role"))?,
    };
    let max: i64 = db::get_setting(&app.db, "max_concurrent_runs").await.unwrap_or(3);
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')").fetch_one(&app.db).await?;
    if active >= max {
        return Err(ApiError::conflict(format!("{active} runs already active (limit {max})")));
    }
    let r = scheduler::start_run(&app, &project, &issue, role, &a).await?;
    Ok((StatusCode::CREATED, Json(run_view(&app, r).await?)))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PermQuery {
    /// `pending` (default), `answered`, `expired`, `all`.
    pub status: Option<String>,
}

/// Permission prompts from agents whose policy is `ask`.
#[utoipa::path(operation_id = "agents_permissions", get, path = "/api/permission-requests", tag = "runs", params(PermQuery), responses((status = 200, body = Vec<PermissionRequest>)))]
pub async fn permissions(State(app): State<AppState>, _a: Actor, Query(q): Query<PermQuery>) -> ApiResult<Json<Vec<PermissionRequest>>> {
    let status = q.status.unwrap_or_else(|| "pending".into());
    Ok(Json(
        sqlx::query_as::<_, PermissionRequest>(
            "SELECT * FROM permission_requests WHERE (? = 'all' OR status = ?) ORDER BY id DESC LIMIT 100",
        )
        .bind(&status)
        .bind(&status)
        .fetch_all(&app.db)
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PermissionAnswer {
    /// One of the request's `options[].optionId`.
    pub option_id: String,
}

/// Answer an agent's permission prompt.
#[utoipa::path(operation_id = "agents_answer_permission", post, path = "/api/permission-requests/{id}", tag = "runs", params(("id" = i64, Path)), request_body = PermissionAnswer, responses((status = 200, body = PermissionRequest)))]
pub async fn answer_permission(
    State(app): State<AppState>,
    Human(a): Human,
    Path(id): Path<i64>,
    Json(req): Json<PermissionAnswer>,
) -> ApiResult<Json<PermissionRequest>> {
    let delivered = app.runs.answer_permission(id, req.option_id.clone());
    sqlx::query("UPDATE permission_requests SET status = ?, selected_option_id = ?, answered_by = ?, answered_at = ? WHERE id = ?")
        .bind(if delivered { "answered" } else { "expired" })
        .bind(&req.option_id)
        .bind(a.name())
        .bind(db::now())
        .bind(id)
        .execute(&app.db)
        .await?;
    app.bus.emit("permissions.updated", None, None, None, None);
    Ok(Json(sqlx::query_as::<_, PermissionRequest>("SELECT * FROM permission_requests WHERE id = ?").bind(id).fetch_one(&app.db).await?))
}
