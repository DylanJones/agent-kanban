//! Lifecycle of a single agent run: prepare worktree → spawn adapter → ACP session → outcome.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use super::limits::{self, Evidence, LimitSignal};
use super::prompt;
use crate::AppState;
use crate::acp::{self, AcpError, Ctl, Out};
use crate::auth;
use crate::db;
use crate::domain::models::{AgentDefinition, AgentRun, Comment, Issue, Project, PullRequest, Review, RunEvent};
use crate::domain::{Actor, Hold, IssueState, Role};
use crate::git;
use crate::services::{self, comments};

enum Outcome {
    Succeeded(String),
    Failed(String),
    /// Failed in a way retrying won't fix (e.g. branch checked out elsewhere).
    Blocked(String),
    RateLimited(LimitSignal),
    Cancelled(String),
}

struct TextBlock {
    kind: String,
    key: String,
    text: String,
    written: Instant,
    dirty: bool,
}

/// Records transcript events. Streamed text chunks accumulate into a single row per block
/// (updated in place as more arrives), so a message is never split mid-sentence.
struct Transcript {
    app: AppState,
    run_id: i64,
    seq: i64,
    events: tokio::sync::broadcast::Sender<RunEvent>,
    /// The text block currently streaming in; it's written to one row and updated in place.
    pending: Option<TextBlock>,
    blocks: u64,
    stderr_file: Option<tokio::fs::File>,
    stderr_tail: VecDeque<String>,
    pending_stderr: Vec<String>,
    last_message: String,
    rate_limit_meta: Option<Value>,
    /// Usage, model and cost as reported over ACP (fallback when no session log is readable).
    acp_usage: crate::usage::AcpReport,
}

impl Transcript {
    async fn insert(&mut self, kind: &str, key: Option<&str>, payload: Value) {
        self.seq += 1;
        let ev = RunEvent {
            run_id: self.run_id,
            seq: self.seq,
            ts: db::now(),
            kind: kind.into(),
            key: key.map(str::to_string),
            payload: sqlx::types::Json(payload),
        };
        let _ = sqlx::query("INSERT INTO run_events(run_id, seq, ts, kind, key, payload) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(ev.run_id)
            .bind(ev.seq)
            .bind(&ev.ts)
            .bind(&ev.kind)
            .bind(&ev.key)
            .bind(ev.payload.0.to_string())
            .execute(&self.app.db)
            .await;
        let _ = self.events.send(ev);
    }

    /// Insert or merge (by key) — used for tool calls, whose updates patch the original.
    async fn upsert(&mut self, kind: &str, key: &str, patch: Value) {
        let existing: Option<(i64, String, String)> =
            sqlx::query_as("SELECT seq, ts, payload FROM run_events WHERE run_id = ? AND key = ? ORDER BY seq DESC LIMIT 1")
                .bind(self.run_id)
                .bind(key)
                .fetch_optional(&self.app.db)
                .await
                .ok()
                .flatten();
        match existing {
            Some((seq, ts, payload)) => {
                let mut v: Value = serde_json::from_str(&payload).unwrap_or(json!({}));
                if let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) {
                    for (k, val) in p {
                        if !val.is_null() {
                            obj.insert(k.clone(), val.clone());
                        }
                    }
                }
                let _ = sqlx::query("UPDATE run_events SET payload = ? WHERE run_id = ? AND seq = ?")
                    .bind(v.to_string())
                    .bind(self.run_id)
                    .bind(seq)
                    .execute(&self.app.db)
                    .await;
                let _ = self.events.send(RunEvent {
                    run_id: self.run_id,
                    seq,
                    ts,
                    kind: kind.into(),
                    key: Some(key.into()),
                    payload: sqlx::types::Json(v),
                });
            }
            None => self.insert(kind, Some(key), patch).await,
        }
    }

    async fn write_pending(&mut self) {
        let Some(b) = self.pending.as_mut() else { return };
        if !b.dirty || b.text.trim().is_empty() {
            return;
        }
        b.dirty = false;
        b.written = Instant::now();
        let (kind, key, text) = (b.kind.clone(), b.key.clone(), b.text.clone());
        self.upsert(&kind, &key, json!({ "text": text })).await;
    }

    /// Close the current text block and write any buffered stderr.
    async fn flush(&mut self) {
        self.write_pending().await;
        self.pending = None;
        if !self.pending_stderr.is_empty() {
            let text = self.pending_stderr.join("\n");
            self.pending_stderr.clear();
            self.insert("stderr", None, json!({ "text": text })).await;
        }
    }

    /// Periodic write so live viewers see text as it streams, without starting a new block.
    async fn maybe_flush(&mut self) {
        if self.pending.as_ref().is_some_and(|b| b.dirty && b.written.elapsed() > Duration::from_millis(400)) {
            self.write_pending().await;
        }
        if self.pending_stderr.len() > 50 {
            let text = self.pending_stderr.join("\n");
            self.pending_stderr.clear();
            self.insert("stderr", None, json!({ "text": text })).await;
        }
    }

    async fn text_chunk(&mut self, kind: &str, text: &str) {
        match &mut self.pending {
            Some(b) if b.kind == kind => {
                b.text.push_str(text);
                b.dirty = true;
            }
            _ => {
                self.flush().await;
                self.blocks += 1;
                self.pending = Some(TextBlock {
                    kind: kind.to_string(),
                    key: format!("text:{}", self.blocks),
                    text: text.to_string(),
                    written: Instant::now(),
                    dirty: true,
                });
            }
        }
        if kind == "message" {
            self.last_message.push_str(text);
        }
    }

    async fn stderr(&mut self, line: String) {
        if let Some(f) = &mut self.stderr_file {
            let _ = f.write_all(format!("{line}\n").as_bytes()).await;
        }
        self.stderr_tail.push_back(line.clone());
        while self.stderr_tail.len() > 200 {
            self.stderr_tail.pop_front();
        }
        self.pending_stderr.push(line);
    }

