-- The session config overrides (option id -> value) requested when a fix/merge_prep run
-- started, so a later run can check whether it's still safe to resume that run's ACP session
-- instead of silently keeping settings the requester no longer wants.
ALTER TABLE agent_runs ADD COLUMN requested_session_config TEXT;
