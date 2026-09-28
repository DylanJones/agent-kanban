//! Review verdicts. Recording a verdict is atomic with the issue transition it implies.

use serde::Deserialize;
use serde_json::json;
use utoipa::ToSchema;

use super::{comments, ensure_pr_access, record_event};
use crate::AppState;
use crate::db::{self, begin_write};
use crate::domain::models::{Project, Review};
use crate::domain::{Actor, IssueState, Role};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct NewReview {
    /// `approve` → ready to merge; `changes_requested` → back to the fix agent;
    /// `needs_decision` → paused for a human; `comment` → no state change.
    pub verdict: String,
    /// Review summary (markdown). For `needs_decision`: the question, options and consequences.
    #[serde(default)]
    pub body: String,
    /// The PR head SHA you reviewed. Must equal the current head.
    pub commit_sha: String,
}

pub async fn list(app: &AppState, pr_id: i64) -> sqlx::Result<Vec<Review>> {
    sqlx::query_as::<_, Review>("SELECT * FROM reviews WHERE pr_id = ? ORDER BY created_at, id").bind(pr_id).fetch_all(&app.db).await
}

pub async fn submit(app: &AppState, project: &Project, number: i64, actor: &Actor, req: NewReview) -> ApiResult<Review> {
    let pr = super::pull(&app.db, project.id, number).await?;
    ensure_pr_access(&app.db, actor, &pr).await?;
    if let Actor::Agent { role, .. } = actor
        && *role != Role::Review
        && req.verdict != "comment"
    {
        return Err(ApiError::forbidden("only review runs may record a verdict"));
    }
    let verdict = req.verdict.as_str();
    if !matches!(verdict, "approve" | "changes_requested" | "needs_decision" | "comment") {
        return Err(ApiError::bad("verdict must be approve, changes_requested, needs_decision or comment"));
    }
    if pr.state != "open" {
        return Err(ApiError::conflict(format!("PR #{number} is {}", pr.state)));
    }
    let pr = super::pulls::refresh_head(app, project, &pr).await?;
    let head = pr.head_sha.clone().unwrap_or_default();
    let short = |s: &str| s.chars().take(10).collect::<String>();
    if req.commit_sha.len() < 7 || !head.starts_with(req.commit_sha.trim()) {
        return Err(ApiError::conflict(format!(
            "stale review: you reviewed `{}` but the PR head is now `{}`. Re-review the changes since your commit (GET …/diff?since={}) and resubmit.",
            short(&req.commit_sha),
            short(&head),
            req.commit_sha.trim()
        )));
    }
    let blocking = super::pulls::unresolved_blocking(&app.db, pr.id).await?;
    if verdict == "approve" && blocking > 0 && !actor.is_human() {
        return Err(ApiError::conflict(format!(
            "{blocking} blocking thread(s) are unresolved; resolve them (POST /api/threads/{{id}}/resolve) or request changes"
        )));
    }
    if verdict == "needs_decision" && req.body.trim().is_empty() {
        return Err(ApiError::bad("needs_decision requires a body with the question, options and consequences"));
    }

    let issue_ids = super::pr_issue_ids(&app.db, pr.id).await?;
    let mut issues = Vec::new();
    for id in &issue_ids {
        issues.push(super::issue_by_id(&app.db, *id).await?);
    }
    let target = match verdict {
        "approve" => Some(IssueState::ReadyToMerge),
        "changes_requested" => Some(IssueState::ChangesRequested),
        _ => None,
    };
    if let Some(_t) = target
        && !actor.is_human()
        && let Some(i) = issues.iter().find(|i| i.state != IssueState::InReview)
    {
        return Err(ApiError::conflict(format!(
            "issue #{} is `{}`, not `in_review`; a verdict can only be recorded for work in review",
            i.number,
            i.state.as_str()
        )));
    }

    let title = match verdict {
        "approve" => "✅ **Review verdict: ready to merge**",
        "changes_requested" => "🔁 **Review verdict: changes requested**",
        "needs_decision" => "❓ **Review verdict: needs human decision**",
        _ => "💬 **Review comment**",
    };
    let summary = if req.body.trim().is_empty() {
        format!("{title} at `{}`", short(&head))
    } else {
        format!("{title} at `{}`\n\n{}", short(&head), req.body.trim())
    };

    let mut tx = begin_write(&app.db).await?;
    let now = db::now();
    let rid: i64 = sqlx::query_scalar(
        "INSERT INTO reviews(pr_id, verdict, body, commit_sha, author_kind, author_name, run_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(pr.id)
    .bind(verdict)
    .bind(req.body.trim())
    .bind(&head)
    .bind(actor.kind())
    .bind(actor.name())
    .bind(actor.run_id())
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;
    comments::insert(&mut tx, project.id, comments::Target::Pr(pr.id), actor, "review_summary", &summary).await?;
    sqlx::query("UPDATE pull_requests SET last_reviewed_sha = ?, approved_sha = CASE WHEN ? = 'approve' THEN ? ELSE approved_sha END, updated_at = ? WHERE id = ?")
        .bind(&head)
        .bind(verdict)
        .bind(&head)
        .bind(&now)
        .bind(pr.id)
        .execute(&mut *tx)
        .await?;
    if verdict == "changes_requested" {
        sqlx::query("UPDATE pull_requests SET approved_sha = NULL WHERE id = ?").bind(pr.id).execute(&mut *tx).await?;
    }
    if verdict == "needs_decision" {
        for i in &issues {
            sqlx::query("UPDATE issues SET hold = 'needs_decision', hold_reason = ?, hold_set_at = ?, updated_at = ? WHERE id = ?")
                .bind(req.body.lines().next().unwrap_or("review needs a decision"))
                .bind(&now)
                .bind(&now)
                .bind(i.id)
                .execute(&mut *tx)
                .await?;
            comments::insert(
                &mut tx,
                project.id,
                comments::Target::Issue(i.id),
                actor,
                "decision_request",
                &format!("From review of PR #{number}:\n\n{}", req.body.trim()),
            )
            .await?;
        }
    }
    record_event(&mut tx, Some(project.id), None, Some(pr.id), actor, "pr.review", json!({"verdict": verdict, "commit_sha": head})).await?;
    tx.commit().await?;

    if let Some(t) = target {
        for i in &issues {
            if i.state != t {
                super::issues::set_state(app, project, i, t, actor, None, None).await?;
            }
        }
    }
    app.bus.pr(&project.slug, number);
    for i in &issues {
        app.bus.issue(&project.slug, i.number);
    }
    crate::github::mirror::on_review(app, project, pr.id, &summary).await;
    sqlx::query_as::<_, Review>("SELECT * FROM reviews WHERE id = ?").bind(rid).fetch_one(&app.db).await.map_err(Into::into)
}
