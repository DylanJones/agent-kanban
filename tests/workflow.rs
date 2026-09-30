//! End-to-end workflow tests: a scripted ACP agent drives the board through the REST API.

mod common;

use agent_kanban::domain::{Actor, Hold, IssueState, Role};
use agent_kanban::orchestrator::scheduler;
use agent_kanban::services;
use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn triage_fix_review_merge() {
    let env = setup().await;
    fake_agent(&env.app, "f-triage", "triage", "fake").await;
    fake_agent(&env.app, "f-fix", "fix_and_bug", "fake").await;
    fake_agent(&env.app, "f-fix2", "fix", "fake").await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    fake_agent(&env.app, "f-approve", "review_approve", "fake").await;
    let i = new_issue(&env, "Crash on empty input", IssueState::Triage).await;

    // Triage.
    let r = run(&env, i.number, Role::Triage, "f-triage").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    assert_eq!(issue(&env, i.number).await.state, IssueState::Ready);
    // The triage worktree is ephemeral.
    assert!(!std::path::Path::new(r.worktree_path.as_ref().unwrap()).exists());

    // Fix: commits, files a side bug, opens a PR, moves to review.
    let r = run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    let cur = issue(&env, i.number).await;
    assert_eq!(cur.state, IssueState::InReview);
    let branch = cur.branch_name.clone().unwrap();
    assert!(branch.starts_with("agent/issue-1-crash-on-empty-input"), "{branch}");
    let pr = services::pulls::open_pr_for_issue(&env.app.db, cur.id).await.unwrap().unwrap();
    assert_eq!(pr.review_requested_sha, pr.head_sha);
    let side: (String, String) = sqlx::query_as("SELECT state, source FROM issues WHERE title = 'Side bug found while fixing'")
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    assert_eq!(side, ("triage".into(), "agent".into()));
    let wt = r.worktree_path.clone().unwrap();
    assert!(std::path::Path::new(&wt).exists(), "branch worktree persists across the review cycle");

    // Review requests changes with an inline thread.
    let r = run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);
    assert_eq!(services::pulls::unresolved_blocking(&env.app.db, pr.id).await.unwrap(), 1);

    // Fix agent addresses feedback in the same worktree.
    let r = run(&env, i.number, Role::Fix, "f-fix2").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    assert_eq!(r.worktree_path.as_deref(), Some(wt.as_str()));
    assert_eq!(issue(&env, i.number).await.state, IssueState::InReview);

    // Approval resolves the thread and makes it ready to merge.
    let r = run(&env, i.number, Role::Review, "f-approve").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    assert_eq!(issue(&env, i.number).await.state, IssueState::ReadyToMerge);
    let pr = services::pull_by_id(&env.app.db, pr.id).await.unwrap();
    assert_eq!(pr.approved_sha, pr.head_sha);

    // A human merges with the project default (merge commit): base advances, issue done, worktree and branch cleaned up.
    assert_eq!(env.project.merge_strategy, "merge", "new projects default to a merge commit");
    let res = services::merge::merge(&env.app, &env.project, pr.number, &Actor::human("dylan"), Default::default()).await.unwrap();
    assert_eq!(git(&env.repo, &["rev-parse", "master"]), res.merged_sha);
    assert_eq!(res.strategy, "merge");
    assert_eq!(git(&env.repo, &["rev-list", "--parents", "-n1", "master"]).split(' ').count(), 3, "merge commit has two parents");
    assert!(git(&env.repo, &["log", "-1", "--format=%s", "master"]).starts_with("🐛 Fix the thing"));
    assert!(git(&env.repo, &["log", "-1", "--format=%b", "master"]).contains("Closes #1"));
    assert_eq!(issue(&env, i.number).await.state, IssueState::Done);
    assert!(!std::path::Path::new(&wt).exists(), "worktree removed on done");
    assert!(git(&env.repo, &["branch", "--list", &branch]).is_empty(), "merged branch deleted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_commit_after_approval_returns_to_review() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    fake_agent(&env.app, "f-approve", "review_approve", "fake").await;
    let i = new_issue(&env, "Thing", IssueState::Ready).await;
    run(&env, i.number, Role::Fix, "f-fix").await;
    run(&env, i.number, Role::Review, "f-approve").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ReadyToMerge);
    let wt = format!("{}/worktrees/demo/issue-{}", env.app.config.data_dir.display(), i.number);
    std::fs::write(format!("{wt}/extra.txt"), "x").unwrap();
    git(std::path::Path::new(&wt), &["add", "-A"]);
    git(std::path::Path::new(&wt), &["commit", "-q", "-m", "➕ more"]);
    agent_kanban::git::scanner::scan_all(&env.app).await.unwrap();
    assert_eq!(issue(&env, i.number).await.state, IssueState::InReview);
    let pr = services::pulls::open_pr_for_issue(&env.app.db, i.id).await.unwrap().unwrap();
    assert!(pr.approved_sha.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_conflict_goes_to_merge_prep() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    fake_agent(&env.app, "f-prep", "merge_prep", "fake").await;
    let i = new_issue(&env, "Conflicting", IssueState::Ready).await;
    run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::InReview);
    // Someone lands a conflicting change on master.
    std::fs::write(env.repo.join("fix.txt"), "start\nconflicting line on master\n").unwrap();
    git(&env.repo, &["commit", "-qam", "💥 conflicting"]);
    agent_kanban::git::scanner::scan_all(&env.app).await.unwrap();
    assert_eq!(issue(&env, i.number).await.state, IssueState::MergeConflict);
    let pr = services::pulls::open_pr_for_issue(&env.app.db, i.id).await.unwrap().unwrap();
    assert!(pr.has_conflicts);
    assert_eq!(pr.conflict_files.0, vec!["fix.txt".to_string()]);
    // Scheduler dispatches merge prep for merge_conflict.
    assert_eq!(IssueState::MergeConflict.dispatch_role(), Some(Role::MergePrep));
    let r = run(&env, i.number, Role::MergePrep, "f-prep").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    agent_kanban::git::scanner::scan_all(&env.app).await.unwrap();
    assert_eq!(issue(&env, i.number).await.state, IssueState::InReview);
    let pr = services::pull_by_id(&env.app.db, pr.id).await.unwrap();
    assert!(!pr.has_conflicts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn usage_limit_pauses_group_without_penalty() {
    let env = setup().await;
    fake_agent(&env.app, "f-limit", "limit", "fakesub").await;
    let i = new_issue(&env, "Limited", IssueState::Ready).await;
    let r = run(&env, i.number, Role::Fix, "f-limit").await;
    assert_eq!(r.status, "rate_limited", "{}", transcript(&env, r.id).await);
    let g: (bool, Option<String>, Option<String>) =
        sqlx::query_as("SELECT paused, pause_kind, paused_until FROM limit_groups WHERE name = 'fakesub'")
            .fetch_one(&env.app.db)
            .await
            .unwrap();
    assert!(g.0);
    assert_eq!(g.1.as_deref(), Some("quota"));
    let until = agent_kanban::db::parse_time(&g.2.unwrap()).unwrap();
    let la = until.with_timezone(&chrono_tz::America::Los_Angeles);
    assert_eq!(chrono::Timelike::hour(&la), 17, "{la}");
    let cur = issue(&env, i.number).await;
    assert_eq!(cur.failure_count, 0);
    assert!(cur.hold.is_none());

    // The scheduler does not dispatch to a paused group.
    sqlx::query("INSERT OR REPLACE INTO project_role_agents(project_id, role, agent_definition_id) SELECT ?, 'fix', id FROM agent_definitions WHERE slug = 'f-limit'")
        .bind(env.project.id)
        .execute(&env.app.db)
        .await
        .unwrap();
    scheduler::tick(&env.app).await.unwrap();
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')")
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    assert_eq!(active, 0);

    // Once resumed, work continues.
    agent_kanban::orchestrator::limits::resume(&env.app, "fakesub").await.unwrap();
    scheduler::tick(&env.app).await.unwrap();
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE issue_id = ? AND id > ?")
        .bind(cur.id)
        .bind(r.id)
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    assert_eq!(active, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_outcome_is_nudged_then_failed_with_backoff() {
    let env = setup().await;
    fake_agent(&env.app, "f-noop", "noop", "fake").await;
    let i = new_issue(&env, "Lazy", IssueState::Triage).await;
    let r = run(&env, i.number, Role::Triage, "f-noop").await;
    assert_eq!(r.status, "failed");
    assert_eq!(r.nudges, 1);
    let cur = issue(&env, i.number).await;
    assert_eq!(cur.failure_count, 1);
    assert!(cur.next_attempt_at.is_some());
    // Not eligible again until the backoff elapses.
    scheduler::tick(&env.app).await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE issue_id = ?").bind(cur.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(n, 1);
    // Third failure stalls the issue.
    for _ in 0..2 {
        sqlx::query("UPDATE issues SET next_attempt_at = NULL WHERE id = ?").bind(cur.id).execute(&env.app.db).await.unwrap();
        run(&env, i.number, Role::Triage, "f-noop").await;
    }
    assert_eq!(issue(&env, i.number).await.hold, Some(Hold::Stalled));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_and_backlog_cleanup() {
    let env = setup().await;
    fake_agent(&env.app, "f-hang", "hang", "fake").await;
    let i = new_issue(&env, "Hangs", IssueState::Ready).await;
    let a = agent(&env, "f-hang").await;
    let r = scheduler::start_run(&env.app, &env.project, &i, Role::Fix, &a).await.unwrap();
    // Wait until it's running.
    for _ in 0..100 {
        let s: String = sqlx::query_scalar("SELECT status FROM agent_runs WHERE id = ?").bind(r.id).fetch_one(&env.app.db).await.unwrap();
        if s == "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(issue(&env, i.number).await.state, IssueState::InProgress);
    // Only one active run per issue.
    assert!(scheduler::start_run(&env.app, &env.project, &issue(&env, i.number).await, Role::Fix, &a).await.is_err());
    // A human moves it back to the backlog: run cancelled, worktree removed, branch kept.
    let cur = issue(&env, i.number).await;
    services::issues::set_state(&env.app, &env.project, &cur, IssueState::Backlog, &Actor::human("dylan"), None, None).await.unwrap();
    let r = wait_run(&env, r.id).await;
    assert_eq!(r.status, "cancelled");
    let wt = r.worktree_path.unwrap();
    assert!(!std::path::Path::new(&wt).exists());
    let branch = issue(&env, i.number).await.branch_name.unwrap();
    assert!(!git(&env.repo, &["branch", "--list", &branch]).is_empty(), "unmerged branch is kept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_respects_concurrency_and_priority() {
    let env = setup().await;
    fake_agent(&env.app, "f-hang", "hang", "fake").await;
    sqlx::query("INSERT OR REPLACE INTO project_role_agents(project_id, role, agent_definition_id) SELECT ?, r, id FROM agent_definitions, (SELECT 'fix' AS r UNION SELECT 'triage') WHERE slug = 'f-hang'")
        .bind(env.project.id)
        .execute(&env.app.db)
        .await
        .unwrap();
    agent_kanban::db::set_setting(&env.app.db, "max_concurrent_runs", &2).await.unwrap();
    let low = new_issue(&env, "low", IssueState::Triage).await;
    let a = new_issue(&env, "a", IssueState::Ready).await;
    let b = new_issue(&env, "b", IssueState::Ready).await;
    services::issues::update(
        &env.app,
        &env.project,
        b.number,
        &Actor::human("d"),
        services::issues::IssuePatch { priority: Some("P0".into()), ..Default::default() },
    )
    .await
    .unwrap();
    scheduler::tick(&env.app).await.unwrap();
    let active: Vec<i64> =
        sqlx::query_scalar("SELECT issue_id FROM agent_runs WHERE status IN ('queued','preparing','running') ORDER BY id")
            .fetch_all(&env.app.db)
            .await
            .unwrap();
    assert_eq!(active, vec![b.id, a.id], "P0 first, then fix work before triage; capped at 2");
    assert!(!active.contains(&low.id));
    // Clean up the hanging runs.
    for id in env.app.runs.live_ids() {
        env.app.runs.cancel_and_wait(id, "test done", std::time::Duration::from_secs(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decision_request_holds_until_answered() {
    let env = setup().await;
    fake_agent(&env.app, "f-decide", "decide", "fake").await;
    let i = new_issue(&env, "Design question", IssueState::Ready).await;
    let r = run(&env, i.number, Role::Fix, "f-decide").await;
    assert_eq!(r.status, "succeeded");
    let cur = issue(&env, i.number).await;
    assert_eq!(cur.hold, Some(Hold::NeedsDecision));
    // Held issues are not dispatched.
    scheduler::tick(&env.app).await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE issue_id = ?").bind(cur.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(n, 1);
    services::issues::answer_decision(
        &env.app,
        &env.project,
        i.number,
        &Actor::human("dylan"),
        services::issues::DecisionAnswer { answer: "Use B.".into(), resume_to: Some(IssueState::Ready) },
    )
    .await
    .unwrap();
    let cur = issue(&env, i.number).await;
    assert!(cur.hold.is_none());
    assert_eq!(cur.state, IssueState::Ready);
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM comments WHERE issue_id = ? ORDER BY id").bind(cur.id).fetch_all(&env.app.db).await.unwrap();
    assert!(kinds.contains(&"decision_request".to_string()) && kinds.contains(&"decision".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_strategies_and_dirty_base() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    let human = Actor::human("dylan");
    let force = |strategy: &str| services::merge::MergeRequest { strategy: Some(strategy.into()), message: None, force: true };

    // Dirty base checkout blocks the merge.
    let a = new_issue(&env, "A", IssueState::Ready).await;
    run(&env, a.number, Role::Fix, "f-fix").await;
    let pr_a = services::pulls::open_pr_for_issue(&env.app.db, a.id).await.unwrap().unwrap();
    std::fs::write(env.repo.join("main.txt"), "dirty\n").unwrap();
    let err = services::merge::merge(&env.app, &env.project, pr_a.number, &human, force("merge")).await.unwrap_err();
    assert!(err.to_string().contains("local changes"), "{err}");
    git(&env.repo, &["checkout", "--", "main.txt"]);

    // Merge commit: two parents.
    services::merge::merge(&env.app, &env.project, pr_a.number, &human, force("merge")).await.unwrap();
    assert_eq!(git(&env.repo, &["rev-list", "--parents", "-n1", "master"]).split(' ').count(), 3);

    // Rebase: linear history, original commit subject kept. Base isn't checked out this time.
    git(&env.repo, &["checkout", "-q", "--detach"]);
    let b = new_issue(&env, "B", IssueState::Ready).await;
    run(&env, b.number, Role::Fix, "f-fix").await;
    let pr_b = services::pulls::open_pr_for_issue(&env.app.db, b.id).await.unwrap().unwrap();
    let before = git(&env.repo, &["rev-parse", "master"]);
    services::merge::merge(&env.app, &env.project, pr_b.number, &human, force("rebase")).await.unwrap();
    assert_eq!(git(&env.repo, &["rev-parse", "master~1"]), before);
    assert_eq!(git(&env.repo, &["log", "-1", "--format=%s", "master"]), "🐛 Fix the thing");
    assert_eq!(issue(&env, b.number).await.state, IssueState::Done);

    // Without force, an unapproved PR can't be merged.
    let c = new_issue(&env, "C", IssueState::Ready).await;
    run(&env, c.number, Role::Fix, "f-fix").await;
    let pr_c = services::pulls::open_pr_for_issue(&env.app.db, c.id).await.unwrap().unwrap();
    assert!(services::merge::merge(&env.app, &env.project, pr_c.number, &human, Default::default()).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allowlist_answers_permission_prompts() {
    let env = setup().await;
    fake_agent(&env.app, "f-perm", "perm", "fake").await;
    sqlx::query("UPDATE agent_definitions SET permission_policy = 'allowlist', permission_rules = ? WHERE slug = 'f-perm'")
        .bind(serde_json::to_string(&agent_kanban::orchestrator::permissions::default_rules()).unwrap())
        .execute(&env.app.db)
        .await
        .unwrap();
    let i = new_issue(&env, "Perms", IssueState::Triage).await;
    let r = run(&env, i.number, Role::Triage, "f-perm").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    let body: String = sqlx::query_scalar("SELECT body FROM comments WHERE issue_id = ? AND author_kind = 'agent'")
        .bind(i.id)
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ninja -C build tests"], "allow");
    assert_eq!(v["git push origin HEAD"], "reject");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_settings_apply_defaults_and_role_overrides() {
    let env = setup().await;
    fake_agent(&env.app, "f-triage", "triage", "fake").await;
    // Agent default: big model. Project override for triage: high effort, and a bogus value that's skipped.
    sqlx::query("UPDATE agent_definitions SET session_config = ? WHERE slug = 'f-triage'")
        .bind(serde_json::json!({"model": "big", "effort": "low"}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO role_session_config(project_id, role, agent_definition_id, config) SELECT ?, 'triage', id, ? FROM agent_definitions WHERE slug = 'f-triage'")
        .bind(env.project.id)
        .bind(serde_json::json!({"effort": "high", "speed": "warp"}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let i = new_issue(&env, "Settings", IssueState::Triage).await;
    let r = run(&env, i.number, Role::Triage, "f-triage").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    let used = r.session_config.unwrap().0;
    assert_eq!(used["model"], "big");
    assert_eq!(used["effort"], "high", "role override beats agent default");
    let t = transcript(&env, r.id).await;
    assert!(t.contains("model: Big · effort: High"), "{t}");
    assert!(t.contains("couldn't set `speed`"), "unknown settings are reported, not fatal");
    // The agent's options were cached for the UI.
    let a = agent(&env, "f-triage").await;
    assert_eq!(a.config_options.unwrap().0.as_array().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_agents_need_the_flag_without_a_container() {
    let env = setup().await;
    fake_agent(&env.app, "f-noop", "noop", "fake").await;
    let i = new_issue(&env, "Anything", IssueState::Ready).await;
    let a = agent(&env, "f-noop").await;
    // The server was started without --dangerously-allow-host-agents and the project has no container.
    let mut config = (*env.app.config).clone();
    config.allow_host_agents = false;
    let app = agent_kanban::AppState { config: std::sync::Arc::new(config), ..env.app.clone() };

    let err = scheduler::start_run(&app, &env.project, &i, Role::Fix, &a).await.unwrap_err();
    assert!(err.to_string().contains("--dangerously-allow-host-agents"), "{err}");
    let avail = scheduler::role_availability(&app, &env.project).await.unwrap();
    let step = scheduler::next_step(&i, None, false, &avail, &agent_kanban::db::now());
    assert_eq!((step.kind.as_str(), step.label.as_str()), ("blocked", "Needs a container"));
    // Nothing is dispatched automatically either.
    scheduler::tick(&app).await.unwrap();
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs").fetch_one(&app.db).await.unwrap();
    assert_eq!(runs, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fix_run_resumes_the_previous_fix_run_session_after_review_changes() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;

    // First fix run on the issue: nothing to resume, starts a fresh session.
    let r1 = run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);
    let t1 = transcript(&env, r1.id).await;
    assert!(t1.contains("session started"), "{t1}");
    assert!(!t1.contains("resumed"), "{t1}");

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    // Second fix run reloads the first run's session (same agent, same issue) instead of
    // reconstructing everything from the prompt alone.
    let r2 = run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    let t2 = transcript(&env, r2.id).await;
    assert!(t2.contains("session resumed from a previous run"), "{t2}");
    assert_eq!(r2.acp_session_id.as_deref(), Some("s1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fix_run_falls_back_to_a_new_session_when_resume_is_unsupported_or_fails() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;

    // The adapter doesn't advertise session/load support at all: never attempted, no fallback
    // noise, and the run still succeeds cold.
    fake_agent_env(&env.app, "f-fix-unsupported", "fix", "fake", json!({"FAKE_LOAD_SESSION": "unsupported"})).await;
    let a = new_issue(&env, "A", IssueState::Ready).await;
    run(&env, a.number, Role::Fix, "f-fix-unsupported").await;
    run(&env, a.number, Role::Review, "f-changes").await;
    let r2 = run(&env, a.number, Role::Fix, "f-fix-unsupported").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    assert!(!transcript(&env, r2.id).await.contains("resumed"), "{}", transcript(&env, r2.id).await);

    // The adapter advertises support but rejects the reload (its own session state didn't
    // survive, e.g. a recycled container): the client falls back to a fresh session.
    fake_agent_env(&env.app, "f-fix-fail", "fix", "fake", json!({"FAKE_LOAD_SESSION": "fail"})).await;
    let b = new_issue(&env, "B", IssueState::Ready).await;
    run(&env, b.number, Role::Fix, "f-fix-fail").await;
    run(&env, b.number, Role::Review, "f-changes").await;
    let r2 = run(&env, b.number, Role::Fix, "f-fix-fail").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    let t2 = transcript(&env, r2.id).await;
    assert!(t2.contains("couldn't resume session"), "{t2}");
    assert!(!t2.contains("resumed from a previous run"), "{t2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_session_usage_is_not_double_counted() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;
    // Outside the worktree: a real adapter's cumulative usage counter survives across the
    // separate processes each run spawns for the same resumed session, unlike worktree files.
    let usage_state = env.repo.parent().unwrap().join("usage-state.json").to_string_lossy().into_owned();

    fake_agent_env(&env.app, "f-fix-usage", "fix", "fake", json!({"FAKE_USAGE_STEP": "100", "FAKE_USAGE_STATE": usage_state})).await;
    let r1 = run(&env, i.number, Role::Fix, "f-fix-usage").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    // The adapter's cumulative session usage is now 100 (run 1) + 50 (this run) = 150; only 50
    // of that belongs to run 2. Update in place (not another `fake_agent_env` call, which would
    // replace the agent_definitions row and break the FK from run 1's already-recorded run).
    sqlx::query("UPDATE agent_definitions SET env = ? WHERE slug = 'f-fix-usage'")
        .bind(json!({"FAKE_MODE": "fix", "FAKE_USAGE_STEP": "50", "FAKE_USAGE_STATE": usage_state}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-usage").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    assert!(transcript(&env, r2.id).await.contains("session resumed from a previous run"));
    assert_eq!(r2.acp_session_id, r1.acp_session_id);

    let total1: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(total_tokens),0) FROM run_usage WHERE run_id = ?")
        .bind(r1.id)
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    let total2: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(total_tokens),0) FROM run_usage WHERE run_id = ?")
        .bind(r2.id)
        .fetch_one(&env.app.db)
        .await
        .unwrap();
    assert_eq!(total1, 100, "run 1's own usage");
    assert_eq!(total2, 50, "run 2's own usage, not the session's 150-token cumulative total");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_session_cost_is_not_double_counted() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;
    // Same idea as the cumulative token counter: the adapter's cumulative session cost survives
    // across the separate processes each run spawns for the same resumed session.
    let cost_state = env.repo.parent().unwrap().join("cost-state.json").to_string_lossy().into_owned();

    // FAKE_USAGE_STEP just needs to be present so the run reports a `usage` payload at all (cost
    // is only stored alongside it); its own accounting is covered by the token-doubling test.
    fake_agent_env(&env.app, "f-fix-cost", "fix", "fake", json!({"FAKE_USAGE_STEP": "1", "FAKE_COST_STEP": "1.00", "FAKE_COST_STATE": cost_state})).await;
    let r1 = run(&env, i.number, Role::Fix, "f-fix-cost").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    // The adapter's cumulative session cost is now $1.00 (run 1) + $0.50 (this run) = $1.50; only
    // $0.50 of that belongs to run 2.
    sqlx::query("UPDATE agent_definitions SET env = ? WHERE slug = 'f-fix-cost'")
        .bind(json!({"FAKE_MODE": "fix", "FAKE_USAGE_STEP": "1", "FAKE_COST_STEP": "0.50", "FAKE_COST_STATE": cost_state}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-cost").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    assert!(transcript(&env, r2.id).await.contains("session resumed from a previous run"));

    let cost1: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd),0.0) FROM run_usage WHERE run_id = ?").bind(r1.id).fetch_one(&env.app.db).await.unwrap();
    let cost2: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd),0.0) FROM run_usage WHERE run_id = ?").bind(r2.id).fetch_one(&env.app.db).await.unwrap();
    assert!((cost1 - 1.00).abs() < 1e-9, "run 1's own cost: {cost1}");
    assert!((cost2 - 0.50).abs() < 1e-9, "run 2's own cost, not the session's $1.50 cumulative total: {cost2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backfill_never_rewrites_an_earlier_runs_settled_usage() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;
    let usage_state = env.repo.parent().unwrap().join("usage-state.json").to_string_lossy().into_owned();

    // Both runs finish with only an approximate ACP-reported total (the precise session log
    // isn't available yet, e.g. it hasn't been synced from a container).
    fake_agent_env(&env.app, "f-fix-log", "fix", "fake", json!({"FAKE_USAGE_STEP": "100", "FAKE_USAGE_STATE": usage_state})).await;
    let r1 = run(&env, i.number, Role::Fix, "f-fix-log").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    sqlx::query("UPDATE agent_definitions SET env = ? WHERE slug = 'f-fix-log'")
        .bind(json!({"FAKE_MODE": "fix", "FAKE_USAGE_STEP": "50", "FAKE_USAGE_STATE": usage_state}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-log").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    assert_eq!(r2.acp_session_id, r1.acp_session_id);
    let session = r1.acp_session_id.clone().unwrap();

    // The precise session log now shows up (e.g. synced from the container after the fact), with
    // a cumulative total for the *whole* session that differs from the sum of the two runs'
    // approximate ACP reports (190, not 150) — as a real adapter's own count would, since the
    // ACP `usage` field and the session log are independent, imprecise-vs-precise sources.
    sqlx::query("UPDATE agent_definitions SET harness = 'codex' WHERE slug = 'f-fix-log'").execute(&env.app.db).await.unwrap();
    let codex_dir = env.app.config.data_dir.join("codex-sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join(format!("rollout-{session}.jsonl")),
        [
            json!({"type": "turn_context", "payload": {"model": "f-fix-log"}}).to_string(),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {
                "input_tokens": 190, "cached_input_tokens": 0, "cache_write_input_tokens": 0,
                "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 190
            }}}}).to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    agent_kanban::usage::backfill(&env.app).await;

    let row1: (i64, String) = sqlx::query_as("SELECT total_tokens, source FROM run_usage WHERE run_id = ?").bind(r1.id).fetch_one(&env.app.db).await.unwrap();
    let row2: (i64, String) = sqlx::query_as("SELECT total_tokens, source FROM run_usage WHERE run_id = ?").bind(r2.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(row1, (100, "acp".to_string()), "run 1 is not the session's latest run, so its settled total must never be rewritten from a log that now also includes run 2's activity");
    assert_eq!(row2, (90, "codex_log".to_string()), "run 2 (the latest run) gets the precise log total minus run 1's frozen 100, not the full 190");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backfill_reconciles_baseline_across_a_model_label_change() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;
    let usage_state = env.repo.parent().unwrap().join("usage-state.json").to_string_lossy().into_owned();

    // Both runs finish with only an approximate ACP-reported total, with no model metadata (the
    // fake agent doesn't report one), so it's stored under the agent slug.
    fake_agent_env(&env.app, "f-fix-model", "fix", "fake", json!({"FAKE_USAGE_STEP": "100", "FAKE_USAGE_STATE": usage_state})).await;
    let r1 = run(&env, i.number, Role::Fix, "f-fix-model").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    sqlx::query("UPDATE agent_definitions SET env = ? WHERE slug = 'f-fix-model'")
        .bind(json!({"FAKE_MODE": "fix", "FAKE_USAGE_STEP": "50", "FAKE_USAGE_STATE": usage_state}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-model").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    assert_eq!(r2.acp_session_id, r1.acp_session_id);
    let session = r1.acp_session_id.clone().unwrap();

    // The precise session log now shows up naming the *real* model, which differs from the agent
    // slug ("f-fix-model") the earlier ACP-fallback rows were stored under. The baseline lookup
    // must still find run 1's 100 tokens rather than matching on model and missing it.
    sqlx::query("UPDATE agent_definitions SET harness = 'codex' WHERE slug = 'f-fix-model'").execute(&env.app.db).await.unwrap();
    let codex_dir = env.app.config.data_dir.join("codex-sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join(format!("rollout-{session}.jsonl")),
        [
            json!({"type": "turn_context", "payload": {"model": "gpt-6-codex"}}).to_string(),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {
                "input_tokens": 190, "cached_input_tokens": 0, "cache_write_input_tokens": 0,
                "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 190
            }}}}).to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    agent_kanban::usage::backfill(&env.app).await;

    let row1: (i64, String) = sqlx::query_as("SELECT total_tokens, source FROM run_usage WHERE run_id = ?").bind(r1.id).fetch_one(&env.app.db).await.unwrap();
    let row2: (i64, String, String) =
        sqlx::query_as("SELECT total_tokens, model, source FROM run_usage WHERE run_id = ?").bind(r2.id).fetch_one(&env.app.db).await.unwrap();
    assert_eq!(row1, (100, "acp".to_string()), "run 1's settled total is untouched");
    assert_eq!(row2, (90, "gpt-6-codex".to_string(), "codex_log".to_string()), "run 2 gets 190 minus run 1's 100, found despite the model label change");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_is_skipped_when_the_requested_settings_changed() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;

    // First fix run pins an explicit model.
    fake_agent(&env.app, "f-fix-cfg", "fix", "fake").await;
    sqlx::query("UPDATE agent_definitions SET session_config = ? WHERE slug = 'f-fix-cfg'")
        .bind(json!({"model": "big"}).to_string())
        .execute(&env.app.db)
        .await
        .unwrap();
    let r1 = run(&env, i.number, Role::Fix, "f-fix-cfg").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);
    assert_eq!(r1.session_config.unwrap().0["model"], "big");

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    // The override is cleared, returning to the adapter's default model. Resuming the old
    // session would silently keep "big" active instead, so the run must start fresh.
    sqlx::query("UPDATE agent_definitions SET session_config = '{}' WHERE slug = 'f-fix-cfg'").execute(&env.app.db).await.unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-cfg").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    let t2 = transcript(&env, r2.id).await;
    assert!(!t2.contains("resumed from a previous run"), "{t2}");
    assert_eq!(r2.session_config.unwrap().0["model"], "small", "back to the adapter default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_is_skipped_when_the_effective_mode_changes() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;

    // First fix run pins an explicit session mode (the config map itself never changes).
    fake_agent(&env.app, "f-fix-mode", "fix", "fake").await;
    sqlx::query("UPDATE agent_definitions SET session_mode_id = 'default' WHERE slug = 'f-fix-mode'").execute(&env.app.db).await.unwrap();
    let r1 = run(&env, i.number, Role::Fix, "f-fix-mode").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    // The mode pin is cleared. The desired config map is unchanged (still empty), but the
    // effective launch mode is not, so resuming the retained session must still be skipped.
    sqlx::query("UPDATE agent_definitions SET session_mode_id = NULL WHERE slug = 'f-fix-mode'").execute(&env.app.db).await.unwrap();
    let r2 = run(&env, i.number, Role::Fix, "f-fix-mode").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    let t2 = transcript(&env, r2.id).await;
    assert!(!t2.contains("resumed from a previous run"), "{t2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_is_skipped_for_a_pre_migration_run_with_no_snapshot() {
    let env = setup().await;
    fake_agent(&env.app, "f-changes", "review_changes", "fake").await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    let i = new_issue(&env, "Needs changes", IssueState::Ready).await;

    let r1 = run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(r1.status, "succeeded", "{}", transcript(&env, r1.id).await);
    // Simulate a run recorded before the requested_session_config column existed (or a
    // corrupt/unreadable snapshot): its compatibility with any later run's settings is unknown,
    // not the same as "wants nothing", so it must never look compatible even when the later
    // run's own desired settings are also empty.
    sqlx::query("UPDATE agent_runs SET requested_session_config = NULL WHERE id = ?").bind(r1.id).execute(&env.app.db).await.unwrap();

    run(&env, i.number, Role::Review, "f-changes").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ChangesRequested);

    let r2 = run(&env, i.number, Role::Fix, "f-fix").await;
    assert_eq!(r2.status, "succeeded", "{}", transcript(&env, r2.id).await);
    let t2 = transcript(&env, r2.id).await;
    assert!(!t2.contains("resumed from a previous run"), "{t2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_crash_before_its_turn_fails_the_run_cleanly() {
    let env = setup().await;
    fake_agent(&env.app, "f-crash", "crash", "fake").await;
    let i = new_issue(&env, "Crashes", IssueState::Ready).await;
    // The run task used to panic here (its JoinHandle was awaited twice), leaving the run active.
    let r = run(&env, i.number, Role::Fix, "f-crash").await;
    assert_eq!(r.status, "failed");
    assert!(r.error.as_deref().unwrap_or("").contains("session ended"), "{:?}", r.error);
    assert_eq!(issue(&env, i.number).await.failure_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_review_and_merge_leave_parked_members_alone() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    fake_agent(&env.app, "f-approve", "review_approve", "fake").await;
    let batch = new_issue(&env, "Batch", IssueState::Ready).await;
    run(&env, batch.number, Role::Fix, "f-fix").await;
    assert_eq!(issue(&env, batch.number).await.state, IssueState::InReview);
    let pr = services::pulls::open_pr_for_issue(&env.app.db, batch.id).await.unwrap().unwrap();
    // A member a human deliberately parked in backlog is linked to the same PR.
    let parked = new_issue(&env, "Parked member", IssueState::Backlog).await;
    sqlx::query("INSERT INTO pull_request_issues(pr_id, issue_id) VALUES (?, ?)")
        .bind(pr.id)
        .bind(parked.id)
        .execute(&env.app.db)
        .await
        .unwrap();

    let r = run(&env, batch.number, Role::Review, "f-approve").await;
    assert_eq!(r.status, "succeeded", "{}", transcript(&env, r.id).await);
    assert_eq!(issue(&env, batch.number).await.state, IssueState::ReadyToMerge);
    assert_eq!(issue(&env, parked.number).await.state, IssueState::Backlog);
    let pr = services::pull_by_id(&env.app.db, pr.id).await.unwrap();
    assert_eq!(pr.approved_sha, pr.head_sha);
    let msg = services::merge::default_message(&env.app, pr.id, &pr.title).await;
    assert!(msg.contains(&format!("Closes #{}", batch.number)) && !msg.contains(&format!("Closes #{}", parked.number)), "{msg}");

    services::merge::merge(&env.app, &env.project, pr.number, &Actor::human("dylan"), Default::default()).await.unwrap();
    assert_eq!(issue(&env, batch.number).await.state, IssueState::Done);
    assert_eq!(issue(&env, parked.number).await.state, IssueState::Backlog);
    // The GitHub mirror's `merged` job (queued after the local transitions) closes exactly these.
    let closing: Vec<i64> = services::pr_closing_issues(&env.app.db, pr.id).await.unwrap().iter().map(|i| i.number).collect();
    assert_eq!(closing, vec![batch.number]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_without_force_rejects_a_pr_whose_issues_are_all_parked() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    fake_agent(&env.app, "f-approve", "review_approve", "fake").await;
    let i = new_issue(&env, "Deprioritised", IssueState::Ready).await;
    run(&env, i.number, Role::Fix, "f-fix").await;
    run(&env, i.number, Role::Review, "f-approve").await;
    assert_eq!(issue(&env, i.number).await.state, IssueState::ReadyToMerge);
    let pr = services::pulls::open_pr_for_issue(&env.app.db, i.id).await.unwrap().unwrap();
    // A human moves the only linked issue back to backlog after approval.
    sqlx::query("UPDATE issues SET state = 'backlog' WHERE id = ?").bind(i.id).execute(&env.app.db).await.unwrap();

    let err = services::merge::merge(&env.app, &env.project, pr.number, &Actor::human("dylan"), Default::default()).await.unwrap_err();
    assert!(format!("{err:?}").contains("parked"), "{err:?}");
    let req = services::merge::MergeRequest { force: true, ..Default::default() };
    services::merge::merge(&env.app, &env.project, pr.number, &Actor::human("dylan"), req).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_verdict_still_requires_its_own_issue_in_review() {
    let env = setup().await;
    fake_agent(&env.app, "f-fix", "fix", "fake").await;
    let i = new_issue(&env, "Batch", IssueState::Ready).await;
    run(&env, i.number, Role::Fix, "f-fix").await;
    let pr = services::pulls::open_pr_for_issue(&env.app.db, i.id).await.unwrap().unwrap();
    sqlx::query("UPDATE issues SET state = 'backlog' WHERE id = ?").bind(i.id).execute(&env.app.db).await.unwrap();
    let a = agent(&env, "f-fix").await;
    let r = scheduler::start_run(&env.app, &env.project, &issue(&env, i.number).await, Role::Review, &a).await.unwrap();
    wait_run(&env, r.id).await;
    let actor = Actor::Agent { run_id: r.id, role: Role::Review, issue_id: Some(i.id), project_id: env.project.id, agent_name: "f".into() };
    let req = services::reviews::NewReview { verdict: "approve".into(), body: String::new(), commit_sha: pr.head_sha.clone().unwrap() };
    let err = services::reviews::submit(&env.app, &env.project, pr.number, &actor, req).await.unwrap_err();
    assert!(format!("{err:?}").contains("not `in_review`"), "{err:?}");
    assert_eq!(issue(&env, i.number).await.state, IssueState::Backlog);
}
