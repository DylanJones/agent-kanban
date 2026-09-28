//! Domain services: every mutation goes through here so rules, audit events and bus
//! notifications are applied consistently regardless of caller (API, scheduler, importer).

pub mod attachments;
pub mod cleanup;
pub mod comments;
pub mod issues;
pub mod labels;
pub mod merge;
pub mod pulls;
pub mod reviews;

use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::{self, Db};
use crate::domain::Actor;
use crate::domain::models::{Issue, Project, PullRequest};
use crate::error::{ApiError, ApiResult};

pub async fn record_event(
    conn: &mut SqliteConnection,
    project_id: Option<i64>,
    issue_id: Option<i64>,
    pr_id: Option<i64>,
    actor: &Actor,
    kind: &str,
    data: Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO events(project_id, issue_id, pr_id, run_id, actor_kind, actor_name, type, data, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(project_id)
    .bind(issue_id)
    .bind(pr_id)
    .bind(actor.run_id())
    .bind(actor.kind())
    .bind(actor.name())
    .bind(kind)
    .bind(data.to_string())
    .bind(db::now())
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn project(db: &Db, slug: &str) -> ApiResult<Project> {
    sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE slug = ?")
        .bind(slug)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("project `{slug}`")))
}

pub async fn project_by_id(db: &Db, id: i64) -> ApiResult<Project> {
    sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE id = ?").bind(id).fetch_one(db).await.map_err(Into::into)
}

pub async fn issue(db: &Db, project_id: i64, number: i64) -> ApiResult<Issue> {
    sqlx::query_as::<_, Issue>("SELECT * FROM issues WHERE project_id = ? AND number = ?")
        .bind(project_id)
        .bind(number)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("issue #{number}")))
}

pub async fn issue_by_id(db: &Db, id: i64) -> ApiResult<Issue> {
    sqlx::query_as::<_, Issue>("SELECT * FROM issues WHERE id = ?").bind(id).fetch_one(db).await.map_err(Into::into)
}

pub async fn pull(db: &Db, project_id: i64, number: i64) -> ApiResult<PullRequest> {
    sqlx::query_as::<_, PullRequest>("SELECT * FROM pull_requests WHERE project_id = ? AND number = ?")
        .bind(project_id)
        .bind(number)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("pull request #{number}")))
}

pub async fn pull_by_id(db: &Db, id: i64) -> ApiResult<PullRequest> {
    sqlx::query_as::<_, PullRequest>("SELECT * FROM pull_requests WHERE id = ?").bind(id).fetch_one(db).await.map_err(Into::into)
}

/// Agents may only act inside their own project.
pub fn ensure_project_access(actor: &Actor, project: &Project) -> ApiResult<()> {
    if let Actor::Agent { project_id, .. } = actor
        && *project_id != project.id
    {
        return Err(ApiError::forbidden("this run token is scoped to a different project"));
    }
    Ok(())
}

/// Agents may only modify the issue their run is bound to.
pub fn ensure_issue_access(actor: &Actor, issue: &Issue) -> ApiResult<()> {
    if let Actor::Agent { issue_id, project_id, .. } = actor
        && (*project_id != issue.project_id || *issue_id != Some(issue.id))
    {
        return Err(ApiError::forbidden(format!(
            "this run token may only modify its own issue; to report a separate problem, create a new issue instead of editing #{}",
            issue.number
        )));
    }
    Ok(())
}

/// Issues linked to a PR.
pub async fn pr_issue_ids(db: &Db, pr_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT issue_id FROM pull_request_issues WHERE pr_id = ?").bind(pr_id).fetch_all(db).await
}

/// Agents may act on a PR if it is linked to their bound issue.
pub async fn ensure_pr_access(db: &Db, actor: &Actor, pr: &PullRequest) -> ApiResult<()> {
    if let Actor::Agent { issue_id, project_id, .. } = actor {
        if *project_id != pr.project_id {
            return Err(ApiError::forbidden("this run token is scoped to a different project"));
        }
        let linked = pr_issue_ids(db, pr.id).await?;
        if !issue_id.is_some_and(|i| linked.contains(&i)) {
            return Err(ApiError::forbidden(format!("PR #{} is not linked to this run's issue", pr.number)));
        }
    }
    Ok(())
}