    /// Handle one `SessionNotification` (JSON). Returns a rate-limit snapshot if present.
    async fn update(&mut self, n: &Value) -> Option<Value> {
        let u = n.get("update").unwrap_or(n);
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        let text_of = |u: &Value| -> String {
            let c = &u["content"];
            match c.get("type").and_then(Value::as_str) {
                Some("text") => c["text"].as_str().unwrap_or("").to_string(),
                Some(t) => format!("[{t}]"),
                None => String::new(),
            }
        };
        let meta_rl = u
            .get("_meta")
            .and_then(|m| m.get("_claude/rateLimit"))
            .or_else(|| n.get("_meta").and_then(|m| m.get("_claude/rateLimit")))
            .cloned();
        match kind {
            "agent_message_chunk" => self.text_chunk("message", &text_of(u)).await,
            "agent_thought_chunk" => self.text_chunk("thought", &text_of(u)).await,
            "user_message_chunk" => {}
            "tool_call" | "tool_call_update" => {
                self.flush().await;
                let id = u.get("toolCallId").and_then(Value::as_str).unwrap_or("?").to_string();
                let mut p = u.clone();
                if let Some(o) = p.as_object_mut() {
                    o.remove("sessionUpdate");
                    o.remove("_meta");
                }
                self.upsert("tool_call", &format!("tool:{id}"), p).await;
                if kind == "tool_call" {
                    // New tool call: a new message follows.
                    self.last_message.clear();
                }
            }
            "plan" => {
                self.flush().await;
                self.upsert("plan", "plan", json!({ "entries": u.get("entries").cloned().unwrap_or(json!([])) })).await;
            }
            "usage_update" => {
                if let Some(m) = u.pointer("/_meta/_claude~1model").and_then(Value::as_str) {
                    self.acp_usage.model = Some(m.to_string());
                }
                if let Some(c) = u.pointer("/cost/amount").and_then(Value::as_f64) {
                    self.acp_usage.cost_usd = Some(c);
                }
                let mut p = u.clone();
                if let Some(o) = p.as_object_mut() {
                    o.remove("sessionUpdate");
                }
                self.upsert("usage", "usage", p).await;
            }
            "current_mode_update" => {
                self.flush().await;
                self.insert("status", None, json!({ "text": format!("mode: {}", u["currentModeId"].as_str().unwrap_or("?")) })).await;
            }
            _ => {}
        }
        if meta_rl.is_some() {
            self.rate_limit_meta = meta_rl.clone();
        }
        meta_rl
    }

    fn stderr_tail(&self) -> String {
        self.stderr_tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for w in s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        if out.len() + w.len() > 40 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w.to_ascii_lowercase());
    }
    if out.is_empty() { "work".into() } else { out }
}

pub fn branch_for(project: &Project, issue: &Issue) -> String {
    issue.branch_name.clone().unwrap_or_else(|| format!("{}issue-{}-{}", project.branch_prefix, issue.number, slugify(&issue.title)))
}

async fn record_worktree(app: &AppState, project: &Project, issue_id: i64, path: &Path, branch: Option<&str>, kind: &str) {
    let p = path.to_string_lossy().to_string();
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worktrees WHERE path = ? AND removed_at IS NULL)")
        .bind(&p)
        .fetch_one(&app.db)
        .await
        .unwrap_or(false);
    if !exists {
        let _ = sqlx::query("INSERT INTO worktrees(project_id, issue_id, path, branch, kind, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(project.id)
            .bind(issue_id)
            .bind(&p)
            .bind(branch)
            .bind(kind)
            .bind(db::now())
            .execute(&app.db)
            .await;
    }
}

struct Prepared {
    worktree: PathBuf,
    kind: &'static str,
    branch: Option<String>,
    pr: Option<PullRequest>,
}

async fn prepare_worktree(app: &AppState, project: &Project, issue: &Issue, role: Role, run_id: i64) -> Result<Prepared, Outcome> {
    let root = app.config.worktrees_dir().join(&project.slug);
    let repo = &project.repo_path;
    let pr = services::pulls::open_pr_for_issue(&app.db, issue.id).await.map_err(|e| Outcome::Failed(e.to_string()))?;
    match role {
        Role::Fix | Role::MergePrep => {
            let branch = match &pr {
                Some(p) => p.branch.clone(),
                None => branch_for(project, issue),
            };
            if role == Role::MergePrep && pr.is_none() {
                return Err(Outcome::Blocked("merge prep needs an open PR, but none is linked".into()));
            }
            let path = root.join(format!("issue-{}", issue.number));
            git::worktree::ensure_branch(repo, &path, &branch, &project.base_branch)
                .await
                .map_err(|e| Outcome::Blocked(format!("{e:#}")))?;
            record_worktree(app, project, issue.id, &path, Some(&branch), "branch").await;
            let _ = sqlx::query("UPDATE issues SET branch_name = ? WHERE id = ?").bind(&branch).bind(issue.id).execute(&app.db).await;
            Ok(Prepared { worktree: path, kind: "branch", branch: Some(branch), pr })
        }
        Role::Review => {
            let Some(pr) = pr else {
                return Err(Outcome::Blocked("the issue is in review but has no open pull request; open one or move the issue".into()));
            };
            let pr = services::pulls::refresh_head(app, project, &pr).await.map_err(|e| Outcome::Failed(e.to_string()))?;
            let head = pr
                .head_sha
                .clone()
                .ok_or_else(|| Outcome::Blocked(format!("branch `{}` of PR #{} does not exist locally", pr.branch, pr.number)))?;
            let path = root.join(format!("review-{}-run{run_id}", issue.number));
            git::worktree::add_detached(repo, &path, &head).await.map_err(|e| Outcome::Failed(format!("{e:#}")))?;
            record_worktree(app, project, issue.id, &path, None, "detached").await;
            Ok(Prepared { worktree: path, kind: "detached", branch: None, pr: Some(pr) })
        }
        Role::Triage => {
            let path = root.join(format!("triage-{}-run{run_id}", issue.number));
            git::worktree::add_detached(repo, &path, &project.base_branch).await.map_err(|e| Outcome::Failed(format!("{e:#}")))?;
            record_worktree(app, project, issue.id, &path, None, "detached").await;
            Ok(Prepared { worktree: path, kind: "detached", branch: None, pr })
        }
    }
}

