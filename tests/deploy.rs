//! Rebuild-and-restart-from-the-app tests (issue #12). The real `npm`/`cargo` build steps are
//! replaced with fast fake commands (`Config::build_steps_override`), so these exercise the
//! coordinator (build → restart-on-success-only, dispatch pause/drain) without a real compile.
//! The LaunchAgent hand-off itself can't be exercised from a container; see
//! `deploy::restart_kind` for the pure (and separately unit-tested) decision it makes.

mod common;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use agent_kanban::deploy::{self, BuildStep};
use agent_kanban::domain::models::Project;
use agent_kanban::orchestrator::scheduler;
use agent_kanban::{AppState, api};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::git;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn init_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.name", "Test"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo.join("f.txt"), "one\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "one"]);
}

async fn setup(build: impl FnOnce(&Path) -> Vec<BuildStep>) -> (AppState, Project, tempfile::TempDir) {
    setup_with(build, |_| {}).await
}

/// Like `setup`, but lets the caller tweak `Config` further, e.g. to set `restart_artifact`.
async fn setup_with(build: impl FnOnce(&Path) -> Vec<BuildStep>, customize: impl FnOnce(&mut agent_kanban::config::Config)) -> (AppState, Project, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let steps = build(&repo);
    let app = AppState::for_test_with(&tmp.path().join("data"), "http://127.0.0.1:0", |c| {
        c.repo_dir = repo.clone();
        c.build_steps_override = Some(steps);
        // Tests need the default artifact path (`repo_dir/target/release/...`) to be
        // deterministic regardless of whatever `CARGO_TARGET_DIR` this test binary itself
        // happened to be built with.
        c.cargo_target_dir = None;
        customize(c);
    })
    .await
    .unwrap();
    app.start_background().await.unwrap();
    let project = api::projects::create_project(
        &app,
        api::projects::NewProject {
            slug: "demo".into(),
            name: None,
            repo_path: repo.to_string_lossy().into(),
            base_branch: Some("main".into()),
            github_repo: None,
            github_project_owner: None,
            github_project_number: None,
        },
    )
    .await
    .unwrap();
    (app, project, tmp)
}

/// A build step that behaves like a real `cargo build --release`: it succeeds *and* leaves a
/// binary at the default (no `restart_artifact` override) build artifact path, so the post-build
/// artifact check (`deploy::run_build_job`) has something to find, just as it would for a real
/// build placing output under `repo_dir/target/release/`.
fn ok_step(cwd: &Path) -> BuildStep {
    let bin = env!("CARGO_PKG_NAME");
    BuildStep {
        label: "build".into(),
        program: "sh".into(),
        args: vec!["-c".into(), format!("mkdir -p target/release && printf '#!/bin/sh\\necho ok\\n' > target/release/{bin} && chmod +x target/release/{bin}")],
        cwd: cwd.into(),
        cargo_json: false,
    }
}

/// A build step that succeeds without producing any artifact — used to exercise the case where a
/// "successful" build still leaves nothing to restart into.
fn ok_step_no_artifact(cwd: &Path) -> BuildStep {
    BuildStep { label: "build".into(), program: "sh".into(), args: vec!["-c".into(), "echo ok".into()], cwd: cwd.into(), cargo_json: false }
}

fn failing_step(cwd: &Path) -> BuildStep {
    BuildStep { label: "build".into(), program: "sh".into(), args: vec!["-c".into(), "echo boom >&2; exit 1".into()], cwd: cwd.into(), cargo_json: false }
}

