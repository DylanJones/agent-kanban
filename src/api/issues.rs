use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::domain::models::{AgentRun, Comment, Event, Issue, Label, PullRequest};
use crate::domain::state::allowed_transitions;
use crate::domain::{Actor, Column, IssueState};
use crate::error::{ApiError, ApiResult};
use crate::services::comments::{NewComment, Target};
use crate::services::issues::{CreatedIssue, DecisionAnswer, DecisionRequest, HoldRequest, IssuePatch, NewIssue, TransitionRequest};
use crate::services::{self, comments, issues, labels};

#[derive(Debug, Serialize, ToSchema)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub issue: Issue,
    pub column: Column,
    pub labels: Vec<Label>,
    /// States the caller may move this issue to.
    pub allowed_transitions: Vec<IssueState>,
    pub parent: Option<i64>,
    pub children: Vec<i64>,
    pub comments: Vec<Comment>,
    pub pull_requests: Vec<PullRequest>,
    pub runs: Vec<AgentRun>,
    /// Browser URL.
    pub url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssueListItem {
    #[serde(flatten)]
    pub issue: Issue,
    pub column: Column,
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct IssueQuery {
    /// Filter by state, e.g. `triage`.
    pub state: Option<IssueState>,
    /// Filter by column: `backlog`, `in_progress`, `in_review`, `done`.
    pub column: Option<String>,
    /// Filter by hold: `needs_decision`, `stalled`, `paused`, or `any`.
    pub hold: Option<String>,
    /// Filter by label name.
    pub label: Option<String>,
    /// Case-insensitive substring match on title and body.
    pub q: Option<String>,
    /// Max results (default 200).
    pub limit: Option<i64>,
}

pub async fn detail(app: &AppState, actor: &Actor, project: &crate::domain::models::Project, number: i64) -> ApiResult<IssueDetail> {
    let issue = services::issue(&app.db, project.id, number).await?;
    let labels = labels::issue_labels(&app.db, issue.id).await?;
    let parent = match issue.parent_issue_id {
        Some(pid) => sqlx::query_scalar("SELECT number FROM issues WHERE id = ?").bind(pid).fetch_optional(&app.db).await?,
        None => None,
    };
    let children =
        sqlx::query_scalar("SELECT number FROM issues WHERE parent_issue_id = ? ORDER BY number").bind(issue.id).fetch_all(&app.db).await?;
    let comments = comments::for_issue(app, issue.id).await?;
    let pull_requests = services::pulls::prs_for_issue(&app.db, issue.id).await?;
    let runs = sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE issue_id = ? ORDER BY id DESC")
        .bind(issue.id)
        .fetch_all(&app.db)
        .await?;
    let allowed = match actor {
        Actor::Agent { issue_id, .. } if *issue_id != Some(issue.id) => vec![],
        _ => allowed_transitions(issue.state, actor),
    };
    Ok(IssueDetail {
        column: issue.state.column(),
        url: issues::issue_url(app, project, number),
        labels,
        allowed_transitions: allowed,
        parent,
        children,
        comments,
        pull_requests,
        runs,
        issue,
    })
}

/// List issues.
#[utoipa::path(operation_id = "issues_list", get, path = "/api/projects/{p}/issues", tag = "issues",
    params(("p" = String, Path, description = "Project slug"), IssueQuery),
    responses((status = 200, body = Vec<IssueListItem>)))]
pub async fn list(
    State(app): State<AppState>,
    _actor: Actor,
    Path(p): Path<String>,
    Query(q): Query<IssueQuery>,
) -> ApiResult<Json<Vec<IssueListItem>>> {
    let project = services::project(&app.db, &p).await?;
    let like = q.q.as_ref().map(|s| format!("%{}%", s.to_lowercase()));
    let rows = sqlx::query_as::<_, Issue>(
        "SELECT * FROM issues i WHERE project_id = ?
           AND (? IS NULL OR state = ?)
           AND (? IS NULL OR (? = 'any' AND hold IS NOT NULL) OR hold = ?)
           AND (? IS NULL OR lower(title) LIKE ? OR lower(body) LIKE ?)
           AND (? IS NULL OR EXISTS (SELECT 1 FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = i.id AND l.name = ?))
         ORDER BY number DESC LIMIT ?",
    )
    .bind(project.id)
    .bind(q.state)
    .bind(q.state)
    .bind(&q.hold)
    .bind(&q.hold)
    .bind(&q.hold)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&q.label)
    .bind(&q.label)
    .bind(q.limit.unwrap_or(200))
    .fetch_all(&app.db)
    .await?;
    let label_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT il.issue_id, l.name FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE l.project_id = ?")
            .bind(project.id)
            .fetch_all(&app.db)
            .await?;
    let mut out = Vec::new();
    for i in rows {
        let col = i.state.column();
        if let Some(c) = &q.column
            && serde_json::to_value(col).ok().and_then(|v| v.as_str().map(str::to_string)).as_deref() != Some(c.as_str())
        {
            continue;
        }
        let labels = label_rows.iter().filter(|(id, _)| *id == i.id).map(|(_, n)| n.clone()).collect();
        out.push(IssueListItem { column: col, labels, issue: i });
    }
    Ok(Json(out))
}

