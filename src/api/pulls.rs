use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Human;
use crate::domain::Actor;
use crate::domain::models::{Comment, PullRequest, Review};
use crate::error::ApiResult;
use crate::git::CommitInfo;
use crate::git::diff::FileDiff;
use crate::services::comments::{NewComment, Target};
use crate::services::merge::{MergeRequest, MergeResult};
use crate::services::pulls::{Mergeability, NewPull, NewThread, PullPatch, ThreadWithComments};
use crate::services::reviews::NewReview;
use crate::services::{self, comments, merge, pulls, reviews};

#[derive(Debug, Serialize, ToSchema)]
pub struct PullDetail {
    #[serde(flatten)]
    pub pr: PullRequest,
    /// Issue numbers this PR resolves.
    pub issues: Vec<i64>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub threads: Vec<ThreadWithComments>,
    pub url: String,
    /// Default commit message a squash merge would use.
    pub default_merge_message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PullDiff {
    /// The commit the diff starts from (merge base, or `since`).
    pub from: String,
    pub head: Option<String>,
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DiffQuery {
    /// Show only changes since this commit (e.g. your last reviewed SHA).
    pub since: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PullQuery {
    /// `open`, `merged`, `closed`.
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ThreadQuery {
    pub resolved: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResolveRequest {
    /// Optional note added to the thread, e.g. "Fixed in abc123".
    pub comment: Option<String>,
}

async fn detail(app: &AppState, p: &str, n: i64) -> ApiResult<PullDetail> {
    let project = services::project(&app.db, p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    let pr = pulls::refresh_head(app, &project, &pr).await?;
    Ok(PullDetail {
        issues: pulls::linked_issue_numbers(&app.db, pr.id).await?,
        comments: comments::for_pr(app, pr.id).await?,
        reviews: reviews::list(app, pr.id).await?,
        threads: pulls::threads(app, pr.id, None).await?,
        url: format!("{}/p/{}/pulls/{}", app.config.public_url, project.slug, n),
        default_merge_message: merge::default_message(app, pr.id, &pr.title).await,
        pr,
    })
}

/// List pull requests.
#[utoipa::path(operation_id = "pulls_list", get, path = "/api/projects/{p}/pulls", tag = "pulls", params(("p" = String, Path), PullQuery), responses((status = 200, body = Vec<PullRequest>)))]
pub async fn list(
    State(app): State<AppState>,
    _a: Actor,
    Path(p): Path<String>,
    Query(q): Query<PullQuery>,
) -> ApiResult<Json<Vec<PullRequest>>> {
    let project = services::project(&app.db, &p).await?;
    Ok(Json(
        sqlx::query_as::<_, PullRequest>(
            "SELECT * FROM pull_requests WHERE project_id = ? AND (? IS NULL OR state = ?) ORDER BY number DESC",
        )
        .bind(project.id)
        .bind(&q.state)
        .bind(&q.state)
        .fetch_all(&app.db)
        .await?,
    ))
}

/// Open a pull request for a branch.
///
/// Agents: `branch` and `issues` default to your run's worktree branch and issue. Commit first —
/// the branch must have commits ahead of the base branch.
#[utoipa::path(operation_id = "pulls_create", post, path = "/api/projects/{p}/pulls", tag = "pulls", params(("p" = String, Path)), request_body = NewPull, responses((status = 201, body = PullDetail)))]
pub async fn create(
    State(app): State<AppState>,
    actor: Actor,
    Path(p): Path<String>,
    Json(req): Json<NewPull>,
) -> ApiResult<(StatusCode, Json<PullDetail>)> {
    let project = services::project(&app.db, &p).await?;
    let pr = pulls::create(&app, &project, &actor, req).await?;
    Ok((StatusCode::CREATED, Json(detail(&app, &p, pr.number).await?)))
}

/// Get a pull request with comments, reviews and inline threads.
#[utoipa::path(operation_id = "pulls_get", get, path = "/api/projects/{p}/pulls/{n}", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = PullDetail)))]
pub async fn get(State(app): State<AppState>, _a: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<PullDetail>> {
    Ok(Json(detail(&app, &p, n).await?))
}

/// Edit a pull request's title or description.
#[utoipa::path(operation_id = "pulls_patch", patch, path = "/api/projects/{p}/pulls/{n}", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), request_body = PullPatch, responses((status = 200, body = PullDetail)))]
pub async fn patch(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(req): Json<PullPatch>,
) -> ApiResult<Json<PullDetail>> {
    let project = services::project(&app.db, &p).await?;
    pulls::update(&app, &project, n, &actor, req).await?;
    Ok(Json(detail(&app, &p, n).await?))
}

/// Unified diff of the PR against its merge base, split per file.
#[utoipa::path(operation_id = "pulls_diff", get, path = "/api/projects/{p}/pulls/{n}/diff", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path), DiffQuery), responses((status = 200, body = PullDiff)))]
pub async fn diff(
    State(app): State<AppState>,
    _a: Actor,
    Path((p, n)): Path<(String, i64)>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<PullDiff>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    let pr = pulls::refresh_head(&app, &project, &pr).await?;
    let files = pulls::diff(&app, &project, &pr, q.since.as_deref()).await?;
    let from = q.since.clone().or(pr.merge_base_sha.clone()).unwrap_or_else(|| pr.base_branch.clone());
    Ok(Json(PullDiff { from, head: pr.head_sha.clone(), files }))
}

/// Commits on the PR branch that are not on the base branch.
#[utoipa::path(operation_id = "pulls_commits", get, path = "/api/projects/{p}/pulls/{n}/commits", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = Vec<CommitInfo>)))]
pub async fn commits(State(app): State<AppState>, _a: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Vec<CommitInfo>>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    let head = pr.head_sha.clone().unwrap_or_else(|| pr.branch.clone());
    let from = pr.merge_base_sha.clone().unwrap_or_else(|| pr.base_branch.clone());
    let range = format!("{from}..{head}");
    Ok(Json(crate::git::log(&project.repo_path, &range, 200).await.unwrap_or_default()))
}

/// List PR conversation comments.
#[utoipa::path(operation_id = "pulls_list_comments", get, path = "/api/projects/{p}/pulls/{n}/comments", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = Vec<Comment>)))]
pub async fn list_comments(State(app): State<AppState>, _a: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Vec<Comment>>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    Ok(Json(comments::for_pr(&app, pr.id).await?))
}

/// Add a PR conversation comment (not attached to a line).
#[utoipa::path(operation_id = "pulls_add_comment", post, path = "/api/projects/{p}/pulls/{n}/comments", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), request_body = NewComment, responses((status = 201, body = Comment)))]
pub async fn add_comment(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(req): Json<NewComment>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
    let project = services::project(&app.db, &p).await?;
    services::ensure_project_access(&actor, &project)?;
    let pr = services::pull(&app.db, project.id, n).await?;
    let c = comments::add(&app, project.id, Target::Pr(pr.id), &actor, &req.body).await?;
    app.bus.pr(&p, n);
    Ok((StatusCode::CREATED, Json(c)))
}

/// List inline review threads.
#[utoipa::path(operation_id = "pulls_list_threads", get, path = "/api/projects/{p}/pulls/{n}/threads", tag = "reviews", params(("p" = String, Path), ("n" = i64, Path), ThreadQuery), responses((status = 200, body = Vec<ThreadWithComments>)))]
pub async fn list_threads(
    State(app): State<AppState>,
    _a: Actor,
    Path((p, n)): Path<(String, i64)>,
    Query(q): Query<ThreadQuery>,
) -> ApiResult<Json<Vec<ThreadWithComments>>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    Ok(Json(pulls::threads(&app, pr.id, q.resolved).await?))
}

/// Start an inline review thread on a line of the diff.
#[utoipa::path(operation_id = "pulls_create_thread", post, path = "/api/projects/{p}/pulls/{n}/threads", tag = "reviews", params(("p" = String, Path), ("n" = i64, Path)), request_body = NewThread, responses((status = 201, body = ThreadWithComments)))]
pub async fn create_thread(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(req): Json<NewThread>,
) -> ApiResult<(StatusCode, Json<ThreadWithComments>)> {
    let project = services::project(&app.db, &p).await?;
    Ok((StatusCode::CREATED, Json(pulls::create_thread(&app, &project, n, &actor, req).await?)))
}

/// Reply to an inline thread.
#[utoipa::path(operation_id = "pulls_reply", post, path = "/api/threads/{id}/replies", tag = "reviews", params(("id" = i64, Path)), request_body = NewComment, responses((status = 201, body = ThreadWithComments)))]
pub async fn reply(
    State(app): State<AppState>,
    actor: Actor,
    Path(id): Path<i64>,
    Json(req): Json<NewComment>,
) -> ApiResult<(StatusCode, Json<ThreadWithComments>)> {
    Ok((StatusCode::CREATED, Json(pulls::reply(&app, id, &actor, &req.body).await?)))
}

/// Mark a thread resolved.
#[utoipa::path(operation_id = "pulls_resolve", post, path = "/api/threads/{id}/resolve", tag = "reviews", params(("id" = i64, Path)), request_body(content = Option<ResolveRequest>), responses((status = 200, body = ThreadWithComments)))]
pub async fn resolve(
    State(app): State<AppState>,
    actor: Actor,
    Path(id): Path<i64>,
    body: Option<Json<ResolveRequest>>,
) -> ApiResult<Json<ThreadWithComments>> {
    let note = body.and_then(|b| b.0.comment);
    Ok(Json(pulls::set_resolved(&app, id, &actor, true, note.as_deref()).await?))
}

/// Reopen a resolved thread.
#[utoipa::path(operation_id = "pulls_unresolve", post, path = "/api/threads/{id}/unresolve", tag = "reviews", params(("id" = i64, Path)), request_body(content = Option<ResolveRequest>), responses((status = 200, body = ThreadWithComments)))]
pub async fn unresolve(
    State(app): State<AppState>,
    actor: Actor,
    Path(id): Path<i64>,
    body: Option<Json<ResolveRequest>>,
) -> ApiResult<Json<ThreadWithComments>> {
    let note = body.and_then(|b| b.0.comment);
    Ok(Json(pulls::set_resolved(&app, id, &actor, false, note.as_deref()).await?))
}

/// List reviews (verdicts) on a PR.
#[utoipa::path(operation_id = "pulls_list_reviews", get, path = "/api/projects/{p}/pulls/{n}/reviews", tag = "reviews", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = Vec<Review>)))]
pub async fn list_reviews(State(app): State<AppState>, _a: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Vec<Review>>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    Ok(Json(reviews::list(&app, pr.id).await?))
}

/// Record a review verdict for the current head. Moves the linked issues:
/// `approve` → ready_to_merge, `changes_requested` → changes_requested, `needs_decision` → hold.
#[utoipa::path(operation_id = "pulls_submit_review", post, path = "/api/projects/{p}/pulls/{n}/reviews", tag = "reviews", params(("p" = String, Path), ("n" = i64, Path)), request_body = NewReview, responses((status = 201, body = Review), (status = 409, body = crate::error::Problem)))]
pub async fn submit_review(
    State(app): State<AppState>,
    actor: Actor,
    Path((p, n)): Path<(String, i64)>,
    Json(req): Json<NewReview>,
) -> ApiResult<(StatusCode, Json<Review>)> {
    let project = services::project(&app.db, &p).await?;
    Ok((StatusCode::CREATED, Json(reviews::submit(&app, &project, n, &actor, req).await?)))
}

/// Can this PR be merged right now, and if not, why?
#[utoipa::path(operation_id = "pulls_mergeability", get, path = "/api/projects/{p}/pulls/{n}/mergeability", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = Mergeability)))]
pub async fn mergeability(State(app): State<AppState>, _a: Actor, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<Mergeability>> {
    let project = services::project(&app.db, &p).await?;
    let pr = services::pull(&app.db, project.id, n).await?;
    Ok(Json(pulls::mergeability(&app, &project, &pr).await?))
}

/// Merge the PR into its base branch (humans only).
#[utoipa::path(operation_id = "pulls_do_merge", post, path = "/api/projects/{p}/pulls/{n}/merge", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), request_body = MergeRequest, responses((status = 200, body = MergeResult)))]
pub async fn do_merge(
    State(app): State<AppState>,
    Human(actor): Human,
    Path((p, n)): Path<(String, i64)>,
    Json(req): Json<MergeRequest>,
) -> ApiResult<Json<MergeResult>> {
    let project = services::project(&app.db, &p).await?;
    Ok(Json(merge::merge(&app, &project, n, &actor, req).await?))
}

/// Close a PR without merging (humans only).
#[utoipa::path(operation_id = "pulls_close", post, path = "/api/projects/{p}/pulls/{n}/close", tag = "pulls", params(("p" = String, Path), ("n" = i64, Path)), responses((status = 200, body = PullDetail)))]
pub async fn close(State(app): State<AppState>, Human(actor): Human, Path((p, n)): Path<(String, i64)>) -> ApiResult<Json<PullDetail>> {
    let project = services::project(&app.db, &p).await?;
    pulls::close(&app, &project, n, &actor).await?;
    Ok(Json(detail(&app, &p, n).await?))
}
