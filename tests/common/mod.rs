#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use agent_kanban::AppState;
use agent_kanban::domain::models::{AgentRun, Issue, Project};
use agent_kanban::domain::{Actor, IssueState, Role};
use agent_kanban::services;
use serde_json::json;

pub struct Env {
    pub app: AppState,
    pub project: Project,
    pub repo: PathBuf,
    pub url: String,
    _tmp: tempfile::TempDir,
}

pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub async fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "master"]);
    // The server commits too (merges): don't depend on the machine having a git identity.
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo.join("main.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(repo.join("fix.txt"), "start\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "🌱 Initial"]);
    let repo = repo.canonicalize().unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = AppState::for_test(&tmp.path().join("data"), &url).await.unwrap();
    let router = agent_kanban::api::app(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let project = agent_kanban::api::projects::create_project(
        &app,
        agent_kanban::api::projects::NewProject {
            slug: "demo".into(),
            name: None,
            repo_path: repo.to_string_lossy().into(),
            base_branch: Some("master".into()),
            github_repo: None,
            github_project_owner: None,
            github_project_number: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET commit_msg_regex = '^\\p{Extended_Pictographic}' WHERE id = ?")
        .bind(project.id)
        .execute(&app.db)
        .await
        .unwrap();
    let project = services::project(&app.db, "demo").await.unwrap();
    Env { app, project, repo, url, _tmp: tmp }
}

/// Register a fake agent with a scripted behaviour.
pub async fn fake_agent(app: &AppState, slug: &str, mode: &str, group: &str) {
    fake_agent_env(app, slug, mode, group, json!({})).await
}

/// Like [`fake_agent`], with extra environment variables merged in (e.g. `FAKE_LOAD_SESSION`).
pub async fn fake_agent_env(app: &AppState, slug: &str, mode: &str, group: &str, extra_env: serde_json::Value) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.py");
    let now = agent_kanban::db::now();
    let mut env = extra_env;
    env["FAKE_MODE"] = json!(mode);
    sqlx::query(
        "INSERT OR REPLACE INTO agent_definitions(slug, name, harness, command, args, env, limit_group, max_concurrent,
            permission_policy, enabled, needs_auth, created_at, updated_at)
         VALUES (?, ?, 'custom', 'python3', ?, ?, ?, 5, 'auto_allow', 1, 0, ?, ?)",
    )
    .bind(slug)
    .bind(slug)
    .bind(json!([script]).to_string())
    .bind(env.to_string())
    .bind(group)
    .bind(&now)
    .bind(&now)
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT OR IGNORE INTO limit_groups(name, updated_at) VALUES (?, ?)")
        .bind(group)
        .bind(&now)
        .execute(&app.db)
        .await
        .unwrap();
}

pub async fn new_issue(env: &Env, title: &str, state: IssueState) -> Issue {
    let c = services::issues::create(
        &env.app,
        &env.project,
        &Actor::human("tester"),
        services::issues::NewIssue { title: title.into(), body: "details".into(), state: Some(state), ..Default::default() },
    )
    .await
    .unwrap();
    services::issue(&env.app.db, env.project.id, c.number).await.unwrap()
}

pub async fn issue(env: &Env, n: i64) -> Issue {
    services::issue(&env.app.db, env.project.id, n).await.unwrap()
}

pub async fn agent(env: &Env, slug: &str) -> agent_kanban::domain::models::AgentDefinition {
    sqlx::query_as("SELECT * FROM agent_definitions WHERE slug = ?").bind(slug).fetch_one(&env.app.db).await.unwrap()
}

/// Start a run with a given fake agent and wait for it to finish.
pub async fn run(env: &Env, n: i64, role: Role, agent_slug: &str) -> AgentRun {
    let i = issue(env, n).await;
    let a = agent(env, agent_slug).await;
    let r = agent_kanban::orchestrator::scheduler::start_run(&env.app, &env.project, &i, role, &a).await.unwrap();
    wait_run(env, r.id).await
}

pub async fn wait_run(env: &Env, id: i64) -> AgentRun {
    for _ in 0..600 {
        let r: AgentRun = sqlx::query_as("SELECT * FROM agent_runs WHERE id = ?").bind(id).fetch_one(&env.app.db).await.unwrap();
        if !r.is_active() && !env.app.runs.is_live(id) {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("run {id} did not finish");
}

pub async fn transcript(env: &Env, id: i64) -> String {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, payload FROM run_events WHERE run_id = ? ORDER BY seq").bind(id).fetch_all(&env.app.db).await.unwrap();
    rows.into_iter().map(|(k, p)| format!("[{k}] {p}")).collect::<Vec<_>>().join("\n")
}
