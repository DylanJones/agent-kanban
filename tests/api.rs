//! HTTP-level tests: auth scoping, quick bug filing, error shapes.

mod common;

use agent_kanban::auth::hash_token;
use agent_kanban::domain::IssueState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

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
