//! ACP client: spawns an adapter process (on the host or in a container), speaks the Agent
//! Client Protocol over its stdio, and relays session updates and permission prompts over channels.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, HttpHeader, Implementation, InitializeRequest, McpServer, McpServerHttp, NewSessionRequest, PermissionOption, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

/// A JSON-RPC error returned by the agent.
#[derive(Debug, Clone, Serialize)]
pub struct AcpError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)?;
        if let Some(d) = &self.data {
            write!(f, " {d}")?;
        }
        Ok(())
    }
}

impl From<agent_client_protocol::Error> for AcpError {
    fn from(e: agent_client_protocol::Error) -> Self {
        AcpError { code: i32::from(e.code), message: e.message, data: e.data }
    }
}

pub enum Ctl {
    Prompt(String),
    Close,
}

pub enum Out {
    Initialized {
        agent_info: Value,
        session_id: String,
        modes: Value,
    },
    /// A `SessionNotification` serialized as JSON (camelCase ACP wire format).
    Update(Value),
    Permission {
        tool_call: Value,
        options: Vec<PermissionOption>,
        reply: oneshot::Sender<Option<String>>,
    },
    TurnEnded(Result<Value, AcpError>),
    Stderr(String),
    /// The session's configuration options (model, effort, ...) after applying the requested
    /// values, plus which requested values were applied or skipped (with a reason).
    Config { options: Value, applied: Vec<(String, Value)>, skipped: Vec<(String, String)> },
}

/// Extra session configuration: the board's MCP server and additional writable directories.
#[derive(Debug, Clone, Default)]
pub struct SessionExtras {
    /// (url, bearer token) of the board MCP endpoint.
    pub mcp: Option<(String, String)>,
    pub additional_directories: Vec<PathBuf>,
    /// Session config values to set (option id → value), e.g. `model`, `effort`/`reasoning_effort`.
    pub config: Vec<(String, Value)>,
}

pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// All selectable values of a config option (flattening grouped selects).
pub fn option_values(opt: &Value) -> Vec<String> {
    let mut out = vec![];
    for o in opt.get("options").and_then(Value::as_array).into_iter().flatten() {
        if let Some(v) = o.get("value").and_then(Value::as_str) {
            out.push(v.to_string());
        }
        for inner in o.get("options").and_then(Value::as_array).into_iter().flatten() {
            if let Some(v) = inner.get("value").and_then(Value::as_str) {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// Validate a requested config value against the session's options. `Ok(None)`: already set.
fn check_config_value(options: &Value, id: &str, value: &Value) -> Result<Option<Value>, String> {
    let Some(opt) = options.as_array().and_then(|a| a.iter().find(|o| o.get("id").and_then(Value::as_str) == Some(id))) else {
        return Err(format!("this agent has no `{id}` setting"));
    };
    if opt.get("currentValue") == Some(value) {
        return Ok(None);
    }
    match (opt.get("type").and_then(Value::as_str), value) {
        (Some("boolean"), Value::Bool(_)) => Ok(Some(value.clone())),
        (Some("boolean"), _) => Err(format!("`{id}` is on/off")),
        (_, Value::String(v)) => {
            let values = option_values(opt);
            if values.iter().any(|x| x == v) {
                Ok(Some(value.clone()))
            } else {
                Err(format!("`{v}` isn't available for `{id}` (available: {})", values.join(", ")))
            }
        }
        _ => Err(format!("unsupported value for `{id}`")),
    }
}

/// Spawn the adapter in its own process group so the whole tree can be killed.
pub fn spawn(launch: &Launch) -> anyhow::Result<Child> {
    let mut c = Command::new(&launch.program);
    c.args(&launch.args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    for (k, v) in &launch.env {
        c.env(k, v);
    }
    let child = c.spawn().map_err(|e| anyhow::anyhow!("failed to start `{}`: {e}", launch.program))?;
    Ok(child)
}

/// Kill the process group: SIGTERM, then SIGKILL after a grace period.
pub async fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
            let _ = child.wait().await;
        }
    }
}

/// Drive one ACP session over the child's stdio until `Ctl::Close`, cancellation, or disconnect.
pub async fn drive(
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: Option<ChildStderr>,
    cwd: PathBuf,
    mode: Option<String>,
    session: SessionExtras,
    out: mpsc::UnboundedSender<Out>,
    mut ctl: mpsc::UnboundedReceiver<Ctl>,
    cancel: CancellationToken,
) -> Result<(), AcpError> {
    if let Some(stderr) = stderr {
        let o = out.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let _ = o.send(Out::Stderr(l));
            }
        });
    }
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let out_n = out.clone();
    let out_p = out.clone();

    let result = Client
        .builder()
        .name("agent-kanban")
        .on_receive_notification(
            async move |n: SessionNotification, _cx| {
                let _ = out_n.send(Out::Update(serde_json::to_value(&n).unwrap_or(Value::Null)));
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |r: RequestPermissionRequest, responder: Responder<RequestPermissionResponse>, cx: ConnectionTo<Agent>| {
                let (tx, rx) = oneshot::channel();
                let tool_call = serde_json::to_value(&r.tool_call).unwrap_or(Value::Null);
                let _ = out_p.send(Out::Permission { tool_call, options: r.options.clone(), reply: tx });
                // Answer off the dispatch loop so a slow human doesn't block the session.
                cx.spawn(async move {
                    let outcome = match rx.await.ok().flatten() {
                        Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                        None => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            let init = cx
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("agent-kanban", env!("CARGO_PKG_VERSION"))),
                )
                .block_task()
                .await?;
            let mut new_session = NewSessionRequest::new(cwd.clone()).additional_directories(session.additional_directories.clone());
            if let Some((url, token)) = &session.mcp
                && init.agent_capabilities.mcp_capabilities.http
            {
                new_session = new_session.mcp_servers(vec![McpServer::Http(
                    McpServerHttp::new("agent-kanban", url.clone()).headers(vec![HttpHeader::new("Authorization", format!("Bearer {token}"))]),
                )]);
            }
            let sess = cx.send_request(new_session).block_task().await?;
            let sid = sess.session_id.clone();
            let _ = out.send(Out::Initialized {
                agent_info: serde_json::to_value(&init.agent_info).unwrap_or(Value::Null),
                session_id: sid.0.to_string(),
                modes: serde_json::to_value(&sess.modes).unwrap_or(Value::Null),
            });
            let mut options = serde_json::to_value(&sess.config_options).unwrap_or(Value::Null);
            if let (Some(m), Some(modes)) = (&mode, &sess.modes)
                && modes.available_modes.iter().any(|am| am.id.0.as_ref() == m.as_str())
            {
                cx.send_request(SetSessionModeRequest::new(sid.clone(), m.clone())).block_task().await?;
                // Keep the reported settings in step with the mode we just selected.
                if let Some(o) = options.as_array_mut().and_then(|a| a.iter_mut().find(|o| o.get("id").and_then(Value::as_str) == Some("mode"))) {
                    o["currentValue"] = Value::String(m.clone());
                }
            }
            let (mut applied, mut skipped) = (vec![], vec![]);
            // Model first: changing it can change which effort levels exist.
            let mut wanted = session.config.clone();
            wanted.sort_by_key(|(id, _)| if id == "model" { 0 } else { 1 });
            for (id, value) in wanted {
                match check_config_value(&options, &id, &value) {
                    Err(why) => skipped.push((id, why)),
                    Ok(None) => {} // already set
                    Ok(Some(v)) => {
                        let req = match &v {
                            Value::Bool(b) => SetSessionConfigOptionRequest::new(sid.clone(), id.clone(), *b),
                            other => SetSessionConfigOptionRequest::new(sid.clone(), id.clone(), other.as_str().unwrap_or_default()),
                        };
                        match cx.send_request(req).block_task().await {
                            Ok(r) => {
                                options = serde_json::to_value(&r.config_options).unwrap_or(options);
                                applied.push((id, v));
                            }
                            Err(e) => skipped.push((id, e.message)),
                        }
                    }
                }
            }
            if !options.is_null() || !skipped.is_empty() {
                let _ = out.send(Out::Config { options, applied, skipped });
            }
            loop {
                let cmd = tokio::select! {
                    c = ctl.recv() => c,
                    _ = cancel.cancelled() => None,
                };
                let Some(Ctl::Prompt(text)) = cmd else { break };
                let fut = cx.send_request(PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new(text))])).block_task();
                tokio::pin!(fut);
                let res = tokio::select! {
                    r = &mut fut => r,
                    _ = cancel.cancelled() => {
                        let _ = cx.send_notification(CancelNotification::new(sid.clone()));
                        match tokio::time::timeout(Duration::from_secs(10), &mut fut).await {
                            Ok(r) => r,
                            Err(_) => Err(agent_client_protocol::Error::internal_error().data(serde_json::json!("cancel timed out"))),
                        }
                    }
                };
                let _ = out.send(Out::TurnEnded(res.map(|r| serde_json::to_value(&r).unwrap_or(Value::Null)).map_err(AcpError::from)));
                if cancel.is_cancelled() {
                    break;
                }
            }
            Ok(())
        })
        .await;
    result.map_err(AcpError::from)
}
