//! ACP client: spawns an adapter process (on the host or in a container), speaks the Agent
//! Client Protocol over its stdio, and relays session updates and permission prompts over channels.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Implementation, InitializeRequest, NewSessionRequest, PermissionOption, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification,
    SetSessionModeRequest, TextContent,
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
}

pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
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
            let sess = cx.send_request(NewSessionRequest::new(cwd.clone())).block_task().await?;
            let sid = sess.session_id.clone();
            let _ = out.send(Out::Initialized {
                agent_info: serde_json::to_value(&init.agent_info).unwrap_or(Value::Null),
                session_id: sid.0.to_string(),
                modes: serde_json::to_value(&sess.modes).unwrap_or(Value::Null),
            });
            if let (Some(m), Some(modes)) = (&mode, &sess.modes)
                && modes.available_modes.iter().any(|am| am.id.0.as_ref() == m.as_str())
            {
                cx.send_request(SetSessionModeRequest::new(sid.clone(), m.clone())).block_task().await?;
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
