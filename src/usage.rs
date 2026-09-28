//! Token usage accounting per run and model.
//!
//! Sources, most precise first:
//! * Codex writes a rollout log per session (`~/.codex/sessions/**/rollout-*-<session>.jsonl`, or
//!   the data dir's `codex-sessions/` for container runs) with running totals and the model.
//! * Claude Code writes a session log (`~/.claude/projects/*/<session>.jsonl`) with per-message
//!   usage and model, so multi-model sessions (sub-agents) are split correctly.
//! * Otherwise the ACP `session/prompt` response's cumulative `usage` (and Claude's reported cost).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::AppState;
use crate::db;

#[derive(Debug, Clone, Default, Serialize, ToSchema, PartialEq)]
pub struct Tokens {
    /// Input tokens not served from cache.
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
    pub output: i64,
    /// Reasoning/thinking tokens (a subset of output where the provider reports them).
    pub reasoning: i64,
    pub total: i64,
}

impl Tokens {
    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cached_input += o.cached_input;
        self.cache_write += o.cache_write;
        self.output += o.output;
        self.reasoning += o.reasoning;
        self.total += o.total;
    }
}

fn n(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

/// Recursively find a file under `dir` whose name contains `needle` and ends with `.jsonl`.
fn find_log(dir: &Path, needle: &str, depth: usize) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = vec![];
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            subdirs.push(p);
        } else if name.ends_with(".jsonl") && name.contains(needle) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    // Newest first (date-named dirs sort chronologically).
    subdirs.sort();
    subdirs.into_iter().rev().find_map(|d| find_log(&d, needle, depth - 1))
}

pub struct CodexLog {
    pub model: Option<String>,
    pub tokens: Tokens,
    pub rate_limits: Option<Value>,
}

pub fn codex_log(session_id: &str, dirs: &[PathBuf]) -> Option<CodexLog> {
    let path = dirs.iter().find_map(|d| find_log(d, session_id, 4))?;
    let text = std::fs::read_to_string(path).ok()?;
    let mut model = None;
    let mut total = None;
    let mut rate_limits = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("turn_context"), _) => {
                if let Some(m) = p["model"].as_str() {
                    model = Some(m.to_string());
                }
            }
            (Some("event_msg"), Some("token_count")) => {
                if let Some(t) = p.pointer("/info/total_token_usage") {
                    total = Some(t.clone());
                }
                if !p["rate_limits"].is_null() {
                    rate_limits = Some(p["rate_limits"].clone());
                }
            }
            _ => {}
        }
    }
    let t = total?;
    let cached = n(&t, "cached_input_tokens");
    let tokens = Tokens {
        input: (n(&t, "input_tokens") - cached).max(0),
        cached_input: cached,
        cache_write: n(&t, "cache_write_input_tokens"),
        output: n(&t, "output_tokens"),
        reasoning: n(&t, "reasoning_output_tokens"),
        total: n(&t, "total_tokens"),
    };
    Some(CodexLog { model, tokens, rate_limits })
}

/// Per-model totals from a Claude Code session log.
pub fn claude_log(session_id: &str) -> Option<HashMap<String, Tokens>> {
    let root = dirs::home_dir()?.join(".claude/projects");
    let path = find_log(&root, session_id, 1)?;
    let text = std::fs::read_to_string(path).ok()?;
    // Streaming writes several lines per message; the last one carries the final usage.
    let mut by_msg: HashMap<String, (String, Value)> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["type"] != "assistant" {
            continue;
        }
        let m = &v["message"];
        let (Some(id), Some(model)) = (m["id"].as_str(), m["model"].as_str()) else { continue };
        if model.starts_with('<') {
            continue; // synthetic messages
        }
        by_msg.insert(id.to_string(), (model.to_string(), m["usage"].clone()));
    }
    let mut out: HashMap<String, Tokens> = HashMap::new();
    for (model, u) in by_msg.into_values() {
        let t = Tokens {
            input: n(&u, "input_tokens"),
            cached_input: n(&u, "cache_read_input_tokens"),
            cache_write: n(&u, "cache_creation_input_tokens"),
            output: n(&u, "output_tokens"),
            reasoning: 0,
            total: n(&u, "input_tokens") + n(&u, "cache_read_input_tokens") + n(&u, "cache_creation_input_tokens") + n(&u, "output_tokens"),
        };
        out.entry(model).or_default().add(&t);
    }
    (!out.is_empty()).then_some(out)
}

/// Cumulative session usage from an ACP prompt response (`usage` object).
pub fn from_acp(u: &Value) -> Tokens {
    let input = n(u, "inputTokens");
    let cached = n(u, "cachedReadTokens");
    let write = n(u, "cachedWriteTokens");
    let output = n(u, "outputTokens");
    let total = n(u, "totalTokens");
    Tokens {
        input,
        cached_input: cached,
        cache_write: write,
        output,
        reasoning: n(u, "thoughtTokens"),
        total: if total > 0 { total } else { input + cached + write + output },
    }
}

struct RunInfo {
    project_id: i64,
    issue_id: Option<i64>,
    role: String,
    agent: String,
    harness: String,
    limit_group: String,
    session: Option<String>,
    container: bool,
    started_at: String,
}

