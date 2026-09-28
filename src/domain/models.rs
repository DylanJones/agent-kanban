//! Database row types, also used as API response bodies.

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use utoipa::ToSchema;

use super::actor::Role;
use super::state::{Hold, IssueState};

pub type JsonValue = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Project {
    pub id: i64,
    /// URL-safe identifier used in API paths, e.g. `emojicode`.
    pub slug: String,
    pub name: String,
    /// Absolute path of the main git checkout.
    pub repo_path: String,
    pub base_branch: String,
    /// Prefix for branches the app creates, e.g. `agent/` → `agent/issue-12-fix-crash`.
    pub branch_prefix: String,
    /// `squash`, `merge` or `rebase`.
    pub merge_strategy: String,
    /// Regex every merge commit message must match (emojicode: must start with an emoji).
    pub commit_msg_regex: Option<String>,
    /// Shell script run once in each new branch worktree (e.g. configure the build).
    pub setup_script: Option<String>,
    /// Project-specific instructions injected into every agent prompt.
    pub agent_instructions: Option<String>,
    pub max_concurrent_runs: Option<i64>,
    #[serde(skip)]
    pub next_number: i64,
    pub container_enabled: bool,
    /// Dockerfile contents for the per-project base image.
    pub container_dockerfile: Option<String>,
    /// Build context directory (defaults to the repo).
    pub container_context: Option<String>,
    /// Built image tag.
    pub container_image: Option<String>,
    #[schema(value_type = Vec<String>)]
    pub container_extra_args: Json<Vec<String>>,
    /// `owner/name` of the GitHub mirror.
    pub github_repo: Option<String>,
    pub github_project_owner: Option<String>,
    pub github_project_number: Option<i64>,
    #[serde(skip)]
    pub github_project_id: Option<String>,
    #[serde(skip)]
    pub github_status_field_id: Option<String>,
    #[serde(skip)]
    pub github_status_options: Json<JsonValue>,
    pub mirror_push_branches: bool,
    pub mirror_create_prs: bool,
    pub mirror_sync_status: bool,
    pub mirror_create_issues: bool,
    pub mirror_post_verdicts: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Label {
    #[serde(skip)]
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    pub name: String,
    /// Hex color without `#`.
    pub color: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Issue {
    #[serde(skip)]
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub hold: Option<Hold>,
    pub hold_reason: Option<String>,
    pub hold_set_at: Option<String>,
    pub priority: Option<String>,
    pub size: Option<String>,
    pub estimate: Option<f64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    #[serde(skip)]
    pub parent_issue_id: Option<i64>,
    /// Sort order within a column (lower first).
    pub rank: f64,
    /// `human`, `agent` or `github`.
    pub source: String,
    pub reported_by_run_id: Option<i64>,
    pub author_name: Option<String>,
    pub close_reason: Option<String>,
    pub failure_count: i64,
    pub next_attempt_at: Option<String>,
    pub branch_name: Option<String>,
    pub github_number: Option<i64>,
    #[serde(skip)]
    pub github_node_id: Option<String>,
    #[serde(skip)]
    pub github_project_item_id: Option<String>,
    #[serde(skip)]
    pub gh_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct PullRequest {
    #[serde(skip)]
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub branch: String,
    pub base_branch: String,
    pub head_sha: Option<String>,
    pub merge_base_sha: Option<String>,
    pub has_conflicts: bool,
    #[schema(value_type = Vec<String>)]
    pub conflict_files: Json<Vec<String>>,
    pub conflicts_checked_at: Option<String>,
    #[serde(skip)]
    pub conflicts_base_sha: Option<String>,
    pub review_requested_sha: Option<String>,
    /// Head SHA the reviewer approved; must equal `head_sha` to merge.
    pub approved_sha: Option<String>,
    pub last_reviewed_sha: Option<String>,
    pub merged_sha: Option<String>,
    pub merged_at: Option<String>,
    pub merge_strategy: Option<String>,
    pub author_kind: String,
    pub author_name: Option<String>,
    pub created_by_run_id: Option<i64>,
    pub github_number: Option<i64>,
    pub github_url: Option<String>,
    #[serde(skip)]
    pub github_node_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Comment {
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    #[serde(skip)]
    pub issue_id: Option<i64>,
    #[serde(skip)]
    pub pr_id: Option<i64>,
    pub thread_id: Option<i64>,
    /// `comment`, `decision_request`, `decision`, `review_summary` or `system`.
    pub kind: String,
    /// Markdown.
    pub body: String,
    /// `human`, `agent`, `system` or `github`.
    pub author_kind: String,
    pub author_name: String,
    pub run_id: Option<i64>,
    #[serde(skip)]
    pub github_node_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct ReviewThread {
    pub id: i64,
    #[serde(skip)]
    pub pr_id: i64,
    /// File path relative to the repo root.
    pub path: String,
    /// Line in the file at `commit_sha` (new side for RIGHT, old side for LEFT).
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    /// `RIGHT` (new code) or `LEFT` (removed code).
    pub side: String,
    pub commit_sha: Option<String>,
    pub original_line: Option<i64>,
    pub original_commit_sha: Option<String>,
    pub diff_hunk: Option<String>,
    /// `blocking` threads must be resolved before approval; `nit` threads need not be.
    pub severity: String,
    pub resolved: bool,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<String>,
    /// The anchored code changed after the thread was written.
    pub outdated: bool,
    pub created_by_run_id: Option<i64>,
    #[serde(skip)]
    pub github_node_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Review {
    pub id: i64,
    #[serde(skip)]
    pub pr_id: i64,
    /// `approve`, `changes_requested`, `needs_decision` or `comment`.
    pub verdict: String,
    pub body: String,
    /// The PR head the review applies to.
    pub commit_sha: Option<String>,
    pub author_kind: String,
    pub author_name: String,
    pub run_id: Option<i64>,
    #[serde(skip)]
    pub github_node_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct AgentDefinition {
    pub id: i64,
    pub slug: String,
    pub name: String,
    /// `claude`, `codex`, `opencode` or `custom`.
    pub harness: String,
    /// Executable that speaks ACP on stdio.
    pub command: String,
    #[schema(value_type = Vec<String>)]
    pub args: Json<Vec<String>>,
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub env: Json<std::collections::BTreeMap<String, String>>,
    /// Command (argv) to run inside a container instead of `command`/`args`.
    #[schema(value_type = Option<Vec<String>>)]
    pub container_command: Option<Json<Vec<String>>>,
    /// Definitions sharing a subscription share a limit group; a usage limit pauses the whole group.
    pub limit_group: String,
    pub max_concurrent: i64,
    /// How to answer the agent's permission prompts when running on the host:
    /// `allowlist` (apply `permission_rules`, ask a human if nothing matches), `auto_allow`, `ask` or `deny`.
    pub permission_policy: String,
    /// Same, when running inside a container (default `auto_allow`: the container is the sandbox).
    pub container_permission_policy: String,
    /// Ordered rules for the `allowlist` policy; the first match wins.
    #[schema(value_type = Vec<crate::orchestrator::permissions::PermissionRule>)]
    pub permission_rules: Json<Vec<crate::orchestrator::permissions::PermissionRule>>,
    /// ACP session mode to select after `session/new` when running on the host.
    pub session_mode_id: Option<String>,
    /// ACP session mode to select when running inside a container.
    pub container_session_mode_id: Option<String>,
    pub enabled: bool,
    /// Set when the adapter reported an authentication error.
    pub needs_auth: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct LimitGroup {
    pub name: String,
    /// When the subscription is expected to be available again.
    pub paused_until: Option<String>,
    pub paused: bool,
    /// `quota`, `rate`, `auth` or `manual`.
    pub pause_kind: Option<String>,
    pub pause_reason: Option<String>,
    pub probe_attempts: i64,
    pub next_probe_at: Option<String>,
    /// Latest usage/rate-limit snapshot reported by the adapter.
    #[schema(value_type = Option<Object>)]
    pub last_snapshot: Option<Json<JsonValue>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct AgentRun {
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    #[serde(skip)]
    pub issue_id: Option<i64>,
    #[serde(skip)]
    pub pr_id: Option<i64>,
    pub role: Role,
    pub agent_definition_id: i64,
    /// `queued`, `preparing`, `running`, `succeeded`, `failed`, `cancelled`, `rate_limited` or `interrupted`.
    pub status: String,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub stop_reason: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_kind: Option<String>,
    pub start_head_sha: Option<String>,
    pub end_head_sha: Option<String>,
    pub container_name: Option<String>,
    pub acp_session_id: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub agent_info: Option<Json<JsonValue>>,
    #[schema(value_type = Option<Object>)]
    pub usage: Option<Json<JsonValue>>,
    #[serde(skip)]
    pub token_hash: Option<String>,
    #[serde(skip)]
    pub token_expires_at: Option<String>,
    pub nudges: i64,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

impl AgentRun {
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "preparing" | "running")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct RunEvent {
    #[serde(skip)]
    pub run_id: i64,
    pub seq: i64,
    pub ts: String,
    /// `message`, `thought`, `tool_call`, `plan`, `usage`, `stderr`, `status`, `prompt`, `permission`...
    pub kind: String,
    #[serde(skip)]
    pub key: Option<String>,
    #[schema(value_type = Object)]
    pub payload: Json<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct PermissionRequest {
    pub id: i64,
    pub run_id: i64,
    #[schema(value_type = Object)]
    pub tool_call: Json<JsonValue>,
    #[schema(value_type = Vec<Object>)]
    pub options: Json<JsonValue>,
    pub status: String,
    pub selected_option_id: Option<String>,
    pub answered_by: Option<String>,
    pub created_at: String,
    pub answered_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Event {
    pub id: i64,
    #[serde(skip)]
    pub project_id: Option<i64>,
    #[serde(skip)]
    pub issue_id: Option<i64>,
    #[serde(skip)]
    pub pr_id: Option<i64>,
    pub run_id: Option<i64>,
    pub actor_kind: String,
    pub actor_name: String,
    /// e.g. `issue.created`, `issue.transition`, `pr.merged`.
    #[serde(rename = "type")]
    pub r#type: String,
    #[schema(value_type = Object)]
    pub data: Json<JsonValue>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub project_id: Option<i64>,
    pub status: String,
    pub log: String,
    pub error: Option<String>,
    pub attempts: i64,
    #[schema(value_type = Object)]
    pub payload: Json<JsonValue>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    #[serde(skip)]
    pub token_hash: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Worktree {
    pub id: i64,
    pub project_id: i64,
    pub issue_id: Option<i64>,
    pub path: String,
    pub branch: Option<String>,
    pub kind: String,
    pub created_at: String,
    pub removed_at: Option<String>,
}
