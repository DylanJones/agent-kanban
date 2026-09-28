-- agent-kanban initial schema. Timestamps are RFC3339 UTC text.

CREATE TABLE projects (
  id INTEGER PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  repo_path TEXT NOT NULL,
  base_branch TEXT NOT NULL DEFAULT 'master',
  branch_prefix TEXT NOT NULL DEFAULT 'agent/',
  merge_strategy TEXT NOT NULL DEFAULT 'squash' CHECK (merge_strategy IN ('squash','merge','rebase')),
  commit_msg_regex TEXT,
  setup_script TEXT,
  agent_instructions TEXT,
  max_concurrent_runs INTEGER,
  next_number INTEGER NOT NULL DEFAULT 1,
  container_enabled INTEGER NOT NULL DEFAULT 0,
  container_dockerfile TEXT,
  container_context TEXT,
  container_image TEXT,
  container_extra_args TEXT NOT NULL DEFAULT '[]',
  github_repo TEXT,
  github_project_owner TEXT,
  github_project_number INTEGER,
  github_project_id TEXT,
  github_status_field_id TEXT,
  github_status_options TEXT NOT NULL DEFAULT '{}',
  mirror_push_branches INTEGER NOT NULL DEFAULT 0,
  mirror_create_prs INTEGER NOT NULL DEFAULT 0,
  mirror_sync_status INTEGER NOT NULL DEFAULT 0,
  mirror_create_issues INTEGER NOT NULL DEFAULT 0,
  mirror_post_verdicts INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE labels (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  color TEXT NOT NULL DEFAULT '888888',
  description TEXT,
  github_node_id TEXT UNIQUE,
  UNIQUE (project_id, name)
);

CREATE TABLE issues (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL,
  hold TEXT CHECK (hold IN ('needs_decision','stalled','paused')),
  hold_reason TEXT,
  hold_set_at TEXT,
  priority TEXT CHECK (priority IN ('P0','P1','P2')),
  size TEXT CHECK (size IN ('XS','S','M','L','XL')),
  estimate REAL,
  start_date TEXT,
  target_date TEXT,
  parent_issue_id INTEGER REFERENCES issues(id) ON DELETE SET NULL,
  rank REAL NOT NULL DEFAULT 0,
  source TEXT NOT NULL DEFAULT 'human' CHECK (source IN ('human','agent','github')),
  reported_by_run_id INTEGER,
  author_name TEXT,
  close_reason TEXT,
  failure_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  branch_name TEXT,
  github_node_id TEXT UNIQUE,
  github_number INTEGER,
  github_project_item_id TEXT,
  gh_synced_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  closed_at TEXT,
  UNIQUE (project_id, number)
);
CREATE INDEX issues_state ON issues(project_id, state);

CREATE TABLE issue_labels (
  issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  label_id INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
  PRIMARY KEY (issue_id, label_id)
);

CREATE TABLE pull_requests (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','merged','closed')),
  branch TEXT NOT NULL,
  base_branch TEXT NOT NULL,
  head_sha TEXT,
  merge_base_sha TEXT,
  has_conflicts INTEGER NOT NULL DEFAULT 0,
  conflict_files TEXT NOT NULL DEFAULT '[]',
  conflicts_checked_at TEXT,
  conflicts_base_sha TEXT,
  review_requested_sha TEXT,
  approved_sha TEXT,
  last_reviewed_sha TEXT,
  merged_sha TEXT,
  merged_at TEXT,
  merge_strategy TEXT,
  author_kind TEXT NOT NULL DEFAULT 'human',
  author_name TEXT,
  created_by_run_id INTEGER,
  github_node_id TEXT UNIQUE,
  github_number INTEGER,
  github_url TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (project_id, number)
);

CREATE TABLE pull_request_issues (
  pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
  issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  closes INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (pr_id, issue_id)
);

CREATE TABLE review_threads (
  id INTEGER PRIMARY KEY,
  pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  line INTEGER,
  start_line INTEGER,
  side TEXT NOT NULL DEFAULT 'RIGHT' CHECK (side IN ('LEFT','RIGHT')),
  commit_sha TEXT,
  original_line INTEGER,
  original_commit_sha TEXT,
  diff_hunk TEXT,
  severity TEXT NOT NULL DEFAULT 'blocking' CHECK (severity IN ('blocking','nit')),
  resolved INTEGER NOT NULL DEFAULT 0,
  resolved_by TEXT,
  resolved_at TEXT,
  outdated INTEGER NOT NULL DEFAULT 0,
  created_by_run_id INTEGER,
  github_node_id TEXT UNIQUE,
  created_at TEXT NOT NULL
);

CREATE TABLE comments (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  issue_id INTEGER REFERENCES issues(id) ON DELETE CASCADE,
  pr_id INTEGER REFERENCES pull_requests(id) ON DELETE CASCADE,
  thread_id INTEGER REFERENCES review_threads(id) ON DELETE CASCADE,
  kind TEXT NOT NULL DEFAULT 'comment' CHECK (kind IN ('comment','decision_request','decision','review_summary','system')),
  body TEXT NOT NULL,
  author_kind TEXT NOT NULL CHECK (author_kind IN ('human','agent','system','github')),
  author_name TEXT NOT NULL,
  run_id INTEGER,
  github_node_id TEXT UNIQUE,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK ((issue_id IS NOT NULL) + (pr_id IS NOT NULL) + (thread_id IS NOT NULL) = 1)
);
CREATE INDEX comments_issue ON comments(issue_id);
CREATE INDEX comments_pr ON comments(pr_id);
CREATE INDEX comments_thread ON comments(thread_id);

CREATE TABLE reviews (
  id INTEGER PRIMARY KEY,
  pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
  verdict TEXT NOT NULL CHECK (verdict IN ('approve','changes_requested','needs_decision','comment')),
  body TEXT NOT NULL DEFAULT '',
  commit_sha TEXT,
  author_kind TEXT NOT NULL,
  author_name TEXT NOT NULL,
  run_id INTEGER,
  github_node_id TEXT UNIQUE,
  created_at TEXT NOT NULL
);

CREATE TABLE agent_definitions (
  id INTEGER PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  harness TEXT NOT NULL CHECK (harness IN ('claude','codex','opencode','custom')),
  command TEXT NOT NULL,
  args TEXT NOT NULL DEFAULT '[]',
  env TEXT NOT NULL DEFAULT '{}',
  container_command TEXT,
  limit_group TEXT NOT NULL,
  max_concurrent INTEGER NOT NULL DEFAULT 2,
  permission_policy TEXT NOT NULL DEFAULT 'allowlist' CHECK (permission_policy IN ('auto_allow','allowlist','ask','deny')),
  container_permission_policy TEXT NOT NULL DEFAULT 'auto_allow' CHECK (container_permission_policy IN ('auto_allow','allowlist','ask','deny')),
  permission_rules TEXT NOT NULL DEFAULT '[]',
  session_mode_id TEXT,
  container_session_mode_id TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  needs_auth INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE limit_groups (
  name TEXT PRIMARY KEY,
  paused_until TEXT,
  paused INTEGER NOT NULL DEFAULT 0,
  pause_kind TEXT CHECK (pause_kind IN ('quota','rate','auth','manual')),
  pause_reason TEXT,
  probe_attempts INTEGER NOT NULL DEFAULT 0,
  next_probe_at TEXT,
  last_snapshot TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE project_role_agents (
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('triage','fix','review','merge_prep')),
  agent_definition_id INTEGER NOT NULL REFERENCES agent_definitions(id) ON DELETE CASCADE,
  PRIMARY KEY (project_id, role)
);

CREATE TABLE prompt_templates (
  id INTEGER PRIMARY KEY,
  project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  body TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (project_id, role)
);

CREATE TABLE agent_runs (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  issue_id INTEGER REFERENCES issues(id) ON DELETE CASCADE,
  pr_id INTEGER REFERENCES pull_requests(id) ON DELETE SET NULL,
  role TEXT NOT NULL,
  agent_definition_id INTEGER NOT NULL REFERENCES agent_definitions(id),
  status TEXT NOT NULL CHECK (status IN ('queued','preparing','running','succeeded','failed','cancelled','rate_limited','interrupted')),
  outcome TEXT,
  error TEXT,
  stop_reason TEXT,
  worktree_path TEXT,
  worktree_kind TEXT,
  start_head_sha TEXT,
  end_head_sha TEXT,
  container_name TEXT,
  acp_session_id TEXT,
  agent_info TEXT,
  usage TEXT,
  token_hash TEXT UNIQUE,
  token_expires_at TEXT,
  nudges INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT
);
CREATE UNIQUE INDEX one_active_run_per_issue ON agent_runs(issue_id)
  WHERE status IN ('queued','preparing','running');

CREATE TABLE run_events (
  run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  ts TEXT NOT NULL,
  kind TEXT NOT NULL,
  key TEXT,
  payload TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);
CREATE INDEX run_events_key ON run_events(run_id, key);

CREATE TABLE permission_requests (
  id INTEGER PRIMARY KEY,
  run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  tool_call TEXT NOT NULL,
  options TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','answered','expired')),
  selected_option_id TEXT,
  answered_by TEXT,
  created_at TEXT NOT NULL,
  answered_at TEXT
);

CREATE TABLE worktrees (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  issue_id INTEGER REFERENCES issues(id) ON DELETE SET NULL,
  path TEXT NOT NULL,
  branch TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('branch','detached')),
  created_at TEXT NOT NULL,
  removed_at TEXT
);
CREATE UNIQUE INDEX worktrees_live_path ON worktrees(path) WHERE removed_at IS NULL;

CREATE TABLE events (
  id INTEGER PRIMARY KEY,
  project_id INTEGER,
  issue_id INTEGER,
  pr_id INTEGER,
  run_id INTEGER,
  actor_kind TEXT NOT NULL,
  actor_name TEXT NOT NULL,
  type TEXT NOT NULL,
  data TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);
CREATE INDEX events_issue ON events(issue_id);

CREATE TABLE api_tokens (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

CREATE TABLE jobs (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  project_id INTEGER,
  status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed')),
  log TEXT NOT NULL DEFAULT '',
  error TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  payload TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  finished_at TEXT
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
