//! Local pull requests: a branch proposed for merge into the project's base branch.

use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::{comments, ensure_pr_access, ensure_project_access, record_event};
use crate::AppState;
use crate::db::{self, Db, begin_write};
use crate::domain::models::{Project, PullRequest, ReviewThread};
use crate::domain::{Actor, IssueState};
use crate::error::{ApiError, ApiResult};
use crate::git;

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct NewPull {
    /// PR title. With squash merges this becomes the commit subject, so follow the
    /// project's commit convention (emojicode: start with an emoji).
    pub title: String,
    /// Markdown description: what changed, test results, dependencies.
    #[serde(default)]
    pub body: String,
    /// Issue numbers this PR resolves. Agents: defaults to the run's issue.
    #[serde(default)]
    pub issues: Vec<i64>,
    /// Branch to merge. Agents: defaults to the run's worktree branch.
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PullPatch {
    pub title: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct NewThread {
    /// File path relative to the repo root (must be part of the PR diff).
    pub path: String,
    /// Line number in the file (new side for `RIGHT`, old side for `LEFT`).
    pub line: i64,
    /// First line of a multi-line range.
    pub start_line: Option<i64>,
    /// `RIGHT` (default) or `LEFT`.
    #[serde(default = "right")]
    pub side: String,
    /// Markdown body of the first comment.
    pub body: String,
    /// `blocking` (default) or `nit`.
    #[serde(default = "blocking")]
    pub severity: String,
}

fn right() -> String {
    "RIGHT".into()
}
fn blocking() -> String {
    "blocking".into()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ThreadWithComments {
    #[serde(flatten)]
    pub thread: ReviewThread,
    pub comments: Vec<crate::domain::models::Comment>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Mergeability {
    pub mergeable: bool,
    /// Reasons merge is blocked right now.
    pub blockers: Vec<String>,
    pub has_conflicts: bool,
    pub conflict_files: Vec<String>,
    pub head_sha: Option<String>,
    pub approved_sha: Option<String>,
    pub base_sha: Option<String>,
    pub commits_ahead: u64,
    pub unresolved_blocking_threads: i64,
}

pub async fn open_pr_for_issue(db: &Db, issue_id: i64) -> sqlx::Result<Option<PullRequest>> {
    sqlx::query_as::<_, PullRequest>(
        "SELECT p.* FROM pull_requests p JOIN pull_request_issues pi ON pi.pr_id = p.id
          WHERE pi.issue_id = ? AND p.state = 'open' ORDER BY p.id DESC LIMIT 1",
    )
    .bind(issue_id)
    .fetch_optional(db)
    .await
}

pub async fn prs_for_issue(db: &Db, issue_id: i64) -> sqlx::Result<Vec<PullRequest>> {
    sqlx::query_as::<_, PullRequest>(
        "SELECT p.* FROM pull_requests p JOIN pull_request_issues pi ON pi.pr_id = p.id
          WHERE pi.issue_id = ? ORDER BY p.id DESC",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await
}

pub async fn linked_issue_numbers(db: &Db, pr_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT i.number FROM issues i JOIN pull_request_issues pi ON pi.issue_id = i.id WHERE pi.pr_id = ? ORDER BY i.number",
    )
    .bind(pr_id)
    .fetch_all(db)
    .await
}

pub async fn create(app: &AppState, project: &Project, actor: &Actor, req: NewPull) -> ApiResult<PullRequest> {
    ensure_project_access(actor, project)?;
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::bad("title is required"));
    }
    let mut issues = req.issues.clone();
    let mut branch = req.branch.clone();
    if let Actor::Agent { issue_id, run_id, .. } = actor {
        let bound = match issue_id {
            Some(id) => super::issue_by_id(&app.db, *id).await?,
            None => return Err(ApiError::forbidden("this run is not bound to an issue")),
        };
        if issues.is_empty() {
            issues.push(bound.number);
        }
        if !issues.contains(&bound.number) {
            return Err(ApiError::forbidden(format!("a PR from this run must include its issue #{}", bound.number)));
        }
        if branch.is_none() {
            let wt: Option<String> =
                sqlx::query_scalar("SELECT worktree_path FROM agent_runs WHERE id = ?").bind(run_id).fetch_one(&app.db).await?;
            if let Some(wt) = wt {
                branch = git::run(&wt, &["symbolic-ref", "--short", "HEAD"]).await.ok();
            }
            branch = branch.or(bound.branch_name.clone());
        }
    }
    let branch = branch.ok_or_else(|| ApiError::bad("`branch` is required"))?;
    let head = git::branch_sha(&project.repo_path, &branch)
        .await
        .ok_or_else(|| ApiError::bad(format!("branch `{branch}` does not exist in {}", project.repo_path)))?;
    let ahead = git::commits_ahead(&project.repo_path, &project.base_branch, &head).await.unwrap_or(0);
    if ahead == 0 {
        return Err(ApiError::conflict(format!("branch `{branch}` has no commits ahead of `{}`", project.base_branch)));
    }
    if let (Some(re), "squash") = (&project.commit_msg_regex, project.merge_strategy.as_str())
        && let Ok(re) = regex::Regex::new(re)
        && !re.is_match(&title)
    {
        return Err(ApiError::bad(format!(
            "PR title becomes the squash commit message and must match `{}` (project commit convention)",
            re.as_str()
        )));
    }
    if let Some(existing) =
        sqlx::query_scalar::<_, i64>("SELECT number FROM pull_requests WHERE project_id = ? AND branch = ? AND state = 'open'")
            .bind(project.id)
            .bind(&branch)
            .fetch_optional(&app.db)
            .await?
    {
        return Err(ApiError::conflict(format!("PR #{existing} is already open for branch `{branch}`; push commits to it instead")));
    }
    let merge_base = git::merge_base(&project.repo_path, &project.base_branch, &head).await;

    let mut tx = begin_write(&app.db).await?;
    let mut issue_ids = Vec::new();
    for n in &issues {
        let id: i64 = sqlx::query_scalar("SELECT id FROM issues WHERE project_id = ? AND number = ?")
            .bind(project.id)
            .bind(n)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::bad(format!("issue #{n} not found")))?;
        issue_ids.push(id);
    }
    let number: i64 = sqlx::query_scalar("UPDATE projects SET next_number = next_number + 1 WHERE id = ? RETURNING next_number - 1")
        .bind(project.id)
        .fetch_one(&mut *tx)
        .await?;
    let now = db::now();
    let pr_id: i64 = sqlx::query_scalar(
        "INSERT INTO pull_requests(project_id, number, title, body, state, branch, base_branch, head_sha, merge_base_sha,
                                   author_kind, author_name, created_by_run_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'open', ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(project.id)
    .bind(number)
    .bind(&title)
    .bind(req.body.trim())
    .bind(&branch)
    .bind(&project.base_branch)
    .bind(&head)
    .bind(&merge_base)
    .bind(actor.kind())
    .bind(actor.name())
    .bind(actor.run_id())
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;
    for id in &issue_ids {
        sqlx::query("INSERT OR IGNORE INTO pull_request_issues(pr_id, issue_id) VALUES (?, ?)")
            .bind(pr_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE issues SET branch_name = COALESCE(branch_name, ?), updated_at = ? WHERE id = ?")
            .bind(&branch)
            .bind(&now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        comments::insert(
            &mut tx,
            project.id,
            comments::Target::Issue(*id),
            &Actor::System,
            "system",
            &format!("🔀 PR #{number} opened: {title}"),
        )
        .await?;
    }
    record_event(&mut tx, Some(project.id), None, Some(pr_id), actor, "pr.created", json!({"number": number, "branch": branch})).await?;
    tx.commit().await?;
    app.bus.emit("pr.created", Some(&project.slug), None, Some(number), None);
    for n in &issues {
        app.bus.issue(&project.slug, *n);
    }
    crate::github::mirror::on_pr_created(app, project, pr_id).await;
    super::pull(&app.db, project.id, number).await
}

pub async fn update(app: &AppState, project: &Project, number: i64, actor: &Actor, patch: PullPatch) -> ApiResult<PullRequest> {
    let pr = super::pull(&app.db, project.id, number).await?;
    ensure_pr_access(&app.db, actor, &pr).await?;
    if let Some(t) = &patch.title {
        sqlx::query("UPDATE pull_requests SET title = ?, updated_at = ? WHERE id = ?")
            .bind(t.trim())
            .bind(db::now())
            .bind(pr.id)
            .execute(&app.db)
            .await?;
    }
    if let Some(b) = &patch.body {
        sqlx::query("UPDATE pull_requests SET body = ?, updated_at = ? WHERE id = ?")
            .bind(b)
            .bind(db::now())
            .bind(pr.id)
            .execute(&app.db)
            .await?;
    }
    app.bus.pr(&project.slug, number);
    super::pull(&app.db, project.id, number).await
}

/// Re-read the branch head; if it moved, apply head-change effects.
pub async fn refresh_head(app: &AppState, project: &Project, pr: &PullRequest) -> ApiResult<PullRequest> {
    if pr.state != "open" {
        return Ok(pr.clone());
    }
    let Some(new) = git::branch_sha(&project.repo_path, &pr.branch).await else {
        return Ok(pr.clone());
    };
    if pr.head_sha.as_deref() != Some(new.as_str()) {
        on_head_change(app, project, pr, &new).await?;
    }
    super::pull_by_id(&app.db, pr.id).await
}

/// New commits on the PR branch: update SHAs, re-anchor threads, and send approved PRs back to review.
pub async fn on_head_change(app: &AppState, project: &Project, pr: &PullRequest, new_head: &str) -> ApiResult<()> {
    let repo = &project.repo_path;
    let merge_base = git::merge_base(repo, &project.base_branch, new_head).await;
    let old_head = pr.head_sha.clone();

    // Re-anchor open RIGHT-side threads.
    let threads = sqlx::query_as::<_, ReviewThread>(
        "SELECT * FROM review_threads WHERE pr_id = ? AND outdated = 0 AND side = 'RIGHT' AND line IS NOT NULL",
    )
    .bind(pr.id)
    .fetch_all(&app.db)
    .await?;
    let mut updates = Vec::new();
    for t in threads {
        let Some(from) = t.commit_sha.clone().or(old_head.clone()) else { continue };
        if from == new_head {
            continue;
        }
        let res = git::linemap::remap(repo, &from, new_head, &t.path, t.line.unwrap_or(0)).await;
        updates.push((t.id, res.ok()));
    }

    let mut tx = begin_write(&app.db).await?;
    sqlx::query("UPDATE pull_requests SET head_sha = ?, merge_base_sha = ?, updated_at = ? WHERE id = ?")
        .bind(new_head)
        .bind(&merge_base)
        .bind(db::now())
        .bind(pr.id)
        .execute(&mut *tx)
        .await?;
    for (id, res) in updates {
        match res {
            Some(git::linemap::Remap::Moved(l)) => {
                sqlx::query("UPDATE review_threads SET line = ?, start_line = CASE WHEN start_line IS NULL THEN NULL ELSE ? - (line - start_line) END, commit_sha = ? WHERE id = ?")
                    .bind(l)
                    .bind(l)
                    .bind(new_head)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            Some(_) => {
                sqlx::query("UPDATE review_threads SET outdated = 1 WHERE id = ?").bind(id).execute(&mut *tx).await?;
            }
            None => {}
        }
    }
    record_event(
        &mut tx,
        Some(project.id),
        None,
        Some(pr.id),
        &Actor::System,
        "pr.head_changed",
        json!({"from": old_head, "to": new_head}),
    )
    .await?;
    tx.commit().await?;
    app.bus.pr(&project.slug, pr.number);

    // "A new code commit after a ready-to-merge verdict returns the issue to In review."
    for iid in super::pr_issue_ids(&app.db, pr.id).await? {
        let issue = super::issue_by_id(&app.db, iid).await?;
        if issue.state == IssueState::ReadyToMerge {
            super::issues::set_state(
                app,
                project,
                &issue,
                IssueState::InReview,
                &Actor::System,
                Some(&format!("New commits after approval (head now `{}`); returning to review.", &new_head[..new_head.len().min(10)])),
                None,
            )
            .await?;
        }
    }
    crate::github::mirror::on_branch_updated(app, project, pr.id).await;
    Ok(())
}

pub async fn close(app: &AppState, project: &Project, number: i64, actor: &Actor) -> ApiResult<PullRequest> {
    let pr = super::pull(&app.db, project.id, number).await?;
    if pr.state != "open" {
        return Err(ApiError::conflict(format!("PR #{number} is already {}", pr.state)));
    }
    let mut tx = begin_write(&app.db).await?;
    sqlx::query("UPDATE pull_requests SET state = 'closed', updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(pr.id)
        .execute(&mut *tx)
        .await?;
    record_event(&mut tx, Some(project.id), None, Some(pr.id), actor, "pr.closed", json!({})).await?;
    tx.commit().await?;
    app.bus.pr(&project.slug, number);
    super::pull(&app.db, project.id, number).await
}

pub async fn diff(_app: &AppState, project: &Project, pr: &PullRequest, since: Option<&str>) -> ApiResult<Vec<git::diff::FileDiff>> {
    let head = match pr.state.as_str() {
        "merged" => pr.head_sha.clone(),
        _ => git::branch_sha(&project.repo_path, &pr.branch).await.or(pr.head_sha.clone()),
    }
    .ok_or_else(|| ApiError::conflict("PR has no head commit"))?;
    let from = match since {
        Some(s) => s.to_string(),
        None => pr
            .merge_base_sha
            .clone()
            .or(git::merge_base(&project.repo_path, &pr.base_branch, &head).await)
            .unwrap_or_else(|| pr.base_branch.clone()),
    };
    git::diff::diff(&project.repo_path, &from, &head, 3).await.map_err(|e| ApiError::conflict(format!("{e:#}")))
}

pub async fn threads(app: &AppState, pr_id: i64, resolved: Option<bool>) -> ApiResult<Vec<ThreadWithComments>> {
    let rows = sqlx::query_as::<_, ReviewThread>(
        "SELECT * FROM review_threads WHERE pr_id = ? AND (? IS NULL OR resolved = ?) ORDER BY path, line, id",
    )
    .bind(pr_id)
    .bind(resolved)
    .bind(resolved)
    .fetch_all(&app.db)
    .await?;
    let mut out = Vec::new();
    for t in rows {
        let comments = comments::for_thread(app, t.id).await?;
        out.push(ThreadWithComments { thread: t, comments });
    }
    Ok(out)
}

pub async fn create_thread(app: &AppState, project: &Project, number: i64, actor: &Actor, req: NewThread) -> ApiResult<ThreadWithComments> {
    let pr = super::pull(&app.db, project.id, number).await?;
    ensure_pr_access(&app.db, actor, &pr).await?;
    let pr = refresh_head(app, project, &pr).await?;
    if req.body.trim().is_empty() {
        return Err(ApiError::bad("body is required"));
    }
    let side = req.side.to_uppercase();
    if side != "RIGHT" && side != "LEFT" {
        return Err(ApiError::bad("side must be RIGHT or LEFT"));
    }
    let severity = req.severity.to_lowercase();
    if severity != "blocking" && severity != "nit" {
        return Err(ApiError::bad("severity must be blocking or nit"));
    }
    let files = diff(app, project, &pr, None).await?;
    let Some(file) = files.iter().find(|f| f.path == req.path || f.old_path.as_deref() == Some(&req.path)) else {
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        return Err(ApiError::bad(format!("`{}` is not changed in this PR. Changed files: {}", req.path, names.join(", "))));
    };
    let commit = if side == "RIGHT" { pr.head_sha.clone() } else { pr.merge_base_sha.clone() };
    let hunk = extract_hunk(&file.patch, req.line, &side);
    let mut tx = begin_write(&app.db).await?;
    let tid: i64 = sqlx::query_scalar(
        "INSERT INTO review_threads(pr_id, path, line, start_line, side, commit_sha, original_line, original_commit_sha, diff_hunk,
                                    severity, created_by_run_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(pr.id)
    .bind(&file.path)
    .bind(req.line)
    .bind(req.start_line)
    .bind(&side)
    .bind(&commit)
    .bind(req.line)
    .bind(&commit)
    .bind(hunk)
    .bind(&severity)
    .bind(actor.run_id())
    .bind(db::now())
    .fetch_one(&mut *tx)
    .await?;
    comments::insert(&mut tx, project.id, comments::Target::Thread(tid), actor, "comment", req.body.trim()).await?;
    record_event(
        &mut tx,
        Some(project.id),
        None,
        Some(pr.id),
        actor,
        "thread.created",
        json!({"thread_id": tid, "path": file.path, "line": req.line}),
    )
    .await?;
    tx.commit().await?;
    app.bus.pr(&project.slug, number);
    thread(app, tid).await
}

/// The hunk text (header + lines up to the anchored line) for context in thread views.
fn extract_hunk(patch: &str, line: i64, side: &str) -> Option<String> {
    let mut cur: Vec<&str> = Vec::new();
    let (mut old, mut new) = (0i64, 0i64);
    for l in patch.lines() {
        if l.starts_with("@@") {
            let h = git::linemap::parse_hunks(l);
            if let Some(h) = h.first() {
                old = h.old_start;
                new = h.new_start;
            }
            cur = vec![l];
            continue;
        }
        if cur.is_empty() {
            continue;
        }
        cur.push(l);
        let (o, n) = (old, new);
        match l.chars().next() {
            Some('+') => new += 1,
            Some('-') => old += 1,
            _ => {
                old += 1;
                new += 1
            }
        }
        let hit = if side == "RIGHT" { !l.starts_with('-') && n == line } else { !l.starts_with('+') && o == line };
        if hit {
            let start = cur.len().saturating_sub(8).max(1);
            let mut out = vec![cur[0]];
            out.extend_from_slice(&cur[start..]);
            return Some(out.join("\n"));
        }
    }
    None
}

pub async fn thread(app: &AppState, id: i64) -> ApiResult<ThreadWithComments> {
    let t = sqlx::query_as::<_, ReviewThread>("SELECT * FROM review_threads WHERE id = ?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("thread {id}")))?;
    let comments = comments::for_thread(app, id).await?;
    Ok(ThreadWithComments { thread: t, comments })
}

pub async fn reply(app: &AppState, thread_id: i64, actor: &Actor, body: &str) -> ApiResult<ThreadWithComments> {
    let t = thread(app, thread_id).await?;
    let pr = super::pull_by_id(&app.db, t.thread.pr_id).await?;
    ensure_pr_access(&app.db, actor, &pr).await?;
    comments::add(app, pr.project_id, comments::Target::Thread(thread_id), actor, body).await?;
    let project = super::project_by_id(&app.db, pr.project_id).await?;
    app.bus.pr(&project.slug, pr.number);
    thread(app, thread_id).await
}

pub async fn set_resolved(
    app: &AppState,
    thread_id: i64,
    actor: &Actor,
    resolved: bool,
    note: Option<&str>,
) -> ApiResult<ThreadWithComments> {
    let t = thread(app, thread_id).await?;
    let pr = super::pull_by_id(&app.db, t.thread.pr_id).await?;
    ensure_pr_access(&app.db, actor, &pr).await?;
    let mut tx = begin_write(&app.db).await?;
    sqlx::query("UPDATE review_threads SET resolved = ?, resolved_by = ?, resolved_at = ? WHERE id = ?")
        .bind(resolved)
        .bind(resolved.then(|| actor.name()))
        .bind(resolved.then(db::now))
        .bind(thread_id)
        .execute(&mut *tx)
        .await?;
    if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
        comments::insert(&mut tx, pr.project_id, comments::Target::Thread(thread_id), actor, "comment", n.trim()).await?;
    }
    record_event(
        &mut tx,
        Some(pr.project_id),
        None,
        Some(pr.id),
        actor,
        if resolved { "thread.resolved" } else { "thread.unresolved" },
        json!({"thread_id": thread_id}),
    )
    .await?;
    tx.commit().await?;
    let project = super::project_by_id(&app.db, pr.project_id).await?;
    app.bus.pr(&project.slug, pr.number);
    thread(app, thread_id).await
}

pub async fn unresolved_blocking(db: &Db, pr_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM review_threads WHERE pr_id = ? AND resolved = 0 AND severity = 'blocking'")
        .bind(pr_id)
        .fetch_one(db)
        .await
}

pub async fn mergeability(app: &AppState, project: &Project, pr: &PullRequest) -> ApiResult<Mergeability> {
    let pr = refresh_head(app, project, pr).await?;
    let repo = &project.repo_path;
    let base_sha = git::branch_sha(repo, &pr.base_branch).await;
    let mut blockers = Vec::new();
    let (mut has_conflicts, mut conflict_files, mut ahead) = (false, vec![], 0);
    if pr.state != "open" {
        blockers.push(format!("PR is {}", pr.state));
    }
    if let (Some(head), Some(base)) = (&pr.head_sha, &base_sha) {
        ahead = git::commits_ahead(repo, base, head).await.unwrap_or(0);
        if ahead == 0 {
            blockers.push("no commits ahead of base".into());
        }
        match git::merge_tree(repo, base, head).await {
            Ok(m) => {
                has_conflicts = !m.clean;
                conflict_files = m.conflicts;
            }
            Err(e) => blockers.push(format!("merge check failed: {e}")),
        }
    } else {
        blockers.push("branch or base missing".into());
    }
    if has_conflicts {
        blockers.push(format!("merge conflicts in {}", conflict_files.join(", ")));
    }
    let unresolved = unresolved_blocking(&app.db, pr.id).await?;
    if pr.approved_sha.is_none() || pr.approved_sha != pr.head_sha {
        blockers.push("no approving review at the current head".into());
    }
    Ok(Mergeability {
        mergeable: blockers.is_empty(),
        blockers,
        has_conflicts,
        conflict_files,
        head_sha: pr.head_sha.clone(),
        approved_sha: pr.approved_sha.clone(),
        base_sha,
        commits_ahead: ahead,
        unresolved_blocking_threads: unresolved,
    })
}