/// File a new issue.
///
/// Accepts JSON (`NewIssue`) or `text/plain`, where the first line is the title and the rest is the body.
/// Agents: new issues land in `triage` with the `found-by-agent` label; this is the fastest way to record
/// a bug you noticed while working on something else.
#[utoipa::path(operation_id = "issues_create", post, path = "/api/projects/{p}/issues", tag = "issues",
    params(("p" = String, Path, description = "Project slug")),
    request_body(content((NewIssue = "application/json"), (String = "text/plain"))),
    responses((status = 201, body = CreatedIssue)))]
pub async fn create(
    State(app): State<AppState>,
    actor: Actor,
    Path(p): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<CreatedIssue>)> {
    let project = services::project(&app.db, &p).await?;
    let ct = headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let new = if ct.starts_with("application/json") {
        serde_json::from_slice::<NewIssue>(&body).map_err(|e| ApiError::bad(format!("invalid JSON: {e}")))?
    } else {
        let text = String::from_utf8_lossy(&body);
        let text = text.trim();
        let (title, rest) = text.split_once('\n').unwrap_or((text, ""));
        NewIssue { title: title.trim().trim_start_matches('#').trim().to_string(), body: rest.trim().to_string(), ..Default::default() }
    };
    let created = issues::create(&app, &project, &actor, new).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Get an issue with its comments, PRs, runs and allowed transitions.
#[utoipa::path(operation_id = "issues_get", get, path = "/api/projects/{p}/issues/{n}", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path, description = "Issue number")),
    responses((status = 200, body = IssueDetail)))]
pub async fn get(State(app): State<AppState>, actor: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Edit an issue's fields.
#[utoipa::path(operation_id = "issues_patch", patch, path = "/api/projects/{p}/issues/{n}", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = IssuePatch,
    responses((status = 200, body = IssueDetail)))]
pub async fn patch(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<IssuePatch>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::update(&app, &project, n, &actor, body).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Move an issue to another workflow state.
///
/// Returns 409 `invalid_transition` with `allowed_transitions` if the move is not permitted for the caller.
#[utoipa::path(operation_id = "issues_transition", post, path = "/api/projects/{p}/issues/{n}/transition", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = TransitionRequest,
    responses((status = 200, body = IssueDetail), (status = 409, body = crate::error::Problem)))]
pub async fn transition(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<TransitionRequest>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::transition(&app, &project, n, &actor, body).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Pause the issue for a human decision.
///
/// Only use this when the decision is not already recorded in the issue/PR thread
/// (look for comments of kind `decision`).
#[utoipa::path(operation_id = "issues_decision_request", post, path = "/api/projects/{p}/issues/{n}/decision-request", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = DecisionRequest,
    responses((status = 200, body = IssueDetail)))]
pub async fn decision_request(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<DecisionRequest>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::request_decision(&app, &project, n, &actor, body).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Answer a pending decision (humans only) and optionally resume work.
#[utoipa::path(operation_id = "issues_decision", post, path = "/api/projects/{p}/issues/{n}/decision", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = DecisionAnswer,
    responses((status = 200, body = IssueDetail)))]
pub async fn decision(
    State(app): State<AppState>,
    crate::auth::Human(actor): crate::auth::Human,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<DecisionAnswer>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::answer_decision(&app, &project, n, &actor, body).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Put an issue on hold (humans only).
#[utoipa::path(operation_id = "issues_put_hold", put, path = "/api/projects/{p}/issues/{n}/hold", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = HoldRequest,
    responses((status = 200, body = IssueDetail)))]
