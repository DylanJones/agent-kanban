-- Model / effort / other ACP session settings.
-- Desired defaults per agent (option id → value), e.g. {"model": "opus", "effort": "high"}.
ALTER TABLE agent_definitions ADD COLUMN session_config TEXT NOT NULL DEFAULT '{}';
-- The options the agent last reported (ACP configOptions), used to offer valid choices.
ALTER TABLE agent_definitions ADD COLUMN config_options TEXT;
ALTER TABLE agent_definitions ADD COLUMN config_options_at TEXT;
-- Per project + role overrides of an agent's defaults.
CREATE TABLE role_session_config (
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  agent_definition_id INTEGER NOT NULL REFERENCES agent_definitions(id) ON DELETE CASCADE,
  config TEXT NOT NULL DEFAULT '{}',
  PRIMARY KEY (project_id, role, agent_definition_id)
);
-- What a run actually used (option id → value, after applying).
ALTER TABLE agent_runs ADD COLUMN session_config TEXT;
