use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::domain::models::{Issue, Label, LimitGroup, Project};
use crate::domain::{Actor, Column, Hold, IssueState};
use crate::error::ApiResult;
use crate::services;

#[derive(Debug, Serialize, ToSchema)]
pub struct LabelRef {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CardPr {
    pub number: i64,
    pub state: String,
    pub has_conflicts: bool,
    /// Approved at the current head.
    pub approved: bool,
    pub unresolved_threads: i64,
    pub github_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CardRun {
    pub id: i64,
    pub role: String,
    pub agent: String,
    pub status: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Failure reason or outcome summary.
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Card {
    pub number: i64,
    pub title: String,
    pub state: IssueState,
    pub column: Column,
    pub hold: Option<Hold>,
    pub hold_reason: Option<String>,
    pub priority: Option<String>,
    pub size: Option<String>,
    pub labels: Vec<LabelRef>,
    pub rank: f64,
    pub source: String,
    pub failure_count: i64,
    pub next_attempt_at: Option<String>,
    pub parent: Option<i64>,
    pub comment_count: i64,
    pub pr: Option<CardPr>,
    pub run: Option<CardRun>,
    /// The most recent finished run (shown as "✓ fix done 5m ago").
    pub last_run: Option<CardRun>,
    /// If the agent that would work on this card next is paused by a usage limit, when it resumes.
    pub waiting_on_limit: Option<String>,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BoardColumn {
    pub id: Column,
    pub title: String,
    pub cards: Vec<Card>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Board {
    pub project: Project,
    pub columns: Vec<BoardColumn>,
    pub labels: Vec<Label>,
    pub limit_groups: Vec<LimitGroup>,
    pub active_runs: i64,
    pub max_concurrent_runs: i64,
    pub scheduler_enabled: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct BoardQuery {
    /// Only cards with this label.
    pub label: Option<String>,
    /// Only cards with this state or hold, e.g. `needs_decision`, `merge_conflict`, `ready_to_merge`.
    pub badge: Option<String>,
    /// Only cards with an active run by this agent slug.
    pub agent: Option<String>,
    /// Only cards with an active run.
    pub running: Option<bool>,
    /// Text search in title/body.
    pub q: Option<String>,
    /// Include Done cards closed more than this many days ago (default 14).
    pub done_days: Option<i64>,
}

/// The kanban board: four columns of cards with badges, PR and agent status.
#[utoipa::path(operation_id = "board_board", get, path = "/api/projects/{p}/board", tag = "board",
    params(("p" = String, Path), BoardQuery),
    responses((status = 200, body = Board)))]
pub async fn board(
    State(app): State<AppState>,
    _actor: Actor,
    Path(p): Path<String>,
    Query(q): Query<BoardQuery>,
) -> ApiResult<Json<Board>> {
    let project = services::project(&app.db, &p).await?;
    let done_cutoff = crate::db::fmt_time(chrono::Utc::now() - chrono::Duration::days(q.done_days.unwrap_or(14)));
    let issues = sqlx::query_as::<_, Issue>(
        "SELECT * FROM issues WHERE project_id = ? AND (state NOT IN ('done','closed') OR COALESCE(closed_at, updated_at) >= ?)
          ORDER BY rank, number",
    )
    .bind(project.id)
    .bind(&done_cutoff)
    .fetch_all(&app.db)
    .await?;

    let label_rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT il.issue_id, l.name, l.color FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE l.project_id = ? ORDER BY l.name",
    )
    .bind(project.id)
    .fetch_all(&app.db)
    .await?;
    let mut labels_by: HashMap<i64, Vec<LabelRef>> = HashMap::new();
    for (i, name, color) in label_rows {
        labels_by.entry(i).or_default().push(LabelRef { name, color });
    }

    let runs: Vec<(i64, i64, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT r.issue_id, r.id, r.role, a.slug, r.status, r.started_at FROM agent_runs r
           JOIN agent_definitions a ON a.id = r.agent_definition_id
          WHERE r.project_id = ? AND r.status IN ('queued','preparing','running')",
    )
    .bind(project.id)
    .fetch_all(&app.db)
    .await?;
    let mut run_by: HashMap<i64, CardRun> = HashMap::new();
    for (iid, id, role, agent, status, started_at) in runs {
        run_by.insert(iid, CardRun { id, role, agent, status, started_at, ended_at: None, error: None });
    }
    let last_runs: Vec<(i64, i64, String, String, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT r.issue_id, r.id, r.role, a.slug, r.status, r.started_at, r.ended_at, r.error FROM agent_runs r
           JOIN agent_definitions a ON a.id = r.agent_definition_id
          WHERE r.project_id = ? AND r.id IN (SELECT MAX(id) FROM agent_runs WHERE project_id = ? AND ended_at IS NOT NULL GROUP BY issue_id)",
    )
    .bind(project.id)
    .bind(project.id)
    .fetch_all(&app.db)
    .await?;
    let mut last_by: HashMap<i64, CardRun> = HashMap::new();
    for (iid, id, role, agent, status, started_at, ended_at, error) in last_runs {
        last_by.insert(iid, CardRun { id, role, agent, status, started_at, ended_at, error });
    }

    let prs: Vec<(i64, i64, String, bool, Option<String>, Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT pi.issue_id, p.number, p.state, p.has_conflicts, p.head_sha, p.approved_sha, p.github_url,
                (SELECT COUNT(*) FROM review_threads t WHERE t.pr_id = p.id AND t.resolved = 0)
           FROM pull_request_issues pi JOIN pull_requests p ON p.id = pi.pr_id
          WHERE p.project_id = ? ORDER BY (p.state = 'open'), p.id",
    )
    .bind(project.id)
    .fetch_all(&app.db)
    .await?;
    let mut pr_by: HashMap<i64, CardPr> = HashMap::new();
    for (iid, number, state, has_conflicts, head, approved, github_url, unresolved) in prs {
        // Later rows win: open PRs sort last.
        pr_by.insert(
            iid,
            CardPr {
                number,
                state,
                has_conflicts,
                approved: head.is_some() && head == approved,
                unresolved_threads: unresolved,
                github_url,
            },
        );
    }

    let comment_counts: Vec<(i64, i64)> =
        sqlx::query_as("SELECT issue_id, COUNT(*) FROM comments WHERE project_id = ? AND issue_id IS NOT NULL GROUP BY issue_id")
            .bind(project.id)
            .fetch_all(&app.db)
            .await?;
    let comment_counts: HashMap<i64, i64> = comment_counts.into_iter().collect();
    let parents: HashMap<i64, i64> = issues.iter().map(|i| (i.id, i.number)).collect();

    // Which roles are waiting on a paused limit group.
    let limit_groups = sqlx::query_as::<_, LimitGroup>("SELECT * FROM limit_groups ORDER BY name").fetch_all(&app.db).await?;
    let role_waiting = crate::orchestrator::scheduler::paused_roles(&app, &project).await.unwrap_or_default();

    let like = q.q.as_ref().map(|s| s.to_lowercase());
    let mut cols: Vec<BoardColumn> =
        [(Column::Backlog, "Backlog"), (Column::InProgress, "In progress"), (Column::InReview, "In review"), (Column::Done, "Done")]
            .into_iter()
            .map(|(id, t)| BoardColumn { id, title: t.into(), cards: vec![] })
            .collect();

    for i in issues {
        let labels = labels_by.remove(&i.id).unwrap_or_default();
        let run = run_by.remove(&i.id);
        if let Some(l) = &q.label
            && !labels.iter().any(|x| &x.name == l)
        {
            continue;
        }
        if let Some(b) = &q.badge {
            let hold = i.hold.map(|h| h.as_str());
            if i.state.as_str() != b && hold != Some(b.as_str()) {
                continue;
            }
        }
        if let Some(a) = &q.agent
            && run.as_ref().is_none_or(|r| &r.agent != a)
        {
            continue;
        }
        if q.running == Some(true) && run.is_none() {
            continue;
        }
        if let Some(s) = &like
            && !i.title.to_lowercase().contains(s)
            && !i.body.to_lowercase().contains(s)
            && !i.number.to_string().eq(s.trim_start_matches('#'))
        {
            continue;
        }
        let waiting_on_limit = match (i.hold, i.state.dispatch_role(), &run) {
            (None, Some(role), None) => role_waiting.get(&role).cloned(),
            _ => None,
        };
        let col = i.state.column();
        let card = Card {
            number: i.number,
            title: i.title.clone(),
            state: i.state,
            column: col,
            hold: i.hold,
            hold_reason: i.hold_reason.clone(),
            priority: i.priority.clone(),
            size: i.size.clone(),
            labels,
            rank: i.rank,
            source: i.source.clone(),
            failure_count: i.failure_count,
            next_attempt_at: i.next_attempt_at.clone(),
            parent: i.parent_issue_id.and_then(|p| parents.get(&p).copied()),
            comment_count: comment_counts.get(&i.id).copied().unwrap_or(0),
            pr: pr_by.remove(&i.id),
            last_run: last_by.remove(&i.id),
            run,
            waiting_on_limit,
            updated_at: i.updated_at.clone(),
            closed_at: i.closed_at.clone(),
        };
        if let Some(c) = cols.iter_mut().find(|c| c.id == col) {
            c.cards.push(card);
        }
    }
    // Done: most recently closed first.
    if let Some(done) = cols.iter_mut().find(|c| c.id == Column::Done) {
        done.cards.sort_by(|a, b| b.closed_at.cmp(&a.closed_at).then(b.updated_at.cmp(&a.updated_at)));
    }

    let labels = services::labels::project_labels(&app.db, project.id).await?;
    let active_runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')").fetch_one(&app.db).await?;
    Ok(Json(Board {
        project,
        columns: cols,
        labels,
        limit_groups,
        active_runs,
        max_concurrent_runs: crate::db::get_setting(&app.db, "max_concurrent_runs").await.unwrap_or(3),
        scheduler_enabled: crate::db::get_setting(&app.db, "scheduler_enabled").await.unwrap_or(false),
    }))
}