async fn run_setup(app: &AppState, project: &Project, worktree: &Path, tr: &mut Transcript) -> Result<(), String> {
    let Some(script) = project.setup_script.as_deref().filter(|s| !s.trim().is_empty()) else { return Ok(()) };
    // In container mode the setup runs in the project image, so e.g. a configured build dir
    // points at the container's toolchain rather than the host's.
    let mut cmd = if project.container_enabled {
        tr.insert("status", None, json!({"text": "running project setup script in the container"})).await;
        let args = crate::container::setup_args(app, project, worktree, script).await.map_err(|e| format!("{e:#}"))?;
        let mut c = tokio::process::Command::new("docker");
        c.args(args);
        c
    } else {
        tr.insert("status", None, json!({"text": "running project setup script"})).await;
        let mut c = tokio::process::Command::new("bash");
        c.arg("-lc").arg(script).current_dir(worktree).env("AKB_REPO", &project.repo_path);
        c
    };
    let out = tokio::time::timeout(Duration::from_secs(15 * 60), cmd.kill_on_drop(true).output())
        .await
        .map_err(|_| "setup script timed out after 15 minutes".to_string())?
        .map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let tail: String = text.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    tr.insert("setup", None, json!({"text": tail, "ok": out.status.success()})).await;
    if out.status.success() { Ok(()) } else { Err(format!("setup script failed (exit {})", out.status.code().unwrap_or(-1))) }
}

#[derive(serde::Serialize)]
struct CommentCtx {
    author: String,
    kind: String,
    body: String,
    created_at: String,
}

fn comment_ctx(c: &Comment) -> CommentCtx {
    CommentCtx { author: c.author_name.clone(), kind: c.kind.clone(), body: c.body.clone(), created_at: c.created_at.clone() }
}

async fn build_prompt(
    app: &AppState,
    project: &Project,
    issue: &Issue,
    role: Role,
    prep: &Prepared,
    token: &str,
    api: &str,
    resume_hint: Option<String>,
) -> anyhow::Result<String> {
    let custom: Option<String> = sqlx::query_scalar("SELECT body FROM prompt_templates WHERE project_id = ? AND role = ?")
        .bind(project.id)
        .bind(role.as_str())
        .fetch_optional(&app.db)
        .await?;
    let template = custom.unwrap_or_else(|| prompt::default_template(role).to_string());
    let labels: Vec<String> = services::labels::issue_labels(&app.db, issue.id).await?.into_iter().map(|l| l.name).collect();
    let all = comments::for_issue(app, issue.id).await?;
    let decisions: Vec<CommentCtx> = all.iter().filter(|c| c.kind == "decision").map(comment_ctx).collect();
    let comments: Vec<CommentCtx> = all.iter().filter(|c| c.kind != "system" || c.body.contains("failed")).map(comment_ctx).collect();
    let prev: Vec<Value> = sqlx::query_as::<_, (i64, String, String, String, Option<String>)>(
        "SELECT r.id, r.role, a.slug, r.status, r.error FROM agent_runs r JOIN agent_definitions a ON a.id = r.agent_definition_id
          WHERE r.issue_id = ? AND r.status NOT IN ('queued','preparing','running') ORDER BY r.id DESC LIMIT 6",
    )
    .bind(issue.id)
    .fetch_all(&app.db)
    .await?
    .into_iter()
    .rev()
    .map(|(id, role, agent, status, error)| json!({"id": id, "role": role, "agent": agent, "status": status, "error": error}))
    .collect();
    let (pr_json, threads, latest_review, pr_comments) = match &prep.pr {
        Some(pr) => {
            let threads: Vec<Value> = services::pulls::threads(app, pr.id, Some(false))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_iter()
                .map(|t| {
                    json!({"id": t.thread.id, "path": t.thread.path, "line": t.thread.line, "severity": t.thread.severity,
                        "outdated": t.thread.outdated,
                        "comments": t.comments.iter().map(|c| json!({"author": c.author_name, "body": c.body})).collect::<Vec<_>>()})
                })
                .collect();
            let review: Option<Review> =
                sqlx::query_as::<_, Review>("SELECT * FROM reviews WHERE pr_id = ? AND verdict != 'comment' ORDER BY id DESC LIMIT 1")
                    .bind(pr.id)
                    .fetch_optional(&app.db)
                    .await?;
            let pr_comments: Vec<CommentCtx> = comments::for_pr(app, pr.id).await?.iter().rev().take(15).rev().map(comment_ctx).collect();
            (
                json!({"number": pr.number, "title": pr.title, "body": pr.body, "branch": pr.branch, "head_sha": pr.head_sha,
                    "last_reviewed_sha": pr.last_reviewed_sha, "merge_base_sha": pr.merge_base_sha,
                    "has_conflicts": pr.has_conflicts, "conflict_files": pr.conflict_files.0}),
                threads,
                review.map(|r| json!({"verdict": r.verdict, "body": r.body, "commit_sha": r.commit_sha, "author": r.author_name})),
                pr_comments,
            )
        }
        None => (Value::Null, vec![], None, vec![]),
    };
    // Triage sees the board's open (and recently closed) issues to deduplicate against.
    let open_issues: Vec<Value> = if role == Role::Triage {
        let cutoff = db::fmt_time(chrono::Utc::now() - chrono::Duration::days(60));
        sqlx::query_as::<_, (i64, String, String, Option<i64>)>(
            "SELECT i.number, i.state, i.title, (SELECT p.number FROM issues p WHERE p.id = i.parent_issue_id)
               FROM issues i WHERE i.project_id = ? AND i.id != ?
                AND (i.state NOT IN ('done','closed') OR COALESCE(i.closed_at, i.updated_at) >= ?)
              ORDER BY i.number DESC LIMIT 120",
        )
        .bind(project.id)
        .bind(issue.id)
        .bind(&cutoff)
        .fetch_all(&app.db)
        .await?
        .into_iter()
        .map(|(n, st, t, parent)| json!({"number": n, "state": st, "title": t, "parent": parent}))
        .collect()
    } else {
        vec![]
    };
    // Everyone else sees the issue's umbrella, children and siblings.
    let related: Vec<Value> = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT number, state, title, 'umbrella issue' FROM issues WHERE id = ?1
         UNION ALL SELECT number, state, title, 'part of this issue' FROM issues WHERE parent_issue_id = ?2
         UNION ALL SELECT number, state, title, 'shares the same umbrella' FROM issues WHERE ?1 IS NOT NULL AND parent_issue_id = ?1 AND id != ?2",
    )
    .bind(issue.parent_issue_id)
    .bind(issue.id)
    .fetch_all(&app.db)
    .await?
    .into_iter()
    .map(|(n, st, t, rel)| json!({"number": n, "state": st, "title": t, "relation": rel}))
    .collect();
    let ctx = json!({
        "open_issues": open_issues,
        "related": related,
        "api": api,
        "token": token,
        "worktree": prep.worktree.to_string_lossy(),
        "branch": prep.branch,
        "project": {
            "slug": project.slug, "name": project.name, "base_branch": project.base_branch,
            "instructions": project.agent_instructions, "commit_convention": project.commit_msg_regex,
        },
        "issue": {
            "number": issue.number, "title": issue.title, "body": issue.body, "state": issue.state.as_str(),
            "labels": labels, "priority": issue.priority,
        },
        "comments": comments,
        "decisions": decisions,
        "previous_runs": prev,
        "resume_hint": resume_hint,
        "pr": pr_json,
        "threads": threads,
        "latest_review": latest_review,
        "pr_comments": pr_comments,
        "role": role.as_str(),
    });
    prompt::render(&template, &ctx)
}