async fn call(app: &AppState, method: &str, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = api::app(app.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn job_status(app: &AppState, id: i64) -> String {
    sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await.unwrap()
}

async fn wait_job_done(app: &AppState, id: i64) -> String {
    for _ in 0..200 {
        let s = job_status(app, id).await;
        if s == "succeeded" || s == "failed" {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} did not finish");
}

#[tokio::test]
async fn build_status_reports_head_and_dirty_state() {
    let (app, _project, tmp) = setup(|_| vec![]).await;
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "GET", "/api/build-status", &admin, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["head_sha"].as_str().is_some());
    // The running test binary's build SHA has nothing to do with this scratch repo.
    assert_eq!(v["commits_behind"], Value::Null);
    assert_eq!(v["dirty"], false);

    std::fs::write(tmp.path().join("repo").join("f.txt"), "changed\n").unwrap();
    let (_, v) = call(&app, "GET", "/api/build-status", &admin, Value::Null).await;
    assert_eq!(v["dirty"], true);
}

#[tokio::test]
async fn rebuild_requires_a_human() {
    let env = common::setup().await;
    common::fake_agent(&env.app, "f-any", "noop", "fake").await;
    let i = common::new_issue(&env, "work", agent_kanban::domain::IssueState::InProgress).await;
    let tok = format!("akr_{}", agent_kanban::auth::random_token());
    sqlx::query(
        "INSERT INTO agent_runs(project_id, issue_id, role, agent_definition_id, status, token_hash, created_at)
         SELECT ?, ?, 'fix', id, 'running', ?, ? FROM agent_definitions WHERE slug = 'f-any'",
    )
    .bind(env.project.id)
    .bind(i.id)
    .bind(agent_kanban::auth::hash_token(&tok))
    .bind(agent_kanban::db::now())
    .execute(&env.app.db)
    .await
    .unwrap();
    let (s, _) = call(&env.app, "POST", "/api/build-status/rebuild", &tok, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn successful_build_requests_a_restart() {
    let (app, _project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");
    assert!(deploy::should_restart(&app), "a successful build should request a restart");
}

#[tokio::test]
async fn failed_build_never_requests_a_restart_and_leaves_dispatch_unpaused() {
    let (app, _project, _tmp) = setup(|repo| vec![failing_step(repo)]).await;
    let admin = app.config.secrets.admin_token.clone();
    // Drain mode pauses dispatch immediately; a failed build must undo that.
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "drain"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert!(app.deploy.dispatch_paused.load(Ordering::Relaxed), "dispatch pauses as soon as drain is requested");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "failed");
    assert!(!deploy::should_restart(&app), "a failed build must not restart the server");
    assert!(!app.deploy.dispatch_paused.load(Ordering::Relaxed), "a failed build clears the drain pause");
}

#[tokio::test]
async fn drain_mode_waits_for_active_runs_before_restarting() {
    let (app, project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    common::fake_agent(&app, "f-any", "noop", "fake").await;
    let agent_id: i64 = sqlx::query_scalar("SELECT id FROM agent_definitions WHERE slug = 'f-any'").fetch_one(&app.db).await.unwrap();
    // A run "in flight" that the drain restart must wait for.
    let run_id: i64 = sqlx::query_scalar(
        "INSERT INTO agent_runs(project_id, role, agent_definition_id, status, created_at) VALUES (?, 'fix', ?, 'running', ?) RETURNING id",
    )
    .bind(project.id)
    .bind(agent_id)
    .bind(agent_kanban::db::now())
    .fetch_one(&app.db)
    .await
    .unwrap();

    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "drain"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");

    // The build succeeded, but the run is still active: no restart yet.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!deploy::should_restart(&app), "must wait for the active run before restarting");
    assert!(app.deploy.dispatch_paused.load(Ordering::Relaxed));

    sqlx::query("UPDATE agent_runs SET status = 'succeeded' WHERE id = ?").bind(run_id).execute(&app.db).await.unwrap();

    for _ in 0..100 {
        if deploy::should_restart(&app) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(deploy::should_restart(&app), "restarts once the active run finishes");
}

#[tokio::test]
async fn commits_behind_uses_local_main_not_whatever_is_checked_out() {
    let (app, _project, tmp) = setup(|_| vec![]).await;
    let repo = tmp.path().join("repo");
    let admin = app.config.secrets.admin_token.clone();

    // Detach from `main` and diverge: a checkout in this state isn't what a rebuild should be
    // trusted to build, so the status must say so instead of quietly reporting a stale count
    // against whatever HEAD happens to be.
    git(&repo, &["checkout", "-q", "--detach"]);
    std::fs::write(repo.join("f.txt"), "detached\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "detached work"]);
    let (_, v) = call(&app, "GET", "/api/build-status", &admin, Value::Null).await;
    assert_eq!(v["on_main"], false, "checkout is detached, not on main");

    // Back on `main`, advance it further: `head_sha` must track `main`'s tip, not this checkout's
    // (now stale) HEAD before the checkout is fast-forwarded.
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("f.txt"), "on main\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "advance main"]);
    let main_sha = git(&repo, &["rev-parse", "main"]);
    let (_, v) = call(&app, "GET", "/api/build-status", &admin, Value::Null).await;
    assert_eq!(v["on_main"], true);
    assert_eq!(v["head_sha"], main_sha);
}

#[tokio::test]
async fn commits_behind_is_unknown_on_diverged_history() {
    let (app, _project, tmp) = setup(|_| vec![]).await;
    let repo = tmp.path().join("repo");
    let admin = app.config.secrets.admin_token.clone();
    // Rewrite `main`'s history so it no longer contains the running binary's (unrelated) build
    // SHA as an ancestor at all: `commits_behind` must not report a rev-list count across
    // diverged history as if it were a linear "behind" count.
    git(&repo, &["commit", "--amend", "-q", "-m", "rewritten"]);
    let (_, v) = call(&app, "GET", "/api/build-status", &admin, Value::Null).await;
    assert_eq!(v["commits_behind"], Value::Null);
}

#[tokio::test]
async fn rebuild_refuses_a_checkout_that_is_not_on_main() {
    let (app, _project, tmp) = setup(|repo| vec![ok_step(repo)]).await;
    let repo = tmp.path().join("repo");
    git(&repo, &["checkout", "-q", "--detach"]);
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "failed", "must refuse to build a detached checkout");
    assert!(!deploy::should_restart(&app));
}

#[tokio::test]
async fn stderr_output_is_not_dropped_when_stdout_closes_first() {
    // A step that closes stdout immediately but keeps writing to stderr must not lose that
    // output (e.g. a compiler error) or hang: both streams have to be drained to EOF.
    let (app, _project, _tmp) = setup(|repo| {
        vec![BuildStep {
            label: "build".into(),
            program: "sh".into(),
            args: vec!["-c".into(), "exec 1>&-; for i in $(seq 1 200); do echo \"err $i\" >&2; done; exit 1".into()],
            cwd: repo.into(),
            cargo_json: false,
        }]
    })
    .await;
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "failed");
    let log: String = sqlx::query_scalar("SELECT log FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await.unwrap();
    assert!(log.contains("err 200"), "stderr output must be fully drained even though stdout closed first:\n{log}");
}

#[tokio::test]
async fn restart_notification_follows_a_durably_succeeded_job() {
    let (app, _project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    let admin = app.config.secrets.admin_token.clone();

    // Register interest before triggering the build: `Notify::notify_waiters` only wakes waiters
    // that are already registered, so this has to be set up first, not raced against.
    let app2 = app.clone();
    let waiter = tokio::spawn(async move { app2.deploy.notify.notified().await });
    tokio::task::yield_now().await;

    let (_, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    let id = v["id"].as_i64().unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("restart was never requested").unwrap();
    assert_eq!(job_status(&app, id).await, "succeeded", "the restart notification must fire only after the job is durably terminal");
}

#[tokio::test]
async fn concurrent_rebuild_requests_are_rejected() {
    let (app, _project, _tmp) = setup(|repo| {
        let bin = env!("CARGO_PKG_NAME");
        vec![BuildStep {
            label: "build".into(),
            program: "sh".into(),
            args: vec!["-c".into(), format!("sleep 0.3 && mkdir -p target/release && printf '#!/bin/sh\\necho ok\\n' > target/release/{bin} && chmod +x target/release/{bin}")],
            cwd: repo.into(),
            cargo_json: false,
        }]
    })
    .await;
    let admin = app.config.secrets.admin_token.clone();
    let (s1, v1) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s1, StatusCode::ACCEPTED, "{v1}");
    let (s2, v2) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "drain"})).await;
    assert_eq!(s2, StatusCode::CONFLICT, "a second rebuild must be rejected while one is in flight: {v2}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'server.build'").fetch_one(&app.db).await.unwrap();
    assert_eq!(count, 1, "the rejected request must not enqueue a second build job");

    let id1 = v1["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id1).await, "succeeded");
}

#[tokio::test]
async fn rebuild_is_rejected_while_a_drain_restart_is_pending() {
    let (app, project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    common::fake_agent(&app, "f-any", "noop", "fake").await;
    let agent_id: i64 = sqlx::query_scalar("SELECT id FROM agent_definitions WHERE slug = 'f-any'").fetch_one(&app.db).await.unwrap();
    sqlx::query(
        "INSERT INTO agent_runs(project_id, role, agent_definition_id, status, created_at) VALUES (?, 'fix', ?, 'running', ?)",
    )
    .bind(project.id)
    .bind(agent_id)
    .bind(agent_kanban::db::now())
    .execute(&app.db)
    .await
    .unwrap();

    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "drain"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");

    // The build succeeded but the active run keeps it waiting to restart; a second request must
    // still be rejected while that wait is pending.
    let (s2, v2) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s2, StatusCode::CONFLICT, "{v2}");
}

#[tokio::test]
async fn start_run_is_blocked_while_dispatch_is_paused() {
    let env = common::setup().await;
    common::fake_agent(&env.app, "f-fix", "fix", "fake").await;
    let i = common::new_issue(&env, "work", agent_kanban::domain::IssueState::Ready).await;
    let a = common::agent(&env, "f-fix").await;

    deploy::begin_drain(&env.app).await;
    let res = scheduler::start_run(&env.app, &env.project, &i, agent_kanban::domain::Role::Fix, &a).await;
    assert!(res.is_err(), "a manual start must be refused once a drain restart is pending, not just a scheduler tick");
}

#[tokio::test]
async fn dispatch_gate_prevents_the_drain_restart_from_missing_a_run_started_just_before_it() {
    let (app, project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    // "hang" keeps the run active until cancelled, so it can't race to completion on its own.
    common::fake_agent(&app, "f-hang", "hang", "fake").await;
    let agent: agent_kanban::domain::models::AgentDefinition =
        sqlx::query_as("SELECT * FROM agent_definitions WHERE slug = 'f-hang'").fetch_one(&app.db).await.unwrap();
    let issue_id: i64 = sqlx::query_scalar(
        "INSERT INTO issues(project_id, number, title, body, state, source, rank, created_at, updated_at)
         VALUES (?, 1, 't', '', 'ready', 'human', 0, ?, ?) RETURNING id",
    )
    .bind(project.id)
    .bind(agent_kanban::db::now())
    .bind(agent_kanban::db::now())
    .fetch_one(&app.db)
    .await
    .unwrap();
    let issue: agent_kanban::domain::models::Issue =
        sqlx::query_as("SELECT * FROM issues WHERE id = ?").bind(issue_id).fetch_one(&app.db).await.unwrap();

    // Simulate a scheduler tick that already found `dispatch_paused == false` and is mid-flight
    // inserting a run, right as a drain restart begins and (moments later) reaches "zero active
    // runs": hold the gate as that in-flight `start_run` would, so a concurrent drain can't
    // interleave its count-and-decide with the insert.
    let guard = app.deploy.dispatch_gate.lock().await;
    let start = {
        let app = app.clone();
        let project = project.clone();
        let issue = issue.clone();
        let agent = agent.clone();
        tokio::spawn(async move { scheduler::start_run(&app, &project, &issue, agent_kanban::domain::Role::Fix, &agent).await })
    };
    tokio::task::yield_now().await;

    let admin = app.config.secrets.admin_token.clone();
    let rebuild = {
        let app = app.clone();
        tokio::spawn(async move { call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "drain"})).await })
    };
    tokio::task::yield_now().await;
    drop(guard);

    let started = start.await.unwrap().expect("start_run was already in flight before the drain began; it must win the gate and succeed");
    let (s, v) = rebuild.await.unwrap();
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");

    // The run started just before the drain began must be waited on, not missed by the restart
    // decision because its insert hadn't landed yet when the count was taken.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!deploy::should_restart(&app), "the drain restart must not miss a run that started just before it began");

    app.runs.cancel_and_wait(started.id, "test cleanup", Duration::from_secs(5)).await;
    for _ in 0..100 {
        if deploy::should_restart(&app) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(deploy::should_restart(&app), "restarts once the run finishes");
}

#[tokio::test]
async fn a_missing_build_artifact_fails_the_job_and_never_restarts() {
    // A build step that reports success but (unlike `ok_step`) leaves nothing at the default
    // artifact path: the job must fail rather than let `jobs::run_job` mark it succeeded and
    // request a restart into a binary that doesn't exist.
    let (app, _project, _tmp) = setup(|repo| vec![ok_step_no_artifact(repo)]).await;
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "failed", "a build with no artifact to restart into must not be reported as succeeded");
    assert!(!deploy::should_restart(&app), "must not request a restart when no artifact was produced");
    let log: String = sqlx::query_scalar("SELECT log FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await.unwrap();
    assert!(log.contains("no artifact"), "log should explain the failure:\n{log}");
}

#[tokio::test]
async fn a_configured_cargo_output_directory_is_resolved_from_build_json_not_guessed() {
    // Reproduces thread #101 (review run #191): a stale binary sitting at the guessed default
    // `repo_dir/target/release/<bin>` path must not be restarted into when Cargo actually placed
    // the fresh build somewhere else (e.g. a `.cargo/config.toml` `target-dir` override, or a
    // configured target triple) — only `cargo build`'s own JSON output can tell where it wrote.
    use std::os::unix::fs::PermissionsExt;
    let bin = env!("CARGO_PKG_NAME");
    let mut fresh_path = None;
    let (app, project, _tmp) = setup(|repo| {
        std::fs::create_dir_all(repo.join("target/release")).unwrap();
        std::fs::write(repo.join("target/release").join(bin), "stale\n").unwrap();

        let custom = repo.join("custom-target").join("release");
        std::fs::create_dir_all(&custom).unwrap();
        let fresh = custom.join(bin);
        std::fs::write(&fresh, "#!/bin/sh\necho fresh\n").unwrap();
        std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755)).unwrap();
        fresh_path = Some(fresh.clone());

        vec![BuildStep {
            label: "cargo build --release".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!(r#"echo '{{"reason":"compiler-artifact","target":{{"kind":["bin"],"name":"{bin}"}},"executable":"{}"}}'"#, fresh.display()),
            ],
            cwd: repo.into(),
            cargo_json: true,
        }]
    })
    .await;
    let fresh_path = fresh_path.unwrap();
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");
    assert!(deploy::should_restart(&app));

    let resolved = app.deploy.last_build_artifact.lock().unwrap().clone();
    assert_eq!(resolved, Some(fresh_path), "must restart into what Cargo's own JSON output reported, not the stale default-path guess");
    assert_eq!(std::fs::read_to_string(Path::new(&project.repo_path).join("target/release").join(bin)).unwrap(), "stale\n", "the stale default-path file must be left untouched");
}

#[tokio::test]
async fn a_configured_restart_artifact_is_installed_from_the_fresh_build() {
    // `restart_artifact` (`AKB_BIN`) is a deployment destination Cargo itself never writes to;
    // seed it with a stale "previous deployment" and confirm a successful build replaces it with
    // the binary the build step just produced, rather than restarting into the old bytes.
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("deployed").join(env!("CARGO_PKG_NAME"));
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, "old binary\n").unwrap();
    let dest2 = dest.clone();

    let (app, _project, _tmp2) = setup_with(|repo| vec![ok_step(repo)], move |c| c.restart_artifact = Some(dest2.clone())).await;
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");
    assert!(deploy::should_restart(&app));

    let installed = std::fs::read_to_string(&dest).unwrap();
    assert_ne!(installed, "old binary\n", "AKB_BIN must be overwritten with the binary the build just produced");
    assert!(installed.contains("echo ok"), "installed file should be the one the build step wrote:\n{installed}");
}

#[tokio::test]
async fn a_persistence_failure_never_requests_a_restart() {
    let (app, _project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    // Simulate the job's terminal DB write failing (e.g. a full disk) right as it would mark the
    // build durably succeeded: the UPDATE that flips status to 'succeeded' is blocked, so
    // `run_job` sees a build that succeeded but couldn't be durably recorded.
    sqlx::query(
        "CREATE TRIGGER fail_on_succeeded BEFORE UPDATE OF status ON jobs \
         WHEN NEW.status = 'succeeded' BEGIN SELECT RAISE(FAIL, 'simulated persistence failure'); END",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();

    // The trigger blocks only the 'succeeded' write (not every write to the row), so `run_job`'s
    // fallback attempt to mark the job 'failed' instead isn't blocked and should land, giving the
    // page a normal, retryable job to look at instead of one stuck `running` forever (thread #122
    // on issue #12, review run #199).
    assert_eq!(wait_job_done(&app, id).await, "failed", "the fallback 'failed' write isn't blocked by a trigger scoped to 'succeeded'");
    let error: Option<String> = sqlx::query_scalar("SELECT error FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await.unwrap();
    assert!(error.as_deref().is_some_and(|e| e.contains("durably recorded")), "got {error:?}");
    assert!(!deploy::should_restart(&app), "a build that couldn't durably persist its success must not restart the server");
    assert!(!app.deploy.deploying.load(Ordering::Relaxed), "deploy state must be released so another rebuild can be requested");
    assert!(!app.deploy.dispatch_paused.load(Ordering::Relaxed));
    let status = deploy::status(&app).await;
    assert!(status.deploy_error.as_deref().is_some_and(|e| e.contains("durably recorded")), "BuildStatus should also expose the failure, got {:?}", status.deploy_error);
}

#[tokio::test]
async fn a_totally_unrecordable_completion_still_surfaces_via_build_status() {
    // The harder case from thread #122 (review run #199): not just the 'succeeded' write but
    // *every* terminal write to the row is blocked (a genuinely down database, as opposed to a
    // constraint on one particular value), so `run_job`'s fallback 'failed' write can't land
    // either and the row is stuck `running` with no error forever. `BuildStatus::deploy_error` is
    // in-memory, not a DB write, so it's the one place this failure can still reach the page that
    // triggered it.
    let (app, _project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    sqlx::query(
        "CREATE TRIGGER fail_on_terminal BEFORE UPDATE OF status ON jobs \
         WHEN NEW.status IN ('succeeded', 'failed') BEGIN SELECT RAISE(FAIL, 'simulated persistence failure'); END",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();

    // Neither terminal write can land: give the worker a moment to (fail to) get there rather than
    // waiting on `wait_job_done`, which would time out.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(job_status(&app, id).await, "running", "both the 'succeeded' and fallback 'failed' writes are blocked by the trigger");
    assert!(!deploy::should_restart(&app), "must not restart when a completion failure couldn't be recorded at all");
    assert!(!app.deploy.deploying.load(Ordering::Relaxed), "deploy state must be released so another rebuild can be requested");
    assert!(!app.deploy.dispatch_paused.load(Ordering::Relaxed));

    let status = deploy::status(&app).await;
    assert!(
        status.deploy_error.as_deref().is_some_and(|e| e.contains("durably recorded")),
        "BuildStatus must expose the failure even though the job row is stuck `running`, got {:?}",
        status.deploy_error
    );
}

#[tokio::test]
async fn a_log_persistence_failure_fails_the_job_and_never_restarts() {
    // Reproduces thread #122 (review run #191): the terminal status write doesn't touch the `log`
    // column, so it can succeed even when every log write failed throughout the build. Marking the
    // job `succeeded` in that case would strand it: a `succeeded` job is never retried (only
    // `running` ones are requeued at startup) and no restart would ever be requested, leaving
    // whoever triggered it waiting forever with no error and no way to retry.
    let (app, _project, _tmp) = setup(|repo| vec![ok_step(repo)]).await;
    sqlx::query("CREATE TRIGGER fail_on_log BEFORE UPDATE OF log ON jobs BEGIN SELECT RAISE(FAIL, 'simulated log persistence failure'); END")
        .execute(&app.db)
        .await
        .unwrap();

    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();

    assert_eq!(wait_job_done(&app, id).await, "failed", "a build whose log couldn't be saved must not be reported as succeeded");
    assert!(!deploy::should_restart(&app), "must not restart when the build's log couldn't be durably recorded");
    assert!(!app.deploy.deploying.load(Ordering::Relaxed), "deploy state must be released so another rebuild can be requested");
    assert!(!app.deploy.dispatch_paused.load(Ordering::Relaxed));
    let error: Option<String> = sqlx::query_scalar("SELECT error FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await.unwrap();
    assert!(error.as_deref().is_some_and(|e| e.contains("log")), "the job's error should explain the log-write failure, got {error:?}");
}

#[tokio::test]
async fn startup_recovery_restores_deploy_guards_for_a_requeued_build() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let app = agent_kanban::AppState::for_test_with(&tmp.path().join("data"), "http://127.0.0.1:0", |c| {
        c.repo_dir = repo.clone();
        c.build_steps_override = Some(vec![ok_step(&repo)]);
        c.cargo_target_dir = None;
    })
    .await
    .unwrap();

    // A `server.build` job in `drain` mode that was mid-flight when the process was interrupted:
    // found `running` on this fresh startup, exactly as `spawn_worker`'s own reset would find it,
    // while `DeployState` (in-memory, not persisted) is back at its just-started defaults.
    sqlx::query("INSERT INTO jobs(kind, status, payload, attempts, created_at) VALUES ('server.build', 'running', '{\"mode\":\"drain\"}', 1, ?)")
        .bind(agent_kanban::db::now())
        .execute(&app.db)
        .await
        .unwrap();

    // `spawn_worker` must fully restore the guards *before* it returns: a caller (in production,
    // `AppState::start_background`, before the scheduler starts or the server accepts any
    // requests) that admitted a new rebuild or dispatched a run right after this `.await` returns
    // must never be able to race the recovery itself (thread #118 on issue #12) — so this asserts
    // the guards immediately, with no polling loop.
    agent_kanban::jobs::spawn_worker(app.clone()).await.unwrap();
    assert!(app.deploy.deploying.load(Ordering::Relaxed), "a recovered in-flight build must re-reserve the deploy pipeline");
    assert!(app.deploy.dispatch_paused.load(Ordering::Relaxed), "a recovered drain-mode build must re-pause dispatch");

    // A concurrent rebuild request must be rejected exactly like it would've been before the
    // interruption.
    let admin = app.config.secrets.admin_token.clone();
    let (s, v) = call(&app, "POST", "/api/build-status/rebuild", &admin, json!({"mode": "now"})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // And the requeued job itself should still run to completion normally. It's still in `drain`
    // mode, and there are no active runs, so the restart follows once the drain loop notices.
    let id: i64 = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'server.build'").fetch_one(&app.db).await.unwrap();
    assert_eq!(wait_job_done(&app, id).await, "succeeded");
    for _ in 0..100 {
        if deploy::should_restart(&app) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(deploy::should_restart(&app));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_paused_blocks_the_scheduler() {
    let env = common::setup().await;
    common::fake_agent(&env.app, "f-fix", "fix", "fake").await;
    sqlx::query(
        "INSERT OR REPLACE INTO project_role_agents(project_id, role, agent_definition_id) SELECT ?, 'fix', id FROM agent_definitions WHERE slug = 'f-fix'",
    )
    .bind(env.project.id)
    .execute(&env.app.db)
    .await
    .unwrap();
    let i = common::new_issue(&env, "work", agent_kanban::domain::IssueState::Ready).await;

    env.app.deploy.dispatch_paused.store(true, Ordering::Relaxed);
    scheduler::tick(&env.app).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE issue_id = ?").bind(i.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(n, 0, "a paused dispatch must not start new runs");

    env.app.deploy.dispatch_paused.store(false, Ordering::Relaxed);
    scheduler::tick(&env.app).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE issue_id = ?").bind(i.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(n, 1, "resumes dispatching once unpaused");
}
