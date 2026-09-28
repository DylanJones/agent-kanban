//! The board API as MCP tools (Streamable HTTP, JSON responses) at `/mcp`.
//!
//! Every agent session gets this server attached over ACP, authenticated with its run token.
//! Tool calls are made by the agent process itself rather than from inside its shell sandbox,
//! so they work for sandboxed agents (e.g. Codex's workspace-write mode) that can't `curl`.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};

use crate::AppState;
use crate::domain::{Actor, IssueState};
use crate::error::ApiError;
use crate::services::{self, attachments, comments, issues, pulls, reviews};

const PROTOCOL: &str = "2025-06-18";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
    })
}

pub fn tools() -> Vec<Value> {
    let n = json!({"type": "integer", "description": "Number; defaults to your run's issue/PR"});
    vec![
        tool("board_current_run", "Your run: role, project, issue, PR, branch, worktree and what the board expects of you.", json!({}), &[]),
        tool(
            "board_file_issue",
            "File a NEW issue for a separate problem you noticed (don't fix it in your current task). Lands in triage.",
            json!({
                "title": {"type": "string"},
                "body": {"type": "string", "description": "Markdown: what, where (file:line), how to reproduce"},
                "labels": {"type": "array", "items": {"type": "string"}},
                "priority": {"type": "string", "enum": ["P0", "P1", "P2"]},
            }),
            &["title"],
        ),
        tool("board_get_issue", "Get an issue with its comments, decisions, PRs and allowed transitions.", json!({"number": n}), &[]),
        tool(
            "board_search_issues",
            "Search issues (e.g. to check for duplicates).",
            json!({"query": {"type": "string"}, "state": {"type": "string"}}),
            &["query"],
        ),
        tool(
            "board_comment",
            "Comment on your issue (default) or on a PR.",
            json!({"body": {"type": "string"}, "pr": {"type": "integer", "description": "Comment on this PR instead of the issue"}}),
            &["body"],
        ),
        tool(
            "board_update_issue",
            "Set fields on your issue. Triage agents may also set `parent`/`add_labels` on other open issues to group related bugs under an umbrella issue.",
            json!({
                "number": {"type": "integer", "description": "Defaults to your issue"},
                "priority": {"type": "string", "enum": ["P0", "P1", "P2"]},
                "size": {"type": "string", "enum": ["XS", "S", "M", "L", "XL"]},
                "add_labels": {"type": "array", "items": {"type": "string"}},
                "parent": {"type": "integer", "description": "Umbrella issue number (0 clears)"},
            }),
            &[],
        ),
        tool(
            "board_move_issue",
            "Move your issue to another workflow state (e.g. triage→ready, in_progress→in_review). Fails with the allowed states if not permitted.",
            json!({
                "to": {"type": "string", "enum": ["backlog", "ready", "closed", "in_progress", "in_review"]},
                "comment": {"type": "string"},
                "close_reason": {"type": "string", "description": "For closed: duplicate, invalid, wontfix"},
                "duplicate_of": {"type": "integer", "description": "With to=closed: the existing issue this duplicates (same bug, same fix). Its thread gets a copy of this report."},
            }),
            &["to"],
        ),
        tool(
            "board_request_decision",
            "Pause your issue for a human language/design decision. Only if the answer is NOT already in the thread. Stop after calling.",
            json!({
                "question": {"type": "string"},
                "options": {"type": "array", "items": {"type": "string"}},
                "consequences": {"type": "string"},
            }),
            &["question"],
        ),
        tool(
            "board_attach_image",
            "Upload an image (e.g. a screenshot of a bug) and get back a markdown snippet. Paste the returned `markdown` into board_comment or an issue/PR body — this only uploads, it doesn't post anywhere by itself.",
            json!({
                "data_base64": {"type": "string", "description": "Image bytes, base64-encoded. PNG, JPEG, GIF or WebP; 10 MiB max decoded."},
            }),
            &["data_base64"],
        ),
        tool(
            "board_open_pr",
            "Open a pull request for your branch (commit first). The title becomes the squash commit subject.",
            json!({"title": {"type": "string"}, "body": {"type": "string", "description": "Summary, test commands + results, dependencies"}}),
            &["title"],
        ),
        tool("board_get_pr", "Get a PR with comments, reviews, inline threads and head SHA.", json!({"number": n}), &[]),
        tool(
            "board_get_diff",
            "Unified diff of the PR against its merge base, per file. `since` limits to changes after a commit (re-review).",
            json!({"number": n, "since": {"type": "string"}}),
            &[],
        ),
        tool(
            "board_list_threads",
            "Inline review threads on the PR.",
            json!({"number": n, "resolved": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "board_add_thread",
            "Start an inline review thread on a line of the PR diff.",
            json!({
                "path": {"type": "string"},
                "line": {"type": "integer"},
                "side": {"type": "string", "enum": ["RIGHT", "LEFT"]},
                "severity": {"type": "string", "enum": ["blocking", "nit"]},
                "body": {"type": "string"},
                "number": n,
            }),
            &["path", "line", "body"],
        ),
        tool("board_reply_thread", "Reply to a review thread.", json!({"thread_id": {"type": "integer"}, "body": {"type": "string"}}), &["thread_id", "body"]),
        tool(
            "board_resolve_thread",
            "Resolve a review thread (reviewers, after verifying the fix).",
            json!({"thread_id": {"type": "integer"}, "comment": {"type": "string"}}),
            &["thread_id"],
        ),
        tool(
            "board_submit_review",
            "Record exactly one verdict for the PR head you reviewed. approve → ready to merge; changes_requested → back to the fix agent; needs_decision → paused for a human.",
            json!({
                "verdict": {"type": "string", "enum": ["approve", "changes_requested", "needs_decision", "comment"]},
                "commit_sha": {"type": "string"},
                "body": {"type": "string"},
                "number": n,
            }),
            &["verdict", "commit_sha"],
        ),
    ]
}

struct Ctx {
    app: AppState,
    actor: Actor,
    project: crate::domain::models::Project,
    issue_number: Option<i64>,
}

impl Ctx {
    fn issue(&self, args: &Value) -> Result<i64, ApiError> {
        args.get("number").and_then(Value::as_i64).or(self.issue_number).ok_or_else(|| ApiError::bad("`number` is required"))
    }

    async fn pr_number(&self, args: &Value) -> Result<i64, ApiError> {
        if let Some(n) = args.get("number").and_then(Value::as_i64) {
            return Ok(n);
        }
        let n = self.issue_number.ok_or_else(|| ApiError::bad("`number` is required"))?;
        let issue = services::issue(&self.app.db, self.project.id, n).await?;
        services::pulls::open_pr_for_issue(&self.app.db, issue.id)
            .await?
            .map(|p| p.number)
            .ok_or_else(|| ApiError::bad("your issue has no open PR; pass `number`"))
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

async fn call(cx: &Ctx, name: &str, a: &Value) -> Result<Value, ApiError> {
    let app = &cx.app;
    let p = &cx.project;
    let to_json = |v: &dyn erased::Ser| v.json();
    Ok(match name {
        "board_current_run" => {
            let Json(cur) = super::agents::current(State(app.clone()), cx.actor.clone()).await?;
            to_json(&cur)
        }
        "board_file_issue" => {
            let new = issues::NewIssue {
                title: s(a, "title").unwrap_or_default().into(),
                body: s(a, "body").unwrap_or_default().into(),
                labels: strings(a, "labels"),
                priority: s(a, "priority").map(str::to_string),
                ..Default::default()
            };
            to_json(&issues::create(app, p, &cx.actor, new).await?)
        }
        "board_get_issue" => to_json(&super::issues::detail(app, &cx.actor, p, cx.issue(a)?).await?),
        "board_search_issues" => {
            let q = format!("%{}%", s(a, "query").unwrap_or_default().to_lowercase());
            let rows: Vec<(i64, String, String)> = sqlx::query_as(
                "SELECT number, state, title FROM issues WHERE project_id = ? AND (lower(title) LIKE ? OR lower(body) LIKE ?)
                   AND (? IS NULL OR state = ?) ORDER BY number DESC LIMIT 30",
            )
            .bind(p.id)
            .bind(&q)
            .bind(&q)
            .bind(s(a, "state"))
            .bind(s(a, "state"))
            .fetch_all(&app.db)
            .await?;
            json!(rows.into_iter().map(|(n, st, t)| json!({"number": n, "state": st, "title": t})).collect::<Vec<_>>())
        }
        "board_comment" => {
            let body = s(a, "body").unwrap_or_default();
            let c = match a.get("pr").and_then(Value::as_i64) {
                Some(n) => {
                    let pr = services::pull(&app.db, p.id, n).await?;
                    let c = comments::add(app, p.id, comments::Target::Pr(pr.id), &cx.actor, body).await?;
                    app.bus.pr(&p.slug, n);
                    c
                }
                None => {
                    let n = cx.issue(a)?;
                    let issue = services::issue(&app.db, p.id, n).await?;
                    let c = comments::add(app, p.id, comments::Target::Issue(issue.id), &cx.actor, body).await?;
                    app.bus.issue(&p.slug, n);
                    c
                }
            };
            json!({"comment_id": c.id})
        }
        "board_update_issue" => {
            let patch = issues::IssuePatch {
                priority: s(a, "priority").map(str::to_string),
                size: s(a, "size").map(str::to_string),
                add_labels: a.get("add_labels").map(|_| strings(a, "add_labels")),
                parent: a.get("parent").and_then(Value::as_i64),
                ..Default::default()
            };
            to_json(&issues::update(app, p, cx.issue(a)?, &cx.actor, patch).await?)
        }
        "board_move_issue" => {
            let to: IssueState =
                serde_json::from_value(a.get("to").cloned().unwrap_or(Value::Null)).map_err(|_| ApiError::bad("unknown state in `to`"))?;
            let req = issues::TransitionRequest {
                to,
                comment: s(a, "comment").map(str::to_string),
                close_reason: s(a, "close_reason").map(str::to_string),
                duplicate_of: a.get("duplicate_of").and_then(Value::as_i64),
            };
            let i = issues::transition(app, p, cx.issue(a)?, &cx.actor, req).await?;
            json!({"number": i.number, "state": i.state})
        }
        "board_request_decision" => {
            let req = issues::DecisionRequest {
                question: s(a, "question").unwrap_or_default().into(),
                options: strings(a, "options"),
                consequences: s(a, "consequences").unwrap_or_default().into(),
            };
            let i = issues::request_decision(app, p, cx.issue(a)?, &cx.actor, req).await?;
            json!({"number": i.number, "hold": i.hold, "note": "The issue is paused for a human. End your turn now."})
        }
        "board_attach_image" => {
            let data = s(a, "data_base64").ok_or_else(|| ApiError::bad("data_base64 is required"))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map_err(|e| ApiError::bad(format!("invalid base64: {e}")))?;
            let att = attachments::save(app, p, &cx.actor, bytes).await?;
            json!({"url": att.url, "markdown": att.markdown})
        }
        "board_open_pr" => {
            let req = pulls::NewPull { title: s(a, "title").unwrap_or_default().into(), body: s(a, "body").unwrap_or_default().into(), issues: vec![], branch: None };
            let pr = pulls::create(app, p, &cx.actor, req).await?;
            json!({"number": pr.number, "branch": pr.branch, "head_sha": pr.head_sha})
        }
        "board_get_pr" => {
            let n = cx.pr_number(a).await?;
            let pr = services::pull(&app.db, p.id, n).await?;
            let pr = pulls::refresh_head(app, p, &pr).await?;
            json!({
                "pr": pr,
                "issues": pulls::linked_issue_numbers(&app.db, pr.id).await?,
                "comments": comments::for_pr(app, pr.id).await?,
                "reviews": reviews::list(app, pr.id).await?,
                "threads": pulls::threads(app, pr.id, None).await?,
            })
        }
        "board_get_diff" => {
            let n = cx.pr_number(a).await?;
            let pr = services::pull(&app.db, p.id, n).await?;
            let pr = pulls::refresh_head(app, p, &pr).await?;
            let files = pulls::diff(app, p, &pr, s(a, "since")).await?;
            // Plain unified diff text is what models read best.
            let text: String = files.iter().map(|f| f.patch.as_str()).collect();
            return Ok(json!({"head_sha": pr.head_sha, "text": text}));
        }
        "board_list_threads" => {
            let n = cx.pr_number(a).await?;
            let pr = services::pull(&app.db, p.id, n).await?;
            to_json(&pulls::threads(app, pr.id, a.get("resolved").and_then(Value::as_bool)).await?)
        }
        "board_add_thread" => {
            let n = cx.pr_number(a).await?;
            let req = pulls::NewThread {
                path: s(a, "path").unwrap_or_default().into(),
                line: a.get("line").and_then(Value::as_i64).unwrap_or(0),
                start_line: None,
                side: s(a, "side").unwrap_or("RIGHT").into(),
                body: s(a, "body").unwrap_or_default().into(),
                severity: s(a, "severity").unwrap_or("blocking").into(),
            };
            let t = pulls::create_thread(app, p, n, &cx.actor, req).await?;
            json!({"thread_id": t.thread.id})
        }
        "board_reply_thread" => {
            let id = a.get("thread_id").and_then(Value::as_i64).ok_or_else(|| ApiError::bad("thread_id is required"))?;
            pulls::reply(app, id, &cx.actor, s(a, "body").unwrap_or_default()).await?;
            json!({"ok": true})
        }
        "board_resolve_thread" => {
            let id = a.get("thread_id").and_then(Value::as_i64).ok_or_else(|| ApiError::bad("thread_id is required"))?;
            pulls::set_resolved(app, id, &cx.actor, true, s(a, "comment")).await?;
            json!({"ok": true})
        }
        "board_submit_review" => {
            let n = cx.pr_number(a).await?;
            let req = reviews::NewReview {
                verdict: s(a, "verdict").unwrap_or_default().into(),
                body: s(a, "body").unwrap_or_default().into(),
                commit_sha: s(a, "commit_sha").unwrap_or_default().into(),
            };
            let r = reviews::submit(app, p, n, &cx.actor, req).await?;
            json!({"review_id": r.id, "verdict": r.verdict, "commit_sha": r.commit_sha})
        }
        other => return Err(ApiError::bad(format!("unknown tool `{other}`"))),
    })
}

/// Tiny helper so `call` can serialize any result type uniformly.
mod erased {
    pub trait Ser {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn handle_one(app: &AppState, actor: &Actor, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications get no response
    Some(match method {
        "initialize" => {
            let requested = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL);
            rpc_result(
                &id,
                json!({
                    "protocolVersion": requested,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "agent-kanban", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Tools for the agent-kanban board that coordinates your work: file bugs you notice, comment, move your issue, open PRs, review, and ask for human decisions.",
                }),
            )
        }
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({"tools": tools()})),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            let Actor::Agent { project_id, issue_id, .. } = actor else {
                return Some(rpc_error(&id, -32600, "board MCP tools are for agent runs (use a run token)"));
            };
            let project = match services::project_by_id(&app.db, *project_id).await {
                Ok(p) => p,
                Err(e) => return Some(rpc_error(&id, -32603, &e.to_string())),
            };
            let issue_number = match issue_id {
                Some(i) => services::issue_by_id(&app.db, *i).await.ok().map(|i| i.number),
                None => None,
            };
            let cx = Ctx { app: app.clone(), actor: actor.clone(), project, issue_number };
            match call(&cx, name, &args).await {
                Ok(v) => {
                    let text = match v.get("text").and_then(Value::as_str) {
                        Some(t) if name == "board_get_diff" => format!("head_sha: {}\n\n{t}", v["head_sha"].as_str().unwrap_or("?")),
                        _ => serde_json::to_string_pretty(&v).unwrap_or_default(),
                    };
                    rpc_result(&id, json!({"content": [{"type": "text", "text": text}], "isError": false}))
                }
                Err(e) => {
                    let (status, detail) = match &e {
                        ApiError::InvalidTransition { detail, allowed } => {
                            ("invalid_transition", format!("{detail}. Allowed: {}", allowed.join(", ")))
                        }
                        other => ("error", other.to_string()),
                    };
                    rpc_result(&id, json!({"content": [{"type": "text", "text": format!("{status}: {detail}")}], "isError": true}))
                }
            }
        }
        _ => rpc_error(&id, -32601, &format!("method `{method}` not found")),
    })
}

/// `POST /mcp` — JSON-RPC over Streamable HTTP (always answered with plain JSON).
pub async fn post(State(app): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let actor = match crate::auth::resolve(&app, token.as_deref()).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let responses: Vec<Value> = match &body {
        Value::Array(batch) => {
            let mut out = vec![];
            for m in batch {
                if let Some(r) = handle_one(&app, &actor, m).await {
                    out.push(r);
                }
            }
            out
        }
        m => handle_one(&app, &actor, m).await.into_iter().collect(),
    };
    match (&body, responses.len()) {
        (_, 0) => StatusCode::ACCEPTED.into_response(),
        (Value::Array(_), _) => Json(Value::Array(responses)).into_response(),
        _ => Json(responses.into_iter().next().unwrap()).into_response(),
    }
}

/// `GET /mcp` — no server-initiated stream.
pub async fn get() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
