//! Issue creation, editing, workflow transitions, holds and human decisions.

use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::{cleanup, comments, ensure_issue_access, ensure_project_access, labels, record_event};
use crate::AppState;
use crate::db::{self, begin_write};
use crate::domain::models::{Issue, Project};
use crate::domain::state::{allowed_transitions, check_transition};
use crate::domain::{Actor, Hold, IssueState};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct NewIssue {
    /// One-line summary.
    pub title: String,
    /// Markdown description: what happened, how to reproduce, where in the code.
    #[serde(default)]
    pub body: String,
    /// Label names; unknown labels are created.
    #[serde(default)]
    pub labels: Vec<String>,
    /// `P0`, `P1` or `P2`.
    pub priority: Option<String>,
    /// `XS`, `S`, `M`, `L` or `XL`.
    pub size: Option<String>,
    /// Number of a parent issue.
    pub parent: Option<i64>,
    /// Initial state (humans only; defaults to `triage`).
    pub state: Option<IssueState>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    /// Replaces the full label set.
    pub labels: Option<Vec<String>>,
    /// Labels to add (keeps existing ones).
    pub add_labels: Option<Vec<String>>,
    /// `P0`/`P1`/`P2`, or empty string to clear.
    pub priority: Option<String>,
    /// `XS`..`XL`, or empty string to clear.
    pub size: Option<String>,
    pub estimate: Option<f64>,
    /// Parent issue number, or 0 to clear.
    pub parent: Option<i64>,
    pub rank: Option<f64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TransitionRequest {
    /// Target state.
    pub to: IssueState,
    /// Optional comment recorded on the issue explaining the move.
    pub comment: Option<String>,
    /// For `closed`: `duplicate`, `invalid`, `wontfix`, `not_planned`...
    pub close_reason: Option<String>,
    /// For duplicates: the issue this one duplicates. Its thread gets a note with this report,
    /// so any new repro details aren't lost. Implies `close_reason: duplicate`.
    #[serde(default)]
    pub duplicate_of: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct DecisionRequest {
    /// The concrete question the human must answer.
    pub question: String,
    /// Candidate answers.
    #[serde(default)]
    pub options: Vec<String>,
    /// What each option implies (compatibility, complexity, semantics...).
    #[serde(default)]
    pub consequences: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct DecisionAnswer {
    /// The decision, recorded as a `decision` comment agents will see as settled.
    pub answer: String,
    /// Optionally move the issue after clearing the hold.
    pub resume_to: Option<IssueState>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HoldRequest {
    pub hold: Hold,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CreatedIssue {
    pub number: i64,
    /// Browser URL of the issue.
    pub url: String,
    /// API URL of the issue.
    pub api_url: String,
    pub state: IssueState,
    /// Open or recently closed issues with similar titles. If one is the same bug, comment there
    /// instead (triage closes true duplicates).
    pub possible_duplicates: Vec<SimilarIssue>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SimilarIssue {
    pub number: i64,
    pub title: String,
    pub state: IssueState,
}

fn title_words(t: &str) -> std::collections::HashSet<String> {
    const STOP: &[&str] = &["with", "when", "that", "from", "into", "this", "does", "doesn", "should", "fails", "error", "compiler"];
    t.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

/// Issues whose titles share enough significant words with `title` (open, or closed in the last 60 days).
pub async fn similar_issues(app: &AppState, project_id: i64, title: &str, exclude: Option<i64>) -> sqlx::Result<Vec<SimilarIssue>> {
    let words = title_words(title);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let cutoff = db::fmt_time(chrono::Utc::now() - chrono::Duration::days(60));
    let rows: Vec<(i64, String, IssueState)> = sqlx::query_as(
        "SELECT number, title, state FROM issues WHERE project_id = ? AND (state NOT IN ('done','closed') OR COALESCE(closed_at, updated_at) >= ?)",
    )
    .bind(project_id)
    .bind(&cutoff)
    .fetch_all(&app.db)
    .await?;
    let mut scored: Vec<(f64, SimilarIssue)> = rows
        .into_iter()
        .filter(|(n, _, _)| Some(*n) != exclude)
        .filter_map(|(number, t, state)| {
            let other = title_words(&t);
            let shared = words.intersection(&other).count();
            let jaccard = shared as f64 / words.union(&other).count().max(1) as f64;
            (shared >= 2 && jaccard >= 0.2).then_some((jaccard, SimilarIssue { number, title: t, state }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(5).map(|(_, s)| s).collect())
}

fn validate_priority(p: &Option<String>) -> ApiResult<Option<String>> {
    match p.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v @ ("P0" | "P1" | "P2")) => Ok(Some(v.to_string())),
        Some(v) => Err(ApiError::bad(format!("priority must be P0, P1 or P2, got `{v}`"))),
    }
}

fn validate_size(s: &Option<String>) -> ApiResult<Option<String>> {
    match s.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v @ ("XS" | "S" | "M" | "L" | "XL")) => Ok(Some(v.to_string())),
        Some(v) => Err(ApiError::bad(format!("size must be XS, S, M, L or XL, got `{v}`"))),
    }
}

fn services_check_project(actor: &Actor, project: &Project) -> ApiResult<()> {
    ensure_project_access(actor, project)
}

pub fn issue_url(app: &AppState, project: &Project, number: i64) -> String {
    format!("{}/p/{}/issues/{}", app.config.public_url, project.slug, number)
}

pub async fn create(app: &AppState, project: &Project, actor: &Actor, new: NewIssue) -> ApiResult<CreatedIssue> {
    ensure_project_access(actor, project)?;
    let title = new.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::bad("title is required"));
    }
    let priority = validate_priority(&new.priority)?;
    let size = validate_size(&new.size)?;
    let state = match (&actor, new.state) {
        (Actor::Human { .. }, Some(s)) => s,
        (_, Some(s)) if s != IssueState::Triage => {
            return Err(ApiError::forbidden("agents file new issues into `triage`"));
        }
        _ => IssueState::Triage,
    };
    let source = match actor {
        Actor::Agent { .. } => "agent",
        _ => "human",
    };
    let mut labels = new.labels.clone();
    if matches!(actor, Actor::Agent { .. }) && !labels.iter().any(|l| l == "found-by-agent") {
        labels.push("found-by-agent".into());
    }

    let mut tx = begin_write(&app.db).await?;
    let parent_id = match new.parent {
        Some(n) => Some(
            sqlx::query_scalar::<_, i64>("SELECT id FROM issues WHERE project_id = ? AND number = ?")
                .bind(project.id)
                .bind(n)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| ApiError::bad(format!("parent issue #{n} not found")))?,
        ),
        None => None,
    };
    let number: i64 = sqlx::query_scalar("UPDATE projects SET next_number = next_number + 1 WHERE id = ? RETURNING next_number - 1")
        .bind(project.id)
        .fetch_one(&mut *tx)
        .await?;
    let rank: f64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(rank), 0) + 1 AS REAL) FROM issues WHERE project_id = ?")
        .bind(project.id)
        .fetch_one(&mut *tx)
        .await?;
    let now = db::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO issues(project_id, number, title, body, state, priority, size, parent_issue_id, rank, source,
                            reported_by_run_id, author_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(project.id)
    .bind(number)
    .bind(&title)
    .bind(new.body.trim())
    .bind(state)
    .bind(&priority)
    .bind(&size)
    .bind(parent_id)
    .bind(rank)
    .bind(source)
    .bind(actor.run_id())
    .bind(actor.name())
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;
    labels::add_issue_labels(&mut tx, project.id, id, &labels).await?;
    record_event(&mut tx, Some(project.id), Some(id), None, actor, "issue.created", json!({"state": state})).await?;
    tx.commit().await?;

    app.bus.emit("issue.created", Some(&project.slug), Some(number), None, None);
    Ok(CreatedIssue {
        number,
        url: issue_url(app, project, number),
        api_url: format!("{}/projects/{}/issues/{}", app.config.api_url(), project.slug, number),
        state,
        possible_duplicates: similar_issues(app, project.id, &title, Some(number)).await.unwrap_or_default(),
    })
}

pub async fn update(app: &AppState, project: &Project, number: i64, actor: &Actor, patch: IssuePatch) -> ApiResult<Issue> {
    let issue = super::issue(&app.db, project.id, number).await?;
    // Agents edit their own issue. Triage agents may also organise other open issues (group them
    // under an umbrella issue, add labels), but not rewrite them.
    if let (Actor::Agent { role: crate::domain::Role::Triage, issue_id, .. }, false) = (actor, ensure_issue_access(actor, &issue).is_ok()) {
        let organising_only = patch.title.is_none()
            && patch.body.is_none()
            && patch.labels.is_none()
            && patch.priority.is_none()
            && patch.size.is_none()
            && patch.estimate.is_none()
            && patch.rank.is_none()
            && patch.start_date.is_none()
            && patch.target_date.is_none();
        if !organising_only || issue.state.is_terminal() || issue_id.is_none() {
            return Err(ApiError::forbidden("triage agents may only set `parent` or `add_labels` on other open issues"));
        }
        services_check_project(actor, project)?;
    } else {
        ensure_issue_access(actor, &issue)?;
    }
    let mut tx = begin_write(&app.db).await?;
    let now = db::now();
    if let Some(t) = &patch.title {
        if t.trim().is_empty() {
            return Err(ApiError::bad("title cannot be empty"));
        }
        sqlx::query("UPDATE issues SET title = ? WHERE id = ?").bind(t.trim()).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(b) = &patch.body {
        sqlx::query("UPDATE issues SET body = ? WHERE id = ?").bind(b).bind(issue.id).execute(&mut *tx).await?;
    }
    if patch.priority.is_some() {
        let p = validate_priority(&patch.priority)?;
        sqlx::query("UPDATE issues SET priority = ? WHERE id = ?").bind(p).bind(issue.id).execute(&mut *tx).await?;
    }
    if patch.size.is_some() {
        let s = validate_size(&patch.size)?;
        sqlx::query("UPDATE issues SET size = ? WHERE id = ?").bind(s).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(e) = patch.estimate {
        sqlx::query("UPDATE issues SET estimate = ? WHERE id = ?").bind(e).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(r) = patch.rank {
        sqlx::query("UPDATE issues SET rank = ? WHERE id = ?").bind(r).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(d) = &patch.start_date {
        sqlx::query("UPDATE issues SET start_date = NULLIF(?, '') WHERE id = ?").bind(d).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(d) = &patch.target_date {
        sqlx::query("UPDATE issues SET target_date = NULLIF(?, '') WHERE id = ?").bind(d).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(p) = patch.parent {
        let pid = if p == 0 {
            None
        } else {
            Some(
                sqlx::query_scalar::<_, i64>("SELECT id FROM issues WHERE project_id = ? AND number = ?")
                    .bind(project.id)
                    .bind(p)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| ApiError::bad(format!("parent issue #{p} not found")))?,
            )
        };
        sqlx::query("UPDATE issues SET parent_issue_id = ? WHERE id = ?").bind(pid).bind(issue.id).execute(&mut *tx).await?;
    }
    if let Some(ls) = &patch.labels {
        labels::set_issue_labels(&mut tx, project.id, issue.id, ls).await?;
    }
    if let Some(ls) = &patch.add_labels {
        labels::add_issue_labels(&mut tx, project.id, issue.id, ls).await?;
    }
    sqlx::query("UPDATE issues SET updated_at = ? WHERE id = ?").bind(&now).bind(issue.id).execute(&mut *tx).await?;
    record_event(&mut tx, Some(project.id), Some(issue.id), None, actor, "issue.edited", json!({})).await?;
    tx.commit().await?;
    app.bus.issue(&project.slug, number);
    super::issue(&app.db, project.id, number).await
}

/// Move an issue to a new workflow state, enforcing the transition table and guards.
pub async fn transition(app: &AppState, project: &Project, number: i64, actor: &Actor, req: TransitionRequest) -> ApiResult<Issue> {
    let issue = super::issue(&app.db, project.id, number).await?;
    ensure_issue_access(actor, &issue)?;
    let from = issue.state;
    let to = req.to;
    if let Err(detail) = check_transition(from, to, actor) {
        return Err(ApiError::InvalidTransition {
            detail,
            allowed: allowed_transitions(from, actor).into_iter().map(|s| s.as_str().to_string()).collect(),
        });
    }

    // Guards that apply to agents (humans can override).
    if let Actor::Agent { .. } = actor {
        if matches!(to, IssueState::ChangesRequested | IssueState::ReadyToMerge) {
            return Err(ApiError::conflict(
                "record a review verdict with POST /api/projects/{p}/pulls/{n}/reviews instead; it moves the issue for you",
            ));
        }
        if to == IssueState::InReview {
            let pr = super::pulls::open_pr_for_issue(&app.db, issue.id)
                .await?
                .ok_or_else(|| ApiError::conflict("open a pull request first: POST /api/projects/{p}/pulls with `issues: [n]`"))?;
            let pr = super::pulls::refresh_head(app, project, &pr).await?;
            if let Some(head) = &pr.head_sha {
                let ahead = crate::git::commits_ahead(&project.repo_path, &project.base_branch, head).await.unwrap_or(1);
                if ahead == 0 {
                    return Err(ApiError::conflict(format!(
                        "branch `{}` has no commits ahead of `{}`; commit your work first",
                        pr.branch, project.base_branch
                    )));
                }
            }
        }
        if to == IssueState::Closed && req.close_reason.is_none() && req.comment.is_none() && req.duplicate_of.is_none() {
            return Err(ApiError::bad("closing requires `close_reason`, `duplicate_of`, or a `comment` explaining why"));
        }
    }

    let mut close_reason = req.close_reason.clone();
    let mut comment = req.comment.clone();
    let original = match req.duplicate_of {
        Some(n) if to == IssueState::Closed => {
            if n == number {
                return Err(ApiError::bad("an issue can't duplicate itself"));
            }
            let orig = super::issue(&app.db, project.id, n).await.map_err(|_| ApiError::bad(format!("duplicate_of: issue #{n} not found")))?;
            close_reason = Some("duplicate".into());
            let note = format!("Duplicate of #{n}.");
            comment = Some(match comment {
                Some(c) if !c.trim().is_empty() => format!("{note} {c}"),
                _ => note,
            });
            Some(orig)
        }
        Some(_) => return Err(ApiError::bad("duplicate_of only applies when moving to `closed`")),
        None => None,
    };
    set_state(app, project, &issue, to, actor, comment.as_deref(), close_reason.as_deref()).await?;
    if let Some(orig) = original {
        // Keep the duplicate's report on the original so new repro details aren't lost.
        let body = format!(
            "#{} ({}) was closed as a duplicate of this issue.\n\n<details><summary>Its report</summary>\n\n{}\n\n</details>",
            issue.number,
            issue.title,
            issue.body.trim()
        );
        let mut tx = begin_write(&app.db).await?;
        comments::insert(&mut tx, project.id, comments::Target::Issue(orig.id), &Actor::System, "system", &body).await?;
        tx.commit().await?;
        app.bus.issue(&project.slug, orig.number);
    }
    super::issue(&app.db, project.id, number).await
}

/// Write a state change (no permission checks) and run side effects.
pub async fn set_state(
    app: &AppState,
    project: &Project,
    issue: &Issue,
    to: IssueState,
    actor: &Actor,
    comment: Option<&str>,
    close_reason: Option<&str>,
) -> ApiResult<()> {
    let from = issue.state;
    let now = db::now();
    let mut tx = begin_write(&app.db).await?;
    // Re-read inside the write lock so concurrent movers don't clobber each other.
    let current: IssueState = sqlx::query_scalar("SELECT state FROM issues WHERE id = ?").bind(issue.id).fetch_one(&mut *tx).await?;
    if current != from {
        return Err(ApiError::conflict(format!("issue #{} changed concurrently (now {}); retry", issue.number, current.as_str())));
    }
    let closed_at = if to.is_terminal() { Some(now.clone()) } else { None };
    sqlx::query(
        "UPDATE issues SET state = ?, updated_at = ?, closed_at = ?, failure_count = 0, next_attempt_at = NULL,
                close_reason = CASE WHEN ? = 'closed' THEN ? ELSE NULL END,
                hold = CASE WHEN ? IN ('done','closed') THEN NULL ELSE hold END
          WHERE id = ?",
    )
    .bind(to)
    .bind(&now)
    .bind(closed_at)
    .bind(to.as_str())
    .bind(close_reason)
    .bind(to.as_str())
    .bind(issue.id)
    .execute(&mut *tx)
    .await?;
    if to == IssueState::InReview {
        // Record which head the reviewer is being asked to look at.
        sqlx::query(
            "UPDATE pull_requests SET review_requested_sha = head_sha
              WHERE state = 'open' AND id IN (SELECT pr_id FROM pull_request_issues WHERE issue_id = ?)",
        )
        .bind(issue.id)
        .execute(&mut *tx)
        .await?;
    }
    if from == IssueState::ReadyToMerge && to != IssueState::Done {
        sqlx::query(
            "UPDATE pull_requests SET approved_sha = NULL
              WHERE state = 'open' AND id IN (SELECT pr_id FROM pull_request_issues WHERE issue_id = ?)",
        )
        .bind(issue.id)
        .execute(&mut *tx)
        .await?;
    }
    record_event(
        &mut tx,
        Some(project.id),
        Some(issue.id),
        None,
        actor,
        "issue.transition",
        json!({"from": from, "to": to, "close_reason": close_reason}),
    )
    .await?;
    if let Some(c) = comment.filter(|c| !c.trim().is_empty()) {
        comments::insert(&mut tx, project.id, comments::Target::Issue(issue.id), actor, "comment", c).await?;
    }
    tx.commit().await?;
    app.bus.issue(&project.slug, issue.number);

    cleanup::on_state_change(app, project, issue, from, to, actor.run_id()).await;
    Ok(())
}

pub async fn request_decision(app: &AppState, project: &Project, number: i64, actor: &Actor, req: DecisionRequest) -> ApiResult<Issue> {
    let issue = super::issue(&app.db, project.id, number).await?;
    ensure_issue_access(actor, &issue)?;
    if req.question.trim().is_empty() {
        return Err(ApiError::bad("question is required"));
    }
    if issue.state.is_terminal() {
        return Err(ApiError::conflict("issue is closed"));
    }
    let mut body = format!("**Decision needed:** {}\n", req.question.trim());
    if !req.options.is_empty() {
        body.push_str("\n**Options:**\n");
        for (i, o) in req.options.iter().enumerate() {
            body.push_str(&format!("{}. {}\n", i + 1, o.trim()));
        }
    }
    if !req.consequences.trim().is_empty() {
        body.push_str(&format!("\n**Consequences:**\n{}\n", req.consequences.trim()));
    }
    let mut tx = begin_write(&app.db).await?;
    sqlx::query("UPDATE issues SET hold = 'needs_decision', hold_reason = ?, hold_set_at = ?, updated_at = ? WHERE id = ?")
        .bind(req.question.trim())
        .bind(db::now())
        .bind(db::now())
        .bind(issue.id)
        .execute(&mut *tx)
        .await?;
    comments::insert(&mut tx, project.id, comments::Target::Issue(issue.id), actor, "decision_request", &body).await?;
    record_event(&mut tx, Some(project.id), Some(issue.id), None, actor, "issue.decision_requested", json!({"question": req.question}))
        .await?;
    tx.commit().await?;
    app.bus.issue(&project.slug, number);
    super::issue(&app.db, project.id, number).await
}

pub async fn answer_decision(app: &AppState, project: &Project, number: i64, actor: &Actor, req: DecisionAnswer) -> ApiResult<Issue> {
    let issue = super::issue(&app.db, project.id, number).await?;
    if req.answer.trim().is_empty() {
        return Err(ApiError::bad("answer is required"));
    }
    let mut tx = begin_write(&app.db).await?;
    sqlx::query(
        "UPDATE issues SET hold = CASE WHEN hold = 'needs_decision' THEN NULL ELSE hold END,
                hold_reason = CASE WHEN hold = 'needs_decision' THEN NULL ELSE hold_reason END,
                failure_count = 0, next_attempt_at = NULL, updated_at = ?
          WHERE id = ?",
    )
    .bind(db::now())
    .bind(issue.id)
    .execute(&mut *tx)
    .await?;
    comments::insert(&mut tx, project.id, comments::Target::Issue(issue.id), actor, "decision", req.answer.trim()).await?;
    record_event(&mut tx, Some(project.id), Some(issue.id), None, actor, "issue.decided", json!({})).await?;
    tx.commit().await?;
    app.bus.issue(&project.slug, number);
    if let Some(to) = req.resume_to {
        let issue = super::issue(&app.db, project.id, number).await?;
        if issue.state != to {
            set_state(app, project, &issue, to, actor, None, None).await?;
        }
    }
    super::issue(&app.db, project.id, number).await
}

pub async fn set_hold(app: &AppState, project: &Project, number: i64, actor: &Actor, hold: Option<HoldRequest>) -> ApiResult<Issue> {
    let issue = super::issue(&app.db, project.id, number).await?;
    let mut tx = begin_write(&app.db).await?;
    match &hold {
        Some(h) => {
            sqlx::query("UPDATE issues SET hold = ?, hold_reason = ?, hold_set_at = ?, updated_at = ? WHERE id = ?")
                .bind(h.hold)
                .bind(&h.reason)
                .bind(db::now())
                .bind(db::now())
                .bind(issue.id)
                .execute(&mut *tx)
                .await?;
        }
        None => {
            sqlx::query(
                "UPDATE issues SET hold = NULL, hold_reason = NULL, hold_set_at = NULL, failure_count = 0,
                        next_attempt_at = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(db::now())
            .bind(issue.id)
            .execute(&mut *tx)
            .await?;
        }
    }
    record_event(
        &mut tx,
        Some(project.id),
        Some(issue.id),
        None,
        actor,
        "issue.hold",
        json!({"hold": hold.as_ref().map(|h| h.hold), "reason": hold.as_ref().and_then(|h| h.reason.clone())}),
    )
    .await?;
    tx.commit().await?;
    app.bus.issue(&project.slug, number);
    if hold.is_some() {
        // A manual pause stops the agent working on it.
        cleanup::cancel_active_runs(app, issue.id, "issue was put on hold").await;
    }
    super::issue(&app.db, project.id, number).await
}

/// System-level hold used by the orchestrator (e.g. stalled after repeated failures).
pub async fn system_hold(app: &AppState, issue_id: i64, hold: Hold, reason: &str) -> ApiResult<()> {
    let issue = super::issue_by_id(&app.db, issue_id).await?;
    let project = super::project_by_id(&app.db, issue.project_id).await?;
    let mut tx = begin_write(&app.db).await?;
    sqlx::query("UPDATE issues SET hold = ?, hold_reason = ?, hold_set_at = ?, updated_at = ? WHERE id = ?")
        .bind(hold)
        .bind(reason)
        .bind(db::now())
        .bind(db::now())
        .bind(issue_id)
        .execute(&mut *tx)
        .await?;
    comments::insert(
        &mut tx,
        project.id,
        comments::Target::Issue(issue_id),
        &Actor::System,
        "system",
        &format!("⏸ {}: {reason}", hold.as_str()),
    )
    .await?;
    record_event(&mut tx, Some(project.id), Some(issue_id), None, &Actor::System, "issue.hold", json!({"hold": hold, "reason": reason}))
        .await?;
    tx.commit().await?;
    app.bus.issue(&project.slug, issue.number);
    Ok(())
}