async fn run_info(app: &AppState, run_id: i64) -> Option<RunInfo> {
    let r: (i64, Option<i64>, String, String, String, String, Option<String>, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT r.project_id, r.issue_id, r.role, a.slug, a.harness, a.limit_group, r.acp_session_id, r.container_name,
                r.started_at, r.created_at
           FROM agent_runs r JOIN agent_definitions a ON a.id = r.agent_definition_id WHERE r.id = ?",
    )
    .bind(run_id)
    .fetch_optional(&app.db)
    .await
    .ok()??;
    Some(RunInfo {
        project_id: r.0,
        issue_id: r.1,
        role: r.2,
        agent: r.3,
        harness: r.4,
        limit_group: r.5,
        session: r.6,
        container: r.7.is_some(),
        started_at: r.8.unwrap_or(r.9),
    })
}

/// Replace a run's usage rows.
async fn store(app: &AppState, run_id: i64, info: &RunInfo, rows: &[(String, Tokens, Option<f64>)], source: &str) {
    let now = db::now();
    let Ok(mut tx) = db::begin_write(&app.db).await else { return };
    let _ = sqlx::query("DELETE FROM run_usage WHERE run_id = ?").bind(run_id).execute(&mut *tx).await;
    for (model, t, cost) in rows {
        let _ = sqlx::query(
            "INSERT INTO run_usage(run_id, model, project_id, issue_id, role, agent, harness, limit_group, input_tokens,
                cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens, total_tokens, cost_usd, source, started_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(model)
        .bind(info.project_id)
        .bind(info.issue_id)
        .bind(&info.role)
        .bind(&info.agent)
        .bind(&info.harness)
        .bind(&info.limit_group)
        .bind(t.input)
        .bind(t.cached_input)
        .bind(t.cache_write)
        .bind(t.output)
        .bind(t.reasoning)
        .bind(t.total)
        .bind(cost)
        .bind(source)
        .bind(&info.started_at)
        .bind(&now)
        .execute(&mut *tx)
        .await;
    }
    let _ = tx.commit().await;
}

pub fn codex_dirs(app: &AppState) -> Vec<PathBuf> {
    let mut d = vec![app.config.data_dir.join("codex-sessions")];
    if let Some(h) = dirs::home_dir() {
        d.push(h.join(".codex/sessions"));
    }
    d
}

/// What the agent reported over ACP during the run (fallback source).
#[derive(Debug, Clone, Default)]
pub struct AcpReport {
    pub usage: Option<Value>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
}

/// Recompute a run's usage from the best available source. Returns false if nothing was found.
pub async fn refresh_run(app: &AppState, run_id: i64, acp: Option<&AcpReport>) -> bool {
    let Some(info) = run_info(app, run_id).await else { return false };
    if let Some(session) = info.session.clone() {
        if info.harness == "codex"
            && let Some(log) = codex_log(&session, &codex_dirs(app))
        {
            let model = log.model.unwrap_or_else(|| "codex".into());
            store(app, run_id, &info, &[(model, log.tokens, None)], "codex_log").await;
            if let Some(rl) = log.rate_limits {
                crate::orchestrator::limits::record_snapshot(app, &info.limit_group, &rl).await;
            }
            return true;
        }
        if info.harness == "claude"
            && !info.container
            && let Some(models) = claude_log(&session)
        {
            // Attach the reported session cost to the main (largest) model.
            let main = models.iter().max_by_key(|(_, t)| t.total).map(|(m, _)| m.clone());
            let cost = acp.and_then(|a| a.cost_usd);
            let rows: Vec<_> = models.into_iter().map(|(m, t)| {
                let c = if Some(&m) == main.as_ref() { cost } else { None };
                (m, t, c)
            }).collect();
            store(app, run_id, &info, &rows, "claude_log").await;
            return true;
        }
    }
    if let Some(a) = acp
        && let Some(u) = &a.usage
    {
        let model = a.model.clone().unwrap_or_else(|| info.agent.clone());
        store(app, run_id, &info, &[(model, from_acp(u), a.cost_usd)], "acp").await;
        return true;
    }
    false
}

/// Fill in usage for runs that finished before usage tracking existed (or whose logs appeared later).
pub async fn backfill(app: &AppState) {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT r.id FROM agent_runs r WHERE r.acp_session_id IS NOT NULL
            AND r.status NOT IN ('queued','preparing','running')
            AND NOT EXISTS (SELECT 1 FROM run_usage u WHERE u.run_id = r.id AND u.source != 'acp')",
    )
    .fetch_all(&app.db)
    .await
    .unwrap_or_default();
    let mut n = 0;
    for id in ids {
        if refresh_run(app, id, None).await {
            n += 1;
        }
    }
    if n > 0 {
        tracing::info!("usage: backfilled {n} runs from agent session logs");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/09/28");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-09-28T00-00-00-abc-123.jsonl"),
            [
                r#"{"type":"session_meta","payload":{"id":"abc-123"}}"#,
                r#"{"type":"turn_context","payload":{"model":"gpt-6-codex"}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":60,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110}},"rate_limits":{"primary":{"used_percent":28.0}}}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":200,"cache_write_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":9,"total_tokens":330}},"rate_limits":null}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let log = codex_log("abc-123", &[dir.path().to_path_buf()]).unwrap();
        assert_eq!(log.model.as_deref(), Some("gpt-6-codex"));
        assert_eq!(log.tokens, Tokens { input: 100, cached_input: 200, cache_write: 0, output: 30, reasoning: 9, total: 330 });
        assert!(log.rate_limits.is_some());
    }

    #[test]
    fn acp_usage() {
        let t = from_acp(&serde_json::json!({"inputTokens": 5, "cachedReadTokens": 50, "cachedWriteTokens": 7, "outputTokens": 3, "totalTokens": 65}));
        assert_eq!(t.total, 65);
        assert_eq!(t.cached_input, 50);
    }
}
