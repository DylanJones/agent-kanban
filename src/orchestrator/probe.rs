//! Lightweight agent sessions outside of issue work: availability probes and "Test" checks.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::limits::{self, Evidence, LimitSignal};
use crate::AppState;
use crate::acp::{self, Ctl, Out};
use crate::domain::models::AgentDefinition;

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentTestResult {
    pub ok: bool,
    #[schema(value_type = Option<Object>)]
    pub agent_info: Option<Value>,
    #[schema(value_type = Option<Object>)]
    pub modes: Option<Value>,
    /// Reply to the test prompt, if one was sent.
    pub reply: Option<String>,
    pub error: Option<String>,
    /// `quota`, `rate`, `auth` if a limit was detected.
    pub limit: Option<String>,
    pub stderr_tail: String,
    /// Settings the agent offers (ACP `configOptions`), also cached on the agent.
    #[schema(value_type = Option<Vec<Object>>)]
    pub config_options: Option<Value>,
}

/// Start the adapter in a scratch dir; optionally send a prompt. Never touches a repository.
/// `project`: run it the way that project's runs are launched (e.g. in its container).
/// `extra_env`: extra environment, e.g. a credential being tested before it's saved.
pub async fn session_check(
    app: &AppState,
    agent: &AgentDefinition,
    project: Option<&crate::domain::models::Project>,
    extra_env: Vec<(String, String)>,
    prompt: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<AgentTestResult> {
    let dir = app.config.tmp_dir().join(format!("probe-{}", agent.slug));
    tokio::fs::create_dir_all(&dir).await?;
    let plan = super::run::plan_launch(app, project, agent, 0, &dir, extra_env).await?;
    let mut child = acp::spawn(&plan.launch)?;
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let drive = tokio::spawn(acp::drive(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take(),
        dir.clone(),
        plan.mode.clone(),
        acp::SessionExtras { config: agent.session_config.0.clone().into_iter().collect(), ..Default::default() },
        out_tx,
        ctl_rx,
        cancel.clone(),
    ));
    let mut res =
        AgentTestResult { ok: false, agent_info: None, modes: None, reply: None, error: None, limit: None, stderr_tail: String::new(), config_options: None };
    let mut reply = String::new();
    let mut stderr: Vec<String> = vec![];
    let mut meta: Option<Value> = None;
    let mut turn_err = None;
    let work = async {
        while let Some(ev) = out_rx.recv().await {
            match ev {
                Out::Initialized { agent_info, modes, .. } => {
                    if let (Some(m), Some(avail)) = (&plan.mode, modes.get("availableModes").and_then(Value::as_array))
                        && !avail.iter().any(|a| a.get("id").and_then(Value::as_str) == Some(m.as_str()))
                    {
                        res.error = Some(format!("configured session mode `{m}` is not offered by this agent"));
                    }
                    res.agent_info = Some(agent_info);
                    res.modes = Some(modes);
                    match prompt {
                        Some(p) => {
                            let _ = ctl_tx.send(Ctl::Prompt(p.to_string()));
                        }
                        None => {
                            res.ok = true;
                            let _ = ctl_tx.send(Ctl::Close);
                        }
                    }
                }
                Out::Update(v) => {
                    let u = v.get("update").unwrap_or(&v);
                    if u.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk") {
                        reply.push_str(u["content"]["text"].as_str().unwrap_or(""));
                    }
                    if let Some(m) = u.get("_meta").and_then(|m| m.get("_claude/rateLimit")) {
                        meta = Some(m.clone());
                    }
                }
                Out::Stderr(l) => stderr.push(l),
                Out::Config { options, .. } => {
                    super::run::remember_config_options(app, agent.id, &options).await;
                    res.config_options = Some(options);
                }
                Out::Permission { reply, .. } => {
                    let _ = reply.send(None);
                }
                Out::TurnEnded(r) => {
                    match r {
                        Ok(_) => res.ok = true,
                        Err(e) => turn_err = Some(e),
                    }
                    let _ = ctl_tx.send(Ctl::Close);
                }
            }
        }
    };
    if tokio::time::timeout(timeout, work).await.is_err() {
        cancel.cancel();
        res.error = Some(format!("timed out after {}s", timeout.as_secs()));
        res.ok = false;
    }
    let drive_err = match tokio::time::timeout(Duration::from_secs(10), drive).await {
        Ok(Ok(Err(e))) => Some(e),
        _ => None,
    };
    acp::kill_tree(&mut child).await;
    if let Some(name) = plan.container {
        crate::container::kill(&name).await;
    }
    let err = turn_err.or(drive_err);
    let tail = stderr.iter().rev().take(30).rev().cloned().collect::<Vec<_>>().join("\n");
    let sig = limits::classify(&Evidence {
        error: err.as_ref(),
        last_message: &reply,
        stderr_tail: &tail,
        rate_limit_meta: meta.as_ref(),
        now: chrono::Utc::now(),
        tz: limits::local_tz(),
    });
    res.limit = match &sig {
        LimitSignal::Quota { .. } => Some("quota".into()),
        LimitSignal::Rate { .. } => Some("rate".into()),
        LimitSignal::Auth { .. } => Some("auth".into()),
        LimitSignal::None => None,
    };
    if res.limit.is_some() {
        res.ok = false;
    }
    if let Some(e) = err {
        res.ok = false;
        res.error.get_or_insert(e.to_string());
    }
    if res.agent_info.is_none() && res.error.is_none() {
        res.error = Some("the adapter exited before completing the ACP handshake".into());
    }
    if !reply.is_empty() {
        res.reply = Some(reply);
    }
    if res.error.is_some() {
        res.ok = false;
    }
    res.stderr_tail = tail;
    Ok(res)
}

/// Returns Ok(true) if the group's subscription is usable again.
pub async fn probe_group(app: &AppState, group: &str) -> anyhow::Result<bool> {
    let agent =
        sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE limit_group = ? AND enabled = 1 ORDER BY id LIMIT 1")
            .bind(group)
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no enabled agent in group {group}"))?;
    let r = session_check(app, &agent, None, vec![], Some("Reply with exactly: OK"), Duration::from_secs(120)).await?;
    if r.ok {
        return Ok(true);
    }
    // If the probe revealed a reset time, record it.
    if r.limit.is_some() {
        let err = r.error.clone().unwrap_or_default();
        let e = acp::AcpError { code: -32603, message: err, data: None };
        let sig = limits::classify(&Evidence {
            error: Some(&e),
            last_message: r.reply.as_deref().unwrap_or(""),
            stderr_tail: &r.stderr_tail,
            rate_limit_meta: None,
            now: chrono::Utc::now(),
            tz: limits::local_tz(),
        });
        if let LimitSignal::Quota { resets_at: Some(_), .. } | LimitSignal::Rate { retry_at: Some(_), .. } = sig {
            limits::apply_signal(app, group, agent.id, &sig).await?;
        }
    }
    Ok(false)
}
