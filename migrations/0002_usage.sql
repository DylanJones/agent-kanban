-- Token usage per run and model. `input_tokens` excludes cache reads/writes, which are counted
-- separately; `total_tokens` = input + cached_input + cache_write + output.
CREATE TABLE run_usage (
  run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  project_id INTEGER NOT NULL,
  issue_id INTEGER,
  role TEXT NOT NULL,
  agent TEXT NOT NULL,
  harness TEXT NOT NULL,
  limit_group TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  -- API-equivalent cost when the agent reports one (Claude does); subscriptions aren't billed per token.
  cost_usd REAL,
  -- Where the numbers came from: `codex_log`, `claude_log`, or `acp`.
  source TEXT NOT NULL,
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, model)
);
CREATE INDEX run_usage_started ON run_usage(started_at);
