//! Dispatch loop: picks eligible issues and starts agent runs within the concurrency limits.

use std::collections::HashMap;
use std::time::Duration;

use crate::AppState;
use crate::db;
use crate::domain::models::{AgentDefinition, AgentRun, Issue, Project};
use crate::domain::{IssueState, Role};
use crate::error::{ApiError, ApiResult};

pub fn spawn(app: AppState) {
    tokio::spawn(async move {
        loop {
            let enabled: bool = db::get_setting(&app.db, "scheduler_enabled").await.unwrap_or(false);
            if enabled && let Err(e) = tick(&app).await {
                tracing::warn!("scheduler tick failed: {e:#}");
            }
            tokio::select! {
                _ = app.bus.wake.notified() => {
                    // Debounce bursts of events.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

/// The agent definition that handles `role` in `project`.
pub async fn agent_for(app: &AppState, project: &Project, role: Role) -> anyhow::Result<Option<AgentDefinition>> {
    if let Some(a) = sqlx::query_as::<_, AgentDefinition>(
        "SELECT a.* FROM agent_definitions a JOIN project_role_agents r ON r.agent_definition_id = a.id WHERE r.project_id = ? AND r.role = ?",
    )
    .bind(project.id)
    .bind(role.as_str())
    .fetch_optional(&app.db)
    .await?
    {
        return Ok(Some(a));
    }
    let defaults: HashMap<String, String> = db::get_setting(&app.db, "default_role_agents").await.unwrap_or_default();
    let Some(slug) = defaults.get(role.as_str()) else { return Ok(None) };
    Ok(sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE slug = ?").bind(slug).fetch_optional(&app.db).await?)
}

/// What the scheduler will do with an issue's role right now.
#[derive(Debug, Clone)]
pub enum RoleAvailability {
    Available { slug: String, name: String },
    Disabled { slug: String },
    NeedsAuth { slug: String },
    /// Runs in a container but the credential it needs there isn't configured.
    MissingCredential { slug: String, what: String },
    /// Runs in a container but Docker isn't running.
    NoDocker { slug: String },
    /// The project has no container and the server doesn't allow agents on the host.
    HostAgentsOff { slug: String },
    Paused { slug: String, kind: String, until: Option<String> },
    Unassigned,
}

/// Resolve every role's agent and whether it can be dispatched, once per project.
pub async fn role_availability(app: &AppState, project: &Project) -> anyhow::Result<HashMap<Role, RoleAvailability>> {
    let mut m = HashMap::new();
    let docker_down = project.container_enabled && !crate::container::daemon_ready().await;
    for role in crate::domain::actor::ALL_ROLES {
        let avail = match agent_for(app, project, role).await? {
            None => RoleAvailability::Unassigned,
            Some(a) if !a.enabled => RoleAvailability::Disabled { slug: a.slug },
            Some(a) if !project.container_enabled && !app.config.allow_host_agents => RoleAvailability::HostAgentsOff { slug: a.slug },
            Some(a) if a.needs_auth => RoleAvailability::NeedsAuth { slug: a.slug },
            Some(a) if docker_down => RoleAvailability::NoDocker { slug: a.slug },
            Some(a)
                if project.container_enabled
                    && a.harness == "claude"
                    && app.config.current_secrets().claude_code_oauth_token.as_deref().is_none_or(str::is_empty) =>
            {
                RoleAvailability::MissingCredential { slug: a.slug, what: "claude_code_oauth_token".into() }
            }
            Some(a) => {
                let row: Option<(bool, Option<String>, Option<String>)> =
                    sqlx::query_as("SELECT paused, paused_until, pause_kind FROM limit_groups WHERE name = ?")
                        .bind(&a.limit_group)
                        .fetch_optional(&app.db)
                        .await?;
                match row {
                    Some((true, until, kind)) => RoleAvailability::Paused { slug: a.slug, kind: kind.unwrap_or_else(|| "manual".into()), until },
                    _ => RoleAvailability::Available { slug: a.slug, name: a.name },
                }
            }
        };
        m.insert(role, avail);
    }
    Ok(m)
}

/// The next thing that will happen to an issue, as the scheduler sees it.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct NextStep {
    /// `agent`: the scheduler will start an agent on it · `running`: an agent is working on it ·
    /// `waiting`: an agent will pick it up later (usage limit or retry backoff) · `human`: waiting for you ·
    /// `blocked`: on hold or misconfigured · `parked`: no automation (backlog) · `done`.
    pub kind: String,
    /// Short label for the card, e.g. "Next: Fix · claude".
    pub label: String,
    /// Longer explanation (tooltip).
    pub detail: Option<String>,
    pub role: Option<Role>,
    pub agent: Option<String>,
    /// For `waiting`: when it becomes eligible again.
    pub until: Option<String>,
    /// A fix the UI can offer, e.g. `connect-claude` (set up Claude's container token).
    pub action: Option<String>,
}

fn step(kind: &str, label: impl Into<String>, detail: Option<String>) -> NextStep {
    NextStep { kind: kind.into(), label: label.into(), detail, role: None, agent: None, until: None, action: None }
}

/// Decide what happens next to `issue`. The scheduler dispatches exactly the issues whose step is `agent`.
pub fn next_step(
    issue: &Issue,
    active_run: Option<(Role, &str)>,
    has_open_pr: bool,
    avail: &HashMap<Role, RoleAvailability>,
    now: &str,
) -> NextStep {
    use crate::domain::Hold;
    if let Some((role, agent)) = active_run {
        return NextStep { role: Some(role), agent: Some(agent.into()), ..step("running", format!("{} · {agent} working", role_title(role)), None) };
    }
    if issue.state.is_terminal() {
        return step("done", "", None);
    }
    match issue.hold {
        Some(Hold::NeedsDecision) => return step("human", "Needs your decision", issue.hold_reason.clone()),
        Some(Hold::Stalled) => return step("blocked", "Stalled", issue.hold_reason.clone()),
        Some(Hold::Paused) => return step("blocked", "Paused", issue.hold_reason.clone()),
        None => {}
    }
    match issue.state {
        IssueState::ReadyToMerge => return step("human", "Your turn: merge", Some("Approved at the current head; merge it on the PR page.".into())),
        IssueState::Backlog => {
            return step("parked", "Parked", Some("Agents never pick up Backlog. Move it to Triage or Ready to start.".into()));
        }
        _ => {}
    }
    let Some(role) = issue.state.dispatch_role() else { return step("parked", "", None) };
    if matches!(role, Role::Review | Role::MergePrep) && !has_open_pr {
        return step("human", "No open PR", Some(format!("{} needs an open pull request; open one or move the issue.", role_title(role))));
    }
    let base = |kind: &str, label: String, detail: Option<String>, slug: &str| NextStep {
        role: Some(role),
        agent: Some(slug.to_string()),
        ..step(kind, label, detail)
    };
    match avail.get(&role).unwrap_or(&RoleAvailability::Unassigned) {
        RoleAvailability::Unassigned => step("blocked", format!("No {} agent", role_title(role)), Some("Assign an agent to this role in Project settings.".into())),
        RoleAvailability::Disabled { slug } => base("blocked", format!("{slug} disabled"), Some(format!("Enable {slug} on the Agents page.")), slug),
        RoleAvailability::NeedsAuth { slug } => {
            base("blocked", format!("{slug} needs login"), Some(format!("Log {slug} in, then resume its group on the Agents page.")), slug)
        }
        RoleAvailability::MissingCredential { slug, .. } => NextStep {
            action: Some("connect-claude".into()),
            ..base(
                "blocked",
                format!("{slug}: container token missing"),
                Some(format!("Containers are on, and {slug} can't use your Keychain login inside them. Connect it once with a long-lived token.")),
                slug,
            )
        },
        RoleAvailability::HostAgentsOff { slug } => base(
            "blocked",
            "Needs a container".into(),
            Some("Agents only run in Docker. Turn on \"Run agents in containers\" in Project settings (or start the server with --dangerously-allow-host-agents).".into()),
            slug,
        ),
        RoleAvailability::NoDocker { slug } => base(
            "waiting",
            format!("{} · waiting for Docker", role_title(role)),
            Some("This project runs agents in containers and Docker isn't running. Runs start once it's up.".into()),
            slug,
        ),
        RoleAvailability::Paused { slug, kind, until } => NextStep {
            until: until.clone(),
            ..base("waiting", format!("{} · {slug} paused", role_title(role)), Some(format!("{slug} is paused ({kind}); it resumes automatically.")), slug)
        },
        RoleAvailability::Available { slug, .. } => match &issue.next_attempt_at {
            Some(t) if t.as_str() > now => NextStep {
                until: Some(t.clone()),
                ..base(
                    "waiting",
                    format!("{} · retry", role_title(role)),
                    Some(format!("{} failed run(s); retrying after a backoff.", issue.failure_count)),
                    slug,
                )
            },
            _ => base("agent", format!("Next: {} · {slug}", role_title(role)), Some(format!("The scheduler will start a {} run with {slug}.", role_title(role))), slug),
        },
    }
}

pub fn role_title(role: Role) -> &'static str {
    match role {
        Role::Triage => "Triage",
        Role::Fix => "Fix",
        Role::Review => "Review",
        Role::MergePrep => "Merge prep",
    }
}

fn priority_rank(p: &Option<String>) -> i32 {
    match p.as_deref() {
        Some("P0") => 0,
        Some("P1") => 1,
        Some("P2") => 2,
        _ => 3,
    }
}

pub async fn tick(app: &AppState) -> anyhow::Result<()> {
    let max: i64 = db::get_setting(&app.db, "max_concurrent_runs").await.unwrap_or(3);
    let mut active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')").fetch_one(&app.db).await?;
    if active >= max {
        return Ok(());
    }
    let now = db::now();
    let mut candidates = sqlx::query_as::<_, Issue>(
        "SELECT i.* FROM issues i
          WHERE i.hold IS NULL
            AND i.state IN ('triage','ready','in_progress','changes_requested','merge_conflict','in_review')
            AND (i.next_attempt_at IS NULL OR i.next_attempt_at <= ?)
            AND NOT EXISTS (SELECT 1 FROM agent_runs r WHERE r.issue_id = i.id AND r.status IN ('queued','preparing','running'))",
    )
    .bind(&now)
    .fetch_all(&app.db)
    .await?;
    candidates.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then(a.state.precedence().cmp(&b.state.precedence()))
            .then(a.rank.partial_cmp(&b.rank).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.updated_at.cmp(&b.updated_at))
    });

    let mut projects: HashMap<i64, (Project, HashMap<Role, RoleAvailability>)> = HashMap::new();
    for issue in candidates {
        if active >= max {
            break;
        }
        let Some(role) = issue.state.dispatch_role() else { continue };
        if let std::collections::hash_map::Entry::Vacant(e) = projects.entry(issue.project_id) {
            let p = crate::services::project_by_id(&app.db, issue.project_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            let avail = role_availability(app, &p).await?;
            e.insert((p, avail));
        }
        let (project, avail) = &projects[&issue.project_id];
        if !std::path::Path::new(&project.repo_path).exists() {
            continue;
        }
        let has_open_pr = crate::services::pulls::open_pr_for_issue(&app.db, issue.id).await?.is_some();
        if next_step(&issue, None, has_open_pr, avail, &now).kind != "agent" {
            continue;
        }
        let Some(agent) = agent_for(app, project, role).await? else { continue };
        if let Some(cap) = project.max_concurrent_runs {
            let n: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE project_id = ? AND status IN ('queued','preparing','running')")
                    .bind(project.id)
                    .fetch_one(&app.db)
                    .await?;
            if n >= cap {
                continue;
            }
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs WHERE agent_definition_id = ? AND status IN ('queued','preparing','running')",
        )
        .bind(agent.id)
        .fetch_one(&app.db)
        .await?;
        if n >= agent.max_concurrent {
            continue;
        }
        match start_run(app, project, &issue, role, &agent).await {
            Ok(_) => active += 1,
            Err(e) => tracing::debug!("could not start run for #{}: {e}", issue.number),
        }
    }
    Ok(())
}

/// Create a run row and spawn its task. The unique index on active runs makes this race-free.
pub async fn start_run(app: &AppState, project: &Project, issue: &Issue, role: Role, agent: &AgentDefinition) -> ApiResult<AgentRun> {
    if issue.state == IssueState::Done || issue.state == IssueState::Closed {
        return Err(ApiError::conflict("issue is closed"));
    }
    if !project.container_enabled && !app.config.allow_host_agents {
        return Err(ApiError::conflict(crate::config::HOST_AGENTS_OFF));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO agent_runs(project_id, issue_id, role, agent_definition_id, status, created_at) VALUES (?, ?, ?, ?, 'queued', ?) RETURNING id",
    )
    .bind(project.id)
    .bind(issue.id)
    .bind(role)
    .bind(agent.id)
    .bind(db::now())
    .fetch_one(&app.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::conflict(format!("issue #{} already has an active run", issue.number)),
        e => e.into(),
    })?;
    tracing::info!("run #{id}: {} #{} with {}", role.as_str(), issue.number, agent.slug);
    let app2 = app.clone();
    tokio::spawn(super::run::execute(app2, id));
    app.bus.run(Some(&project.slug), id);
    app.bus.issue(&project.slug, issue.number);
    Ok(sqlx::query_as::<_, AgentRun>("SELECT * FROM agent_runs WHERE id = ?").bind(id).fetch_one(&app.db).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Hold;

    fn issue(state: IssueState) -> Issue {
        Issue {
            id: 1,
            project_id: 1,
            number: 7,
            title: "t".into(),
            body: String::new(),
            state,
            hold: None,
            hold_reason: None,
            hold_set_at: None,
            priority: None,
            size: None,
            estimate: None,
            start_date: None,
            target_date: None,
            parent_issue_id: None,
            rank: 0.0,
            source: "human".into(),
            reported_by_run_id: None,
            author_name: None,
            close_reason: None,
            failure_count: 0,
            next_attempt_at: None,
            branch_name: None,
            github_number: None,
            github_node_id: None,
            github_project_item_id: None,
            gh_synced_at: None,
            created_at: String::new(),
            updated_at: String::new(),
            closed_at: None,
        }
    }

    fn avail() -> HashMap<Role, RoleAvailability> {
        let mut m = HashMap::new();
        for r in crate::domain::actor::ALL_ROLES {
            m.insert(r, RoleAvailability::Available { slug: "claude".into(), name: "Claude".into() });
        }
        m
    }

    const NOW: &str = "2026-09-28T00:00:00.000Z";

    #[test]
    fn kinds() {
        let a = avail();
        let k = |i: &Issue, pr: bool| next_step(i, None, pr, &a, NOW).kind;
        assert_eq!(k(&issue(IssueState::Backlog), false), "parked");
        assert_eq!(k(&issue(IssueState::Triage), false), "agent");
        assert_eq!(k(&issue(IssueState::Ready), false), "agent");
        assert_eq!(k(&issue(IssueState::ChangesRequested), true), "agent");
        assert_eq!(k(&issue(IssueState::InReview), true), "agent");
        assert_eq!(k(&issue(IssueState::InReview), false), "human", "review needs a PR");
        assert_eq!(k(&issue(IssueState::ReadyToMerge), true), "human");
        assert_eq!(k(&issue(IssueState::Done), true), "done");
        let mut held = issue(IssueState::Ready);
        held.hold = Some(Hold::NeedsDecision);
        assert_eq!(k(&held, false), "human");
        held.hold = Some(Hold::Paused);
        assert_eq!(k(&held, false), "blocked");
        let mut retry = issue(IssueState::Ready);
        retry.next_attempt_at = Some("2026-09-28T01:00:00.000Z".into());
        assert_eq!(k(&retry, false), "waiting");
        assert_eq!(next_step(&issue(IssueState::Ready), Some((Role::Fix, "claude")), false, &a, NOW).kind, "running");
    }

    #[test]
    fn availability() {
        let mut a = avail();
        a.insert(Role::Fix, RoleAvailability::Paused { slug: "claude".into(), kind: "quota".into(), until: Some("x".into()) });
        let s = next_step(&issue(IssueState::Ready), None, false, &a, NOW);
        assert_eq!((s.kind.as_str(), s.until.as_deref()), ("waiting", Some("x")));
        a.insert(Role::Fix, RoleAvailability::NeedsAuth { slug: "claude".into() });
        assert_eq!(next_step(&issue(IssueState::Ready), None, false, &a, NOW).kind, "blocked");
        let s = next_step(&issue(IssueState::Triage), None, false, &a, NOW);
        assert_eq!(s.label, "Next: Triage · claude");
    }
}
