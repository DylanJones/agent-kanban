use serde::Deserialize;
use serde_json::json;
use sqlx::SqliteConnection;
use utoipa::ToSchema;

use super::record_event;
use crate::AppState;
use crate::db::{self, begin_write};
use crate::domain::Actor;
use crate::domain::models::Comment;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Copy)]
pub enum Target {
    Issue(i64),
    Pr(i64),
    Thread(i64),
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct NewComment {
    /// Markdown body.
    pub body: String,
}

pub async fn insert(
    conn: &mut SqliteConnection,
    project_id: i64,
    target: Target,
    actor: &Actor,
    kind: &str,
    body: &str,
) -> sqlx::Result<i64> {
    let (issue_id, pr_id, thread_id) = match target {
        Target::Issue(i) => (Some(i), None, None),
        Target::Pr(p) => (None, Some(p), None),
        Target::Thread(t) => (None, None, Some(t)),
    };
    let now = db::now();
    let author_kind = match actor {
        Actor::Human { .. } => "human",
        Actor::Agent { .. } => "agent",
        Actor::System => "system",
    };
    sqlx::query_scalar(
        "INSERT INTO comments(project_id, issue_id, pr_id, thread_id, kind, body, author_kind, author_name, run_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(project_id)
    .bind(issue_id)
    .bind(pr_id)
    .bind(thread_id)
    .bind(kind)
    .bind(body)
    .bind(author_kind)
    .bind(actor.name())
    .bind(actor.run_id())
    .bind(&now)
    .bind(&now)
    .fetch_one(conn)
    .await
}

pub async fn add(app: &AppState, project_id: i64, target: Target, actor: &Actor, body: &str) -> ApiResult<Comment> {
    if body.trim().is_empty() {
        return Err(ApiError::bad("comment body is empty"));
    }
    let mut tx = begin_write(&app.db).await?;
    let id = insert(&mut tx, project_id, target, actor, "comment", body.trim()).await?;
    let (issue_id, pr_id) = match target {
        Target::Issue(i) => (Some(i), None),
        Target::Pr(p) => (None, Some(p)),
        Target::Thread(t) => {
            let pr: i64 = sqlx::query_scalar("SELECT pr_id FROM review_threads WHERE id = ?").bind(t).fetch_one(&mut *tx).await?;
            (None, Some(pr))
        }
    };
    record_event(&mut tx, Some(project_id), issue_id, pr_id, actor, "comment.created", json!({"comment_id": id})).await?;
    tx.commit().await?;
    get(app, id).await
}

pub async fn get(app: &AppState, id: i64) -> ApiResult<Comment> {
    sqlx::query_as::<_, Comment>("SELECT * FROM comments WHERE id = ?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("comment {id}")))
}

pub async fn for_issue(app: &AppState, issue_id: i64) -> sqlx::Result<Vec<Comment>> {
    sqlx::query_as::<_, Comment>("SELECT * FROM comments WHERE issue_id = ? ORDER BY created_at, id")
        .bind(issue_id)
        .fetch_all(&app.db)
        .await
}

pub async fn for_pr(app: &AppState, pr_id: i64) -> sqlx::Result<Vec<Comment>> {
    sqlx::query_as::<_, Comment>("SELECT * FROM comments WHERE pr_id = ? ORDER BY created_at, id").bind(pr_id).fetch_all(&app.db).await
}

pub async fn for_thread(app: &AppState, thread_id: i64) -> sqlx::Result<Vec<Comment>> {
    sqlx::query_as::<_, Comment>("SELECT * FROM comments WHERE thread_id = ? ORDER BY created_at, id")
        .bind(thread_id)
        .fetch_all(&app.db)
        .await
}

/// Edit or delete: only the author (same run or same human) may.
pub async fn ensure_author(actor: &Actor, c: &Comment) -> ApiResult<()> {
    let ok = match actor {
        Actor::Human { .. } => c.author_kind == "human" || c.author_kind == "github",
        Actor::Agent { run_id, .. } => c.run_id == Some(*run_id),
        Actor::System => true,
    };
    if ok { Ok(()) } else { Err(ApiError::forbidden("only the author can edit this comment")) }
}