enum Recorded {
    /// The run itself recorded its outcome.
    ByRun,
    /// Someone else (human, scanner, another run) moved the issue on first.
    Superseded(String),
    Missing,
}

/// Did the run record the outcome its role requires — itself, rather than the issue merely moving on?
async fn outcome_recorded(app: &AppState, run: &AgentRun, issue_id: i64) -> Recorded {
    let Ok(issue) = services::issue_by_id(&app.db, issue_id).await else { return Recorded::Superseded("issue deleted".into()) };
    let acted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE run_id = ?1 AND issue_id = ?2 AND type IN ('issue.transition','issue.decision_requested'))
             OR EXISTS(SELECT 1 FROM reviews WHERE run_id = ?1 AND verdict IN ('approve','changes_requested','needs_decision'))",
    )
    .bind(run.id)
    .bind(issue_id)
    .fetch_one(&app.db)
    .await
    .unwrap_or(false);
    // Merge prep usually just commits the merge; the conflict scanner then moves the issue itself.
    let committed = match sqlx::query_as::<_, (Option<String>, Option<String>)>("SELECT start_head_sha, worktree_path FROM agent_runs WHERE id = ?")
        .bind(run.id)
        .fetch_one(&app.db)
        .await
    {
        Ok((Some(start), Some(wt))) => git::rev_parse(&wt, "HEAD").await.is_some_and(|h| h != start),
        _ => false,
    };
    let moved_on = issue.hold.is_some()
        || issue.state.is_terminal()
        || issue.state == IssueState::Backlog
        || match run.role {
            Role::Triage => issue.state != IssueState::Triage,
            Role::Fix => matches!(issue.state, IssueState::InReview | IssueState::ReadyToMerge),
            Role::MergePrep => issue.state != IssueState::MergeConflict,
            Role::Review => issue.state != IssueState::InReview,
        };
    if acted || (run.role == Role::MergePrep && committed && moved_on) {
        Recorded::ByRun
    } else if moved_on {
        Recorded::Superseded(format!("issue moved to {} before this run recorded an outcome", issue.state.as_str()))
    } else {
        Recorded::Missing
    }
}

/// The agent's default session settings with this project's per-role overrides applied.
pub async fn desired_session_config(
    app: &AppState,
    project: &Project,
    role: Role,
    agent: &AgentDefinition,
) -> std::collections::BTreeMap<String, Value> {
    let mut cfg = agent.session_config.0.clone();
    let over: Option<String> = sqlx::query_scalar("SELECT config FROM role_session_config WHERE project_id = ? AND role = ? AND agent_definition_id = ?")
        .bind(project.id)
        .bind(role.as_str())
        .bind(agent.id)
        .fetch_optional(&app.db)
        .await
        .ok()
        .flatten();
    if let Some(o) = over.and_then(|o| serde_json::from_str::<serde_json::Map<String, Value>>(&o).ok()) {
        for (k, v) in o {
            if v.is_null() {
                cfg.remove(&k);
            } else {
                cfg.insert(k, v);
            }
        }
    }
    cfg
}

/// Option id → current value.
pub fn effective_config(options: &Value) -> serde_json::Map<String, Value> {
    options
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| Some((o.get("id")?.as_str()?.to_string(), o.get("currentValue")?.clone())))
        .collect()
}