pub async fn put_hold(
    State(app): State<AppState>,
    crate::auth::Human(actor): crate::auth::Human,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<HoldRequest>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::set_hold(&app, &project, n, &actor, Some(body)).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// Clear an issue's hold (humans only).
#[utoipa::path(operation_id = "issues_delete_hold", delete, path = "/api/projects/{p}/issues/{n}/hold", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)),
    responses((status = 200, body = IssueDetail)))]
pub async fn delete_hold(
    State(app): State<AppState>,
    crate::auth::Human(actor): crate::auth::Human,
    Path((p, n)): Path<(String, i64)>,
) -> ApiResult<Json<IssueDetail>> {
    let project = services::project(&app.db, &p).await?;
    issues::set_hold(&app, &project, n, &actor, None).await?;
    Ok(Json(detail(&app, &actor, &project, n).await?))
}

/// List comments on an issue.
#[utoipa::path(operation_id = "issues_list_comments", get, path = "/api/projects/{p}/issues/{n}/comments", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)),
    responses((status = 200, body = Vec<Comment>)))]
pub async fn list_comments(State(app): State<AppState>, _actor: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Vec<Comment>>> {
    let project = services::project(&app.db, &p).await?;
    let issue = services::issue(&app.db, project.id, n).await?;
    Ok(Json(comments::for_issue(&app, issue.id).await?))
}

/// Comment on an issue.
#[utoipa::path(operation_id = "issues_add_comment", post, path = "/api/projects/{p}/issues/{n}/comments", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)), request_body = NewComment,
    responses((status = 201, body = Comment)))]
pub async fn add_comment(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(body): Json<NewComment>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
    let project = services::project(&app.db, &p).await?;
    services::ensure_project_access(&actor, &project)?;
    let issue = services::issue(&app.db, project.id, n).await?;
    let c = comments::add(&app, project.id, Target::Issue(issue.id), &actor, &body.body).await?;
    app.bus.issue(&project.slug, n);
    Ok((StatusCode::CREATED, Json(c)))
}

/// Audit log of an issue.
#[utoipa::path(operation_id = "issues_events", get, path = "/api/projects/{p}/issues/{n}/events", tag = "issues",
    params(("p" = String, Path), ("n" = i64, Path)),
    responses((status = 200, body = Vec<Event>)))]
pub async fn events(State(app): State<AppState>, _actor: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Vec<Event>>> {
    let project = services::project(&app.db, &p).await?;
    let issue = services::issue(&app.db, project.id, n).await?;
    let ev = sqlx::query_as::<_, Event>(
        "SELECT * FROM events WHERE issue_id = ? OR pr_id IN (SELECT pr_id FROM pull_request_issues WHERE issue_id = ?) ORDER BY id",
    )
    .bind(issue.id)
    .bind(issue.id)
    .fetch_all(&app.db)
    .await?;
    Ok(Json(ev))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CommentPatch {
    pub body: String,
}

/// Edit a comment (author only).
#[utoipa::path(operation_id = "issues_edit_comment", patch, path = "/api/comments/{id}", tag = "issues",
    params(("id" = i64, Path)), request_body = CommentPatch,
    responses((status = 200, body = Comment)))]
pub async fn edit_comment(
    State(app): State<AppState>,
    actor: Actor,
    Path(id): Path<i64>,
    Json(body): Json<CommentPatch>,
) -> ApiResult<Json<Comment>> {
    let c = comments::get(&app, id).await?;
    comments::ensure_author(&actor, &c).await?;
    sqlx::query("UPDATE comments SET body = ?, updated_at = ? WHERE id = ?")
        .bind(&body.body)
        .bind(crate::db::now())
        .bind(id)
        .execute(&app.db)
        .await?;
    app.bus.emit("comment.updated", None, None, None, None);
    Ok(Json(comments::get(&app, id).await?))
}

/// Delete a comment (author only).
#[utoipa::path(operation_id = "issues_delete_comment", delete, path = "/api/comments/{id}", tag = "issues",
    params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_comment(State(app): State<AppState>, actor: Actor, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    let c = comments::get(&app, id).await?;
    comments::ensure_author(&actor, &c).await?;
    sqlx::query("DELETE FROM comments WHERE id = ?").bind(id).execute(&app.db).await?;
    app.bus.emit("comment.updated", None, None, None, None);
    Ok(StatusCode::NO_CONTENT)
}
