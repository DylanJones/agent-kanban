//! End-to-end workflow tests: a scripted ACP agent drives the board through the REST API.

mod common;

use agent_kanban::domain::{Actor, Hold, IssueState, Role};
use agent_kanban::orchestrator::scheduler;
use agent_kanban::services;
use common::*;

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

    // A human merges (squash): base advances, issue done, worktree and branch cleaned up.
    let res = services::merge::merge(&env.app, &env.project, pr.number, &Actor::human("dylan"), Default::default()).await.unwrap();
    assert_eq!(git(&env.repo, &["rev-parse", "master"]), res.merged_sha);
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