/// "model: Opus · effort: High" from the model/thought-level options, using display names.
pub fn config_summary(options: &Value) -> String {
    let mut parts = vec![];
    for o in options.as_array().into_iter().flatten() {
        let cat = o.get("category").and_then(Value::as_str).unwrap_or("");
        if !matches!(cat, "model" | "thought_level") {
            continue;
        }
        let cur = o.get("currentValue").cloned().unwrap_or(Value::Null);
        let name = crate::acp::option_values(o)
            .iter()
            .position(|v| Some(v.as_str()) == cur.as_str())
            .and_then(|_| {
                let all: Vec<&Value> = o["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|x| x.get("options").and_then(Value::as_array).map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_else(|| vec![x]))
                    .collect();
                all.into_iter().find(|x| x.get("value") == Some(&cur)).and_then(|x| x.get("name")).and_then(Value::as_str).map(str::to_string)
            })
            .or_else(|| cur.as_str().map(str::to_string))
            .unwrap_or_default();
        let label = if cat == "model" { "model" } else { "effort" };
        parts.push(format!("{label}: {name}"));
    }
    parts.join(" · ")
}

pub async fn remember_config_options(app: &AppState, agent_id: i64, options: &Value) {
    if options.as_array().is_some_and(|a| !a.is_empty()) {
        let _ = sqlx::query("UPDATE agent_definitions SET config_options = ?, config_options_at = ? WHERE id = ?")
            .bind(options.to_string())
            .bind(db::now())
            .bind(agent_id)
            .execute(&app.db)
            .await;
    }
}

async fn set_status(app: &AppState, run_id: i64, status: &str) {
    let started = if status == "running" { Some(db::now()) } else { None };
    let _ = sqlx::query("UPDATE agent_runs SET status = ?, started_at = COALESCE(started_at, ?) WHERE id = ?")
        .bind(status)
        .bind(started)
        .bind(run_id)
        .execute(&app.db)
        .await;
}

pub struct LaunchPlan {
    pub launch: acp::Launch,
    pub container: Option<String>,
    pub mode: Option<String>,
}

/// Work out how to start the agent's adapter for a run (host process or container).
pub async fn plan_launch(
    app: &AppState,
    project: Option<&Project>,
    agent: &AgentDefinition,
    run_id: i64,
    cwd: &Path,
    extra_env: Vec<(String, String)>,
) -> anyhow::Result<LaunchPlan> {
    if let Some(p) = project.filter(|p| p.container_enabled) {
        let mut env = extra_env;
        for (k, v) in env.iter_mut() {
            if k == "AKB_API" {
                *v = v.replace(&app.config.public_url, &app.config.container_url);
            }
        }
        let c = crate::container::run_args(app, p, agent, run_id, cwd, &env).await?;
        return Ok(LaunchPlan {
            launch: acp::Launch { program: "docker".into(), args: c.args, env: vec![], cwd: cwd.to_path_buf() },
            container: Some(c.name),
            mode: agent.container_session_mode_id.clone().or(agent.session_mode_id.clone()),
        });
    }
    let mut env: Vec<(String, String)> = agent.env.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if agent.harness == "codex"
        && let Some(p) = project
    {
        // Let codex's workspace-write sandbox commit (the worktree's git dir lives in the main repo).
        let common = git::common_dir(&p.repo_path).await.unwrap_or_default();
        let mut cfg: Value =
            env.iter().find(|(k, _)| k == "CODEX_CONFIG").and_then(|(_, v)| serde_json::from_str(v).ok()).unwrap_or(json!({}));
        cfg["sandbox_workspace_write"]["writable_roots"] = json!([common]);
        if cfg["sandbox_workspace_write"].get("network_access").is_none() {
            cfg["sandbox_workspace_write"]["network_access"] = json!(true);
        }
        env.retain(|(k, _)| k != "CODEX_CONFIG");
        env.push(("CODEX_CONFIG".into(), cfg.to_string()));
    }
    env.extend(extra_env);
    Ok(LaunchPlan {
        launch: acp::Launch { program: agent.command.clone(), args: agent.args.0.clone(), env, cwd: cwd.to_path_buf() },
        container: None,
        mode: agent.session_mode_id.clone(),
    })
}

/// Entry point for a run task.
pub async fn execute(app: AppState, run_id: i64) {
    let ch = app.runs.register(run_id);
    let finished = ch.finished;
    let res = execute_inner(&app, run_id, ch.cancel, ch.cancel_reason, ch.followups, ch.events).await;
    if let Err(e) = res {
        tracing::error!("run {run_id} crashed: {e:#}");
        let _ = sqlx::query(
            "UPDATE agent_runs SET status = 'failed', error = ?, ended_at = ? WHERE id = ? AND status IN ('queued','preparing','running')",
        )
        .bind(format!("internal error: {e:#}"))
        .bind(db::now())
        .bind(run_id)
        .execute(&app.db)
        .await;
    }
    app.runs.unregister(run_id);
    let _ = finished.send(true);
    app.bus.run(None, run_id);
    app.bus.scan.notify_one();
    app.bus.wake.notify_one();
}

async fn execute_inner(
    app: &AppState,
    run_id: i64,
    cancel: tokio_util::sync::CancellationToken,
    cancel_reason: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    mut followups: mpsc::UnboundedReceiver<String>,
    events: tokio::sync::broadcast::Sender<RunEvent>,
) -> anyhow::Result<()> {
    let run = sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE id = ?").bind(run_id).fetch_one(&app.db).await?;
    let issue_id = run.issue_id.ok_or_else(|| anyhow::anyhow!("run has no issue"))?;
    let issue = services::issue_by_id(&app.db, issue_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let project = services::project_by_id(&app.db, run.project_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let agent = sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE id = ?")
        .bind(run.agent_definition_id)
        .fetch_one(&app.db)
        .await?;
    let stderr_path = app.config.runs_dir().join(format!("{run_id}.stderr.log"));
    let mut tr = Transcript {
        app: app.clone(),
        run_id,
        seq: sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM run_events WHERE run_id = ?").bind(run_id).fetch_one(&app.db).await?,
        events,
        pending: None,
        blocks: 0,
        stderr_file: tokio::fs::File::create(&stderr_path).await.ok(),
        stderr_tail: VecDeque::new(),
        pending_stderr: vec![],
        last_message: String::new(),
        rate_limit_meta: None,
        acp_usage: Default::default(),
    };
    set_status(app, run_id, "preparing").await;
    app.bus.run(Some(&project.slug), run_id);
    app.bus.issue(&project.slug, issue.number);
    tr.insert("status", None, json!({"text": format!("preparing {} run with {}", run.role.as_str(), agent.name)})).await;

    let mut child_holder: Option<tokio::process::Child> = None;
    let mut container: Option<String> = None;
    let mut prep_holder: Option<Prepared> = None;

    let outcome = 'run: {
        let prep = match prepare_worktree(app, &project, &issue, run.role, run_id).await {
            Ok(p) => p,
            Err(o) => break 'run o,
        };
        let start_sha = git::rev_parse(&prep.worktree, "HEAD").await;
        let _ = sqlx::query("UPDATE agent_runs SET worktree_path = ?, worktree_kind = ?, start_head_sha = ?, pr_id = ? WHERE id = ?")
            .bind(prep.worktree.to_string_lossy().to_string())
            .bind(prep.kind)
            .bind(&start_sha)
            .bind(prep.pr.as_ref().map(|p| p.id))
            .bind(run_id)
            .execute(&app.db)
            .await;
        // Every run gets the project's setup (e.g. a configured build dir): reviewers build and
        // run tests too, triage may reproduce, and a reused worktree may switch between host and
        // container toolchains. Setup scripts should be idempotent and fast when nothing changed.
        if let Err(e) = run_setup(app, &project, &prep.worktree, &mut tr).await
        {
            prep_holder = Some(prep);
            break 'run Outcome::Failed(e);
        }
        if cancel.is_cancelled() {
            prep_holder = Some(prep);
            break 'run Outcome::Cancelled("cancelled before start".into());
        }
        // A fix run starting moves the work into progress.
        if run.role == Role::Fix {
            let cur = services::issue_by_id(&app.db, issue_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            if matches!(cur.state, IssueState::Ready | IssueState::ChangesRequested | IssueState::Backlog) {
                let _ = services::issues::set_state(app, &project, &cur, IssueState::InProgress, &Actor::System, None, None).await;
            }
        }

        // Resume hints for runs following an interruption.
        let last_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM agent_runs WHERE issue_id = ? AND id < ? ORDER BY id DESC LIMIT 1")
                .bind(issue_id)
                .bind(run_id)
                .fetch_optional(&app.db)
                .await?;
        let dirty = prep.kind == "branch" && git::is_dirty(&prep.worktree).await;
        let resume_hint = match (last_status.as_deref(), dirty) {
            (Some("rate_limited" | "interrupted" | "cancelled" | "failed"), true) | (_, true) => Some(
                "A previous run on this issue stopped before finishing and left uncommitted changes in the worktree. Inspect `git status` and `git diff`, keep what's good, and continue from there.".to_string(),
            ),
            (Some("rate_limited" | "interrupted"), false) => Some(
                "A previous run on this issue was interrupted (usage limit or restart). Check `git log` on your branch and the thread above for work already done, and continue.".to_string(),
            ),
            _ => None,
        };

        // Per-run token.
        let token = format!("akr_{}", auth::random_token());
        sqlx::query("UPDATE agent_runs SET token_hash = ? WHERE id = ?")
            .bind(auth::hash_token(&token))
            .bind(run_id)
            .execute(&app.db)
            .await?;
        let api = if project.container_enabled { format!("{}/api", app.config.container_url) } else { app.config.api_url() };
        let prompt_text = match build_prompt(app, &project, &issue, run.role, &prep, &token, &api, resume_hint).await {
            Ok(p) => p,
            Err(e) => {
                prep_holder = Some(prep);
                break 'run Outcome::Failed(format!("prompt template error: {e:#}"));
            }
        };
        let env = vec![
            ("AKB_API".to_string(), app.config.api_url()),
            ("AKB_AUTH".to_string(), token.clone()),
            ("AKB_PROJECT".to_string(), project.slug.clone()),
            ("AKB_ISSUE".to_string(), issue.number.to_string()),
            ("AKB_RUN".to_string(), run_id.to_string()),
        ];
        let plan = match plan_launch(app, Some(&project), &agent, run_id, &prep.worktree, env).await {
            Ok(p) => p,
            Err(e) => {
                prep_holder = Some(prep);
                break 'run Outcome::Blocked(format!("{e:#}"));
            }
        };
        container = plan.container.clone();
        // Board tools over MCP (work even inside an agent's shell sandbox), and the repo's shared
        // .git as an extra writable dir so sandboxed agents can commit from a linked worktree.
        let base_url = if container.is_some() { app.config.container_url.clone() } else { app.config.public_url.clone() };
        let mut extras = acp::SessionExtras {
            mcp: Some((format!("{base_url}/mcp"), token.clone())),
            additional_directories: vec![],
            config: desired_session_config(app, &project, run.role, &agent).await.into_iter().collect(),
        };
        if prep.kind == "branch"
            && let Ok(common) = git::common_dir(&project.repo_path).await
        {
            extras.additional_directories.push(PathBuf::from(common));
        }
        let mut child = match acp::spawn(&plan.launch) {
            Ok(c) => c,
            Err(e) => {
                prep_holder = Some(prep);
                break 'run Outcome::Blocked(format!("{e:#}"));
            }
        };
        let _ = sqlx::query("UPDATE agent_runs SET container_name = ? WHERE id = ?").bind(&container).bind(run_id).execute(&app.db).await;
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take();
        child_holder = Some(child);
        let drive =
            tokio::spawn(acp::drive(stdin, stdout, stderr, prep.worktree.clone(), plan.mode.clone(), extras, out_tx, ctl_rx, cancel.clone()));
        tr.insert("prompt", None, json!({"text": prompt_text})).await;
        let _ = ctl_tx.send(Ctl::Prompt(prompt_text));

        let timeouts: std::collections::BTreeMap<String, i64> = db::get_setting(&app.db, "run_timeouts_minutes").await.unwrap_or_default();
        let limit = Duration::from_secs(60 * timeouts.get(run.role.as_str()).copied().unwrap_or(120).max(1) as u64);
        let deadline = tokio::time::sleep(limit);
        tokio::pin!(deadline);
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        let mut queued: VecDeque<String> = VecDeque::new();
        let mut in_turn = true;
        let mut nudges = 0;
        let mut drive = drive;
        // A JoinHandle panics if polled again after it completed.
        let mut drive_done = false;
        let mut turn_error: Option<AcpError>;

        let outcome = loop {
            tokio::select! {
                ev = out_rx.recv() => {
                    let Some(ev) = ev else {
                        // The session ended (process exited or transport closed).
                        tr.flush().await;
                        let err = match (&mut drive).await {
                            Ok(Err(e)) => Some(e),
                            _ => None,
                        };
                        drive_done = true;
                        if cancel.is_cancelled() {
                            break Outcome::Cancelled(cancel_reason.lock().unwrap().clone().unwrap_or_else(|| "cancelled".into()));
                        }
                        let sig = limits::classify(&Evidence { error: err.as_ref(), last_message: &tr.last_message, stderr_tail: &tr.stderr_tail(), rate_limit_meta: tr.rate_limit_meta.as_ref(), now: chrono::Utc::now(), tz: limits::local_tz() });
                        if sig != LimitSignal::None { break Outcome::RateLimited(sig); }
                        let tail = tr.stderr_tail();
                        let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                        break Outcome::Failed(match err { Some(e) => format!("agent session ended: {e}"), None => format!("agent process exited unexpectedly {last}") });
                    };
                    match ev {
                        Out::Initialized { agent_info, session_id, modes } => {
                            let _ = sqlx::query("UPDATE agent_runs SET acp_session_id = ?, agent_info = ? WHERE id = ?")
                                .bind(&session_id).bind(agent_info.to_string()).bind(run_id).execute(&app.db).await;
                            set_status(app, run_id, "running").await;
                            tr.insert("status", None, json!({"text": "session started", "agent": agent_info, "modes": modes})).await;
                            app.bus.run(Some(&project.slug), run_id);
                            app.bus.issue(&project.slug, issue.number);
                        }
                        Out::Update(v) => {
                            if let Some(snap) = tr.update(&v).await {
                                limits::record_snapshot(app, &agent.limit_group, &snap).await;
                            }
                        }
                        Out::Stderr(l) => tr.stderr(l).await,
                        Out::Config { options, applied: _, skipped } => {
                            let effective = effective_config(&options);
                            let _ = sqlx::query("UPDATE agent_runs SET session_config = ? WHERE id = ?")
                                .bind(Value::Object(effective.clone()).to_string())
                                .bind(run_id)
                                .execute(&app.db)
                                .await;
                            remember_config_options(app, agent.id, &options).await;
                            let summary = config_summary(&options);
                            if !summary.is_empty() {
                                tr.insert("status", None, json!({"text": format!("settings: {summary}"), "config": effective})).await;
                            }
                            for (id, why) in skipped {
                                tr.insert("status", None, json!({"text": format!("⚠️ couldn't set `{id}`: {why}")})).await;
                            }
                            app.bus.run(Some(&project.slug), run_id);
                        }
                        Out::Permission { tool_call, options, reply } => {
                            tr.flush().await;
                            handle_permission(app, &mut tr, &agent, run_id, container.is_some(), &prep.worktree.to_string_lossy(), tool_call, options, reply).await;
                        }
                        Out::TurnEnded(res) => {
                            tr.flush().await;
                            turn_error = res.as_ref().err().cloned();
                            if let Ok(r) = &res
                                && let Some(u) = r.get("usage").filter(|u| !u.is_null()) {
                                    let _ = sqlx::query("UPDATE agent_runs SET usage = ? WHERE id = ?").bind(u.to_string()).bind(run_id).execute(&app.db).await;
                                    tr.acp_usage.usage = Some(u.clone());
                                }
                            crate::usage::refresh_run(app, run_id, Some(&tr.acp_usage)).await;
                            let stop = res.as_ref().ok().and_then(|r| r.get("stopReason")).and_then(Value::as_str).unwrap_or("error").to_string();
                            let _ = sqlx::query("UPDATE agent_runs SET stop_reason = ? WHERE id = ?").bind(&stop).bind(run_id).execute(&app.db).await;
                            tr.insert("status", None, json!({"text": format!("turn ended: {stop}"), "error": turn_error})).await;
                            let sig = limits::classify(&Evidence { error: turn_error.as_ref(), last_message: &tr.last_message, stderr_tail: &tr.stderr_tail(), rate_limit_meta: tr.rate_limit_meta.as_ref(), now: chrono::Utc::now(), tz: limits::local_tz() });
                            if sig != LimitSignal::None {
                                break Outcome::RateLimited(sig);
                            }
                            if cancel.is_cancelled() || stop == "cancelled" {
                                break Outcome::Cancelled(cancel_reason.lock().unwrap().clone().unwrap_or_else(|| "cancelled".into()));
                            }
                            if let Some(e) = &turn_error {
                                break Outcome::Failed(format!("agent error: {e}"));
                            }
                            if let Some(next) = queued.pop_front() {
                                tr.insert("prompt", None, json!({"text": next, "from": "human"})).await;
                                tr.last_message.clear();
                                let _ = ctl_tx.send(Ctl::Prompt(next));
                                in_turn = true;
                                continue;
                            }
                            match outcome_recorded(app, &run, issue_id).await {
                                Recorded::ByRun => break Outcome::Succeeded(format!("turn ended ({stop})")),
                                Recorded::Superseded(why) => break Outcome::Cancelled(format!("superseded: {why}")),
                                Recorded::Missing => {}
                            }
                            if nudges < 1 && matches!(stop.as_str(), "end_turn" | "max_turn_requests" | "max_tokens") {
                                nudges += 1;
                                let _ = sqlx::query("UPDATE agent_runs SET nudges = nudges + 1 WHERE id = ?").bind(run_id).execute(&app.db).await;
                                tr.insert("prompt", None, json!({"text": prompt::NUDGE, "from": "nudge"})).await;
                                tr.last_message.clear();
                                let _ = ctl_tx.send(Ctl::Prompt(prompt::NUDGE.to_string()));
                                in_turn = true;
                                continue;
                            }
                            break Outcome::Failed(format!("ended without recording an outcome (stop reason: {stop})"));
                        }
                    }
                }
                Some(msg) = followups.recv() => {
                    if in_turn {
                        tr.insert("status", None, json!({"text": "message queued for the next turn"})).await;
                        queued.push_back(msg);
                    } else {
                        tr.insert("prompt", None, json!({"text": msg, "from": "human"})).await;
                        let _ = ctl_tx.send(Ctl::Prompt(msg));
                        in_turn = true;
                    }
                }
                _ = tick.tick() => tr.maybe_flush().await,
                _ = cancel.cancelled(), if !cancel.is_cancelled() => {
                    tr.insert("status", None, json!({"text": "cancelling"})).await;
                    // drive() sends session/cancel and ends; loop continues until the channel closes.
                }
                _ = &mut deadline => {
                    *cancel_reason.lock().unwrap() = Some(format!("timed out after {} minutes", limit.as_secs() / 60));
                    cancel.cancel();
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    break Outcome::Failed(format!("timed out after {} minutes", limit.as_secs() / 60));
                }
            }
        };
        let _ = ctl_tx.send(Ctl::Close);
        drop(ctl_tx);
        if !drive_done && tokio::time::timeout(Duration::from_secs(10), &mut drive).await.is_err() {
            drive.abort();
        }
        tr.flush().await;
        prep_holder = Some(prep);
        outcome
    };

    // Teardown.
    if let Some(mut c) = child_holder.take() {
        acp::kill_tree(&mut c).await;
    }
    // Final usage (session logs are complete once the agent process has exited).
    crate::usage::refresh_run(app, run_id, Some(&tr.acp_usage)).await;
    if let Some(name) = &container {
        crate::container::kill(name).await;
    }
    tr.flush().await;
    let grace = db::fmt_time(chrono::Utc::now() + chrono::Duration::minutes(5));
    let end_sha = match &prep_holder {
        Some(p) => git::rev_parse(&p.worktree, "HEAD").await,
        None => None,
    };
    if let Some(p) = &prep_holder
        && p.kind == "detached"
    {
        let _ = git::worktree::remove(&project.repo_path, &p.worktree).await;
        let _ = sqlx::query("UPDATE worktrees SET removed_at = ? WHERE path = ? AND removed_at IS NULL")
            .bind(db::now())
            .bind(p.worktree.to_string_lossy().to_string())
            .execute(&app.db)
            .await;
    }

    let (status, text, error) = match &outcome {
        Outcome::Succeeded(m) => ("succeeded", m.clone(), None),
        Outcome::Failed(m) => ("failed", m.clone(), Some(m.clone())),
        Outcome::Blocked(m) => ("failed", m.clone(), Some(m.clone())),
        Outcome::RateLimited(sig) => ("rate_limited", format!("{sig:?}"), Some(format!("{sig:?}"))),
        Outcome::Cancelled(m) => ("cancelled", m.clone(), Some(m.clone())),
    };
    sqlx::query(
        "UPDATE agent_runs SET status = ?, outcome = ?, error = ?, end_head_sha = ?, ended_at = ?, token_expires_at = ? WHERE id = ?",
    )
    .bind(status)
    .bind(&text)
    .bind(&error)
    .bind(&end_sha)
    .bind(db::now())
    .bind(&grace)
    .bind(run_id)
    .execute(&app.db)
    .await?;
    tr.insert("status", None, json!({"text": format!("run {status}: {text}")})).await;

    match outcome {
        Outcome::Succeeded(_) => {
            sqlx::query("UPDATE issues SET failure_count = 0, next_attempt_at = NULL WHERE id = ?").bind(issue_id).execute(&app.db).await?;
        }
        Outcome::Failed(msg) => {
            let max: i64 = db::get_setting(&app.db, "max_failures").await.unwrap_or(3);
            let count: i64 = sqlx::query_scalar("UPDATE issues SET failure_count = failure_count + 1 WHERE id = ? RETURNING failure_count")
                .bind(issue_id)
                .fetch_one(&app.db)
                .await?;
            let note = format!("⚠️ {} run #{run_id} ({}) failed: {msg}", run.role.as_str(), agent.slug);
            system_comment(app, &project, issue_id, &note).await;
            if count >= max {
                let _ =
                    services::issues::system_hold(app, issue_id, Hold::Stalled, &format!("{count} consecutive failed runs; last: {msg}"))
                        .await;
            } else {
                let backoff = [5i64, 30, 120][(count as usize - 1).min(2)];
                sqlx::query("UPDATE issues SET next_attempt_at = ? WHERE id = ?")
                    .bind(db::fmt_time(chrono::Utc::now() + chrono::Duration::minutes(backoff)))
                    .bind(issue_id)
                    .execute(&app.db)
                    .await?;
            }
        }
        Outcome::Blocked(msg) => {
            let _ = services::issues::system_hold(app, issue_id, Hold::Stalled, &msg).await;
        }
        Outcome::RateLimited(sig) => {
            let summary = limits::apply_signal(app, &agent.limit_group, agent.id, &sig).await.unwrap_or_default();
            tr.insert("status", None, json!({"text": summary})).await;
            if matches!(sig, LimitSignal::Auth { .. }) {
                system_comment(
                    app,
                    &project,
                    issue_id,
                    &format!("🔑 `{}` needs to be logged in before it can run ({summary}). Log in, then resume the `{}` group on the Agents page.", agent.slug, agent.limit_group),
                )
                .await;
            }
        }
        Outcome::Cancelled(_) => {}
    }
    app.bus.run(Some(&project.slug), run_id);
    app.bus.issue(&project.slug, issue.number);
    Ok(())
}

async fn system_comment(app: &AppState, project: &Project, issue_id: i64, body: &str) {
    if let Ok(mut tx) = db::begin_write(&app.db).await {
        let _ = comments::insert(&mut tx, project.id, comments::Target::Issue(issue_id), &Actor::System, "system", body).await;
        let _ = tx.commit().await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_permission(
    app: &AppState,
    tr: &mut Transcript,
    agent: &AgentDefinition,
    run_id: i64,
    in_container: bool,
    worktree: &str,
    tool_call: Value,
    options: Vec<agent_client_protocol::schema::v1::PermissionOption>,
    reply: oneshot::Sender<Option<String>>,
) {
    use agent_client_protocol::schema::v1::PermissionOptionKind as K;
    let pick = |kinds: &[K]| -> Option<String> {
        kinds.iter().find_map(|k| options.iter().find(|o| &o.kind == k).map(|o| o.option_id.0.to_string()))
    };
    let opts_json = serde_json::to_value(&options).unwrap_or(json!([]));
    let title = tool_call.get("title").and_then(Value::as_str).unwrap_or("tool call").to_string();
    let is_board_tool = regex::Regex::new(r"agent-kanban|\bboard_[a-z_]+").unwrap().is_match(&title);
    let policy = if is_board_tool {
        "auto_allow"
    } else if in_container {
        agent.container_permission_policy.as_str()
    } else {
        agent.permission_policy.as_str()
    };
    let mut policy = policy.to_string();
    let mut note = "policy".to_string();
    if policy == "allowlist" {
        match super::permissions::evaluate(&agent.permission_rules.0, &tool_call, worktree) {
            Some(d) => {
                note = d.note;
                policy = match d.action {
                    super::permissions::RuleAction::Allow => "auto_allow",
                    super::permissions::RuleAction::Deny => "deny",
                    super::permissions::RuleAction::Ask => "ask",
                }
                .into();
            }
            None => {
                note = "no rule matched".into();
                policy = "ask".into();
            }
        }
    }
    match policy.as_str() {
        "deny" => {
            let choice = pick(&[K::RejectOnce, K::RejectAlways]);
            tr.insert("permission", None, json!({"title": title, "decision": format!("denied ({note})"), "tool_call": tool_call})).await;
            let _ = reply.send(choice);
        }
        "ask" => {
            let id: i64 = match sqlx::query_scalar(
                "INSERT INTO permission_requests(run_id, tool_call, options, status, created_at) VALUES (?, ?, ?, 'pending', ?) RETURNING id",
            )
            .bind(run_id)
            .bind(tool_call.to_string())
            .bind(opts_json.to_string())
            .bind(db::now())
            .fetch_one(&app.db)
            .await
            {
                Ok(id) => id,
                Err(_) => {
                    let _ = reply.send(None);
                    return;
                }
            };
            tr.insert(
                "permission",
                Some(&format!("perm:{id}")),
                json!({"id": id, "title": title, "options": opts_json, "status": "pending", "reason": note, "tool_call": tool_call}),
            )
            .await;
            app.bus.emit("permissions.updated", None, None, None, Some(run_id));
            let (tx, rx) = oneshot::channel();
            app.runs.add_permission_waiter(id, tx);
            let app2 = app.clone();
            tokio::spawn(async move {
                let answer = tokio::time::timeout(Duration::from_secs(30 * 60), rx).await.ok().and_then(|r| r.ok());
                if answer.is_none() {
                    app2.runs.drop_permission_waiter(id);
                    let _ = sqlx::query("UPDATE permission_requests SET status = 'expired' WHERE id = ? AND status = 'pending'")
                        .bind(id)
                        .execute(&app2.db)
                        .await;
                    app2.bus.emit("permissions.updated", None, None, None, Some(run_id));
                }
                let _ = reply.send(answer);
            });
        }
        _ => {
            let choice = pick(&[K::AllowAlways, K::AllowOnce]);
            tr.insert("permission", None, json!({"title": title, "decision": format!("allowed ({note})"), "tool_call": tool_call})).await;
            let _ = reply.send(choice);
        }
    }
}
