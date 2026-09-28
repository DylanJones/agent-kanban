//! HTTP-level tests: auth scoping, quick bug filing, error shapes.

mod common;

use agent_kanban::auth::hash_token;
use agent_kanban::domain::IssueState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine;
use common::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// A valid 1x1 transparent PNG.
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

async fn call_bytes(env: &Env, method: &str, path: &str, token: &str, ct: &str, body: Vec<u8>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", ct)
        .body(Body::from(body))
        .unwrap();
    let resp = agent_kanban::api::app(env.app.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, bytes)
}

async fn call(env: &Env, method: &str, path: &str, token: &str, ct: &str, body: String) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", ct)
        .body(Body::from(body))
        .unwrap();
    let resp = agent_kanban::api::app(env.app.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn agent_token(env: &Env, issue_id: i64, role: &str) -> String {
    fake_agent(&env.app, "f-any", "noop", "fake").await;
    let token = "akr_testtoken".to_string() + role;
    sqlx::query(
        "INSERT INTO agent_runs(project_id, issue_id, role, agent_definition_id, status, token_hash, created_at)
         SELECT ?, ?, ?, id, 'running', ?, ? FROM agent_definitions WHERE slug = 'f-any'",
    )
    .bind(env.project.id)
    .bind(issue_id)
    .bind(role)
    .bind(hash_token(&token))
    .bind(agent_kanban::db::now())
    .execute(&env.app.db)
    .await
    .unwrap();
    token
}

#[tokio::test]
async fn auth_required_and_admin_token_works() {
    let env = setup().await;
    let (s, _) = call(&env, "GET", "/api/projects", "nope", "application/json", String::new()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let admin = env.app.config.secrets.admin_token.clone();
    let (s, v) = call(&env, "GET", "/api/projects", &admin, "application/json", String::new()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v[0]["slug"], "demo");
    let (s, _) = call(&env, "GET", "/api/health", "", "application/json", String::new()).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn agent_files_bug_with_text_plain() {
    let env = setup().await;
    let i = new_issue(&env, "Work", IssueState::InProgress).await;
    let tok = agent_token(&env, i.id, "fix").await;
    let (s, v) = call(&env, "POST", "/api/projects/demo/issues", &tok, "text/plain", "Crash in parser\nSteps:\n1. run it".into()).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["state"], "triage");
    let n = v["number"].as_i64().unwrap();
    let (_, d) = call(&env, "GET", &format!("/api/projects/demo/issues/{n}"), &tok, "application/json", String::new()).await;
    assert_eq!(d["title"], "Crash in parser");
    assert_eq!(d["body"], "Steps:\n1. run it");
    assert_eq!(d["source"], "agent");
    assert!(d["labels"].as_array().unwrap().iter().any(|l| l["name"] == "found-by-agent"));
    // The agent may not edit the new issue (only file it).
    let (s, v) =
        call(&env, "PATCH", &format!("/api/projects/demo/issues/{n}"), &tok, "application/json", json!({"title": "x"}).to_string()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(v["detail"].as_str().unwrap().contains("create a new issue"));
}

#[tokio::test]
async fn invalid_transition_lists_allowed() {
    let env = setup().await;
    let i = new_issue(&env, "Work", IssueState::Triage).await;
    let tok = agent_token(&env, i.id, "triage").await;
    let (s, v) = call(
        &env,
        "POST",
        &format!("/api/projects/demo/issues/{}/transition", i.number),
        &tok,
        "application/json",
        json!({"to": "in_review"}).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["type"], "invalid_transition");
    let allowed: Vec<&str> = v["allowed_transitions"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(allowed, vec!["backlog", "ready", "closed"]);
    // Closing needs a reason.
    let (s, _) = call(
        &env,
        "POST",
        &format!("/api/projects/demo/issues/{}/transition", i.number),
        &tok,
        "application/json",
        json!({"to": "closed"}).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = call(
        &env,
        "POST",
        &format!("/api/projects/demo/issues/{}/transition", i.number),
        &tok,
        "application/json",
        json!({"to": "ready"}).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["state"], "ready");
}

#[tokio::test]
async fn fix_agent_cannot_request_review_without_pr_or_approve() {
    let env = setup().await;
    let i = new_issue(&env, "Work", IssueState::InProgress).await;
    let tok = agent_token(&env, i.id, "fix").await;
    let (s, v) = call(
        &env,
        "POST",
        &format!("/api/projects/demo/issues/{}/transition", i.number),
        &tok,
        "application/json",
        json!({"to": "in_review"}).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(v["detail"].as_str().unwrap().contains("open a pull request"), "{v}");
    // Humans only: merge, decisions, settings.
    let (s, _) = call(&env, "PATCH", "/api/settings", &tok, "application/json", json!({"max_concurrent_runs": 9}).to_string()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &env,
        "POST",
        &format!("/api/projects/demo/issues/{}/decision", i.number),
        &tok,
        "application/json",
        json!({"answer": "x"}).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ended_run_token_is_rejected() {
    let env = setup().await;
    let i = new_issue(&env, "Work", IssueState::InProgress).await;
    let tok = agent_token(&env, i.id, "fix").await;
    sqlx::query("UPDATE agent_runs SET status = 'succeeded', token_expires_at = '2000-01-01T00:00:00.000Z'")
        .execute(&env.app.db)
        .await
        .unwrap();
    let (s, _) = call(&env, "GET", "/api/me", &tok, "application/json", String::new()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn openapi_spec_and_guide() {
    let env = setup().await;
    let admin = env.app.config.secrets.admin_token.clone();
    let (s, v) = call(&env, "GET", "/api/openapi.json", &admin, "application/json", String::new()).await;
    assert_eq!(s, StatusCode::OK);
    let ops: Vec<String> = v["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|p| p.as_object().unwrap().values().filter_map(|o| o.get("operationId").and_then(Value::as_str).map(str::to_string)))
        .collect();
    let mut dedup = ops.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(ops.len(), dedup.len(), "operationIds must be unique");
    assert!(v["paths"]["/api/projects/{p}/issues"]["post"]["requestBody"]["content"]["text/plain"].is_object());
}

async fn mcp(env: &Env, token: &str, body: Value) -> (StatusCode, Value) {
    call(env, "POST", "/mcp", token, "application/json", body.to_string()).await
}

#[tokio::test]
async fn mcp_tools_work_for_agent_runs() {
    let env = setup().await;
    let i = new_issue(&env, "Work", IssueState::Triage).await;
    let tok = agent_token(&env, i.id, "triage").await;
    let (s, v) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["result"]["serverInfo"]["name"], "agent-kanban");
    let (s, _) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (_, v) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"board_file_issue") && names.contains(&"board_submit_review"));

    // File a side bug.
    let (_, v) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "board_file_issue", "arguments": {"title": "Side bug", "body": "found it"}}})).await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let created: Value = serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(created["state"], "triage");

    // Invalid move reports the allowed states as a tool error (not a protocol error).
    let (_, v) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "board_move_issue", "arguments": {"to": "in_review"}}})).await;
    assert_eq!(v["result"]["isError"], true);
    assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("Allowed: backlog, ready, closed"));

    // Valid move of the run's own issue.
    let (_, v) = mcp(&env, &tok, json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "board_move_issue", "arguments": {"to": "ready", "comment": "Reproduced."}}})).await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert_eq!(issue(&env, i.number).await.state, IssueState::Ready);

    // Humans (non-run tokens) can't use the tools; unauthenticated requests are rejected.
    let admin = env.app.config.secrets.admin_token.clone();
    let (_, v) = mcp(&env, &admin, json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "board_current_run", "arguments": {}}})).await;
    assert!(v["error"].is_object());
    let (s, _) = mcp(&env, "nope", json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dedup_flow() {
    let env = setup().await;
    let orig = new_issue(&env, "Empty collection literal rejected as method argument", IssueState::Ready).await;
    let other = new_issue(&env, "Unrelated", IssueState::Ready).await;
    let dup = new_issue(&env, "Passing an empty collection literal argument fails", IssueState::Triage).await;
    let tok = agent_token(&env, dup.id, "triage").await;

    // Filing a similar bug returns likely duplicates.
    let (s, v) = call(&env, "POST", "/api/projects/demo/issues", &tok, "text/plain", "Empty collection literal argument rejected\nrepro".into()).await;
    assert_eq!(s, StatusCode::CREATED);
    let dups: Vec<i64> = v["possible_duplicates"].as_array().unwrap().iter().map(|d| d["number"].as_i64().unwrap()).collect();
    assert!(dups.contains(&orig.number), "{v}");
    assert!(!dups.contains(&other.number));

    // Triage may group other open issues under an umbrella (parent/labels only).
    let (s, _) = call(&env, "PATCH", &format!("/api/projects/demo/issues/{}", orig.number), &tok, "application/json", json!({"parent": other.number}).to_string()).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&env, "PATCH", &format!("/api/projects/demo/issues/{}", orig.number), &tok, "application/json", json!({"title": "x"}).to_string()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Closing as a duplicate copies the report to the original.
    let (s, v) = call(&env, "POST", &format!("/api/projects/demo/issues/{}/transition", dup.number), &tok, "application/json", json!({"to": "closed", "duplicate_of": orig.number}).to_string()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["close_reason"], "duplicate");
    let (_, o) = call(&env, "GET", &format!("/api/projects/demo/issues/{}", orig.number), &tok, "application/json", String::new()).await;
    assert!(o["comments"].as_array().unwrap_or_else(|| panic!("{o}")).iter().any(|c| c["body"].as_str().unwrap().contains("closed as a duplicate of this issue")));
}

#[tokio::test]
async fn claude_container_token_flow() {
    let env = setup().await;
    let admin = env.app.config.secrets.admin_token.clone();
    let (_, v) = call(&env, "GET", "/api/credentials", &admin, "application/json", String::new()).await;
    assert_eq!(v["claude_token"], false);
    let (s, _) = call(&env, "PUT", "/api/credentials/claude-token", &admin, "application/json", json!({"token": "not a token"}).to_string()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // No project uses containers, so nothing to verify against: saved directly.
    let (s, v) = call(&env, "PUT", "/api/credentials/claude-token", &admin, "application/json", json!({"token": "sk-ant-oat01-test"}).to_string()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["saved"], true);
    let (_, v) = call(&env, "GET", "/api/credentials", &admin, "application/json", String::new()).await;
    assert_eq!(v["claude_token"], true);
    assert!(!v.to_string().contains("sk-ant-oat01-test"), "the secret is never returned");
    let saved = std::fs::read_to_string(env.app.config.data_dir.join("secrets.toml")).unwrap();
    assert!(saved.contains("sk-ant-oat01-test") && saved.contains(&admin), "admin token preserved");
    // Agents can't set credentials.
    let i = new_issue(&env, "Work", IssueState::Ready).await;
    let tok = agent_token(&env, i.id, "fix").await;
    let (s, _) = call(&env, "PUT", "/api/credentials/claude-token", &tok, "application/json", json!({"token": "sk-ant-x"}).to_string()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&env, "DELETE", "/api/credentials/claude-token", &admin, "application/json", String::new()).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, v) = call(&env, "GET", "/api/credentials", &admin, "application/json", String::new()).await;
    assert_eq!(v["claude_token"], false);
}

#[tokio::test]
async fn attachment_upload_and_download() {
    let env = setup().await;
    let admin = env.app.config.secrets.admin_token.clone();
    let png = base64::engine::general_purpose::STANDARD.decode(TINY_PNG_B64).unwrap();

    // Reject non-image bytes.
    let (s, _, body) = call_bytes(&env, "POST", "/api/projects/demo/attachments", &admin, "application/octet-stream", b"not an image".to_vec()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&body));

    // Reject oversized uploads (magic bytes are valid, but padded past the 10 MiB cap).
    let mut oversized = png.clone();
    oversized.extend(std::iter::repeat_n(0u8, 10 * 1024 * 1024 + 1 - oversized.len()));
    let (s, _, _) = call_bytes(&env, "POST", "/api/projects/demo/attachments", &admin, "image/png", oversized).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Upload succeeds and sniffs the real content type regardless of the request's Content-Type.
    let (s, _, body) = call_bytes(&env, "POST", "/api/projects/demo/attachments", &admin, "application/octet-stream", png.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["content_type"], "image/png");
    assert_eq!(v["byte_size"], png.len());
    let url = v["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/projects/demo/attachments/"), "{url}");
    assert_eq!(v["markdown"], format!("![]({url})"));

    // Download requires auth.
    let (s, _, _) = call_bytes(&env, "GET", &url, "nope", "", vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Download returns the exact bytes with a sniffed, nosniff-guarded content type.
    let (s, headers, body) = call_bytes(&env, "GET", &url, &admin, "", vec![]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, png);
    assert_eq!(headers.get("content-type").unwrap(), "image/png");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");

    // An unknown filename 404s rather than leaking a path-traversal read.
    let (s, _, _) =
        call_bytes(&env, "GET", "/api/projects/demo/attachments/../../../../etc/passwd", &admin, "", vec![]).await;
    assert!(s == StatusCode::NOT_FOUND || s == StatusCode::BAD_REQUEST, "{s}");

    // A fix agent scoped to this project can also upload (e.g. attaching a screenshot) and the
    // MCP board_attach_image tool round-trips to the same REST download endpoint.
    let i = new_issue(&env, "Work", IssueState::InProgress).await;
    let tok = agent_token(&env, i.id, "fix").await;
    let (_, v) = mcp(
        &env,
        &tok,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "board_attach_image", "arguments": {"data_base64": TINY_PNG_B64}}}),
    )
    .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let text: Value = serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let mcp_url = text["url"].as_str().unwrap().to_string();
    let (s, headers, body) = call_bytes(&env, "GET", &mcp_url, &admin, "", vec![]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, png);
    assert_eq!(headers.get("content-type").unwrap(), "image/png");
}
