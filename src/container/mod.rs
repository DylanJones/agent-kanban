//! Optional per-project container sandbox for agent runs (Docker).
//!
//! The image is built in two stages: the project's own Dockerfile (toolchain), plus an overlay
//! that adds Node, git, curl, ccache, the ACP adapters and a non-root user matching the host uid.

use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::AppState;
use crate::db;
use crate::domain::models::{AgentDefinition, Project};

pub const MANAGED_LABEL: &str = "akb.managed=1";

const OVERLAY: &str = r#"
FROM {{BASE}}
USER root
COPY --from=node:24-slim /usr/local /usr/local
RUN (command -v apt-get >/dev/null && apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends git curl ca-certificates ccache bash && rm -rf /var/lib/apt/lists/*) \
 || (command -v apk >/dev/null && apk add --no-cache git curl ca-certificates ccache bash) || true
RUN npm install -g @agentclientprotocol/claude-agent-acp @agentclientprotocol/codex-acp && npm cache clean --force
RUN (getent passwd {{UID}} >/dev/null && userdel -f $(getent passwd {{UID}} | cut -d: -f1) || true) \
 && (getent group {{GID}} >/dev/null || groupadd -g {{GID}} agent) \
 && useradd -m -u {{UID}} -g {{GID}} -s /bin/bash agent \
 && mkdir -p /home/agent/.ccache /home/agent/.codex && chown -R {{UID}}:{{GID}} /home/agent
ENV IS_SANDBOX=1 CCACHE_DIR=/home/agent/.ccache HOME=/home/agent
USER agent
"#;

const DEFAULT_BASE: &str = "FROM debian:bookworm-slim\nRUN apt-get update && apt-get install -y --no-install-recommends build-essential python3 && rm -rf /var/lib/apt/lists/*\n";

fn ids() -> (u32, u32) {
    unsafe { (libc::getuid(), libc::getgid()) }
}

pub async fn docker(args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("docker").args(args).output().await.map_err(|e| anyhow::anyhow!("docker not available: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("docker {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn build(tag: &str, dockerfile: &str, context: &Path, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let df_path = std::env::temp_dir().join(format!("akb-{}.Dockerfile", tag.replace(['/', ':'], "-")));
    tokio::fs::write(&df_path, dockerfile).await?;
    let mut child = Command::new("docker")
        .args(["build", "--progress=plain", "-t", tag, "-f"])
        .arg(&df_path)
        .arg(context)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        tokio::select! {
            l = err.next_line() => match l? { Some(l) => log(l), None => break },
            l = out.next_line() => if let Some(l) = l? { log(l) },
        }
    }
    let status = child.wait().await?;
    let _ = tokio::fs::remove_file(&df_path).await;
    let _ = tokio::io::stdout().flush().await;
    if !status.success() {
        anyhow::bail!("docker build failed for {tag}");
    }
    Ok(())
}

/// Build the base + overlay images for a project and record the tag.
pub async fn build_image(app: &AppState, project: &Project, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<String> {
    let base_df = project.container_dockerfile.clone().unwrap_or_else(|| DEFAULT_BASE.to_string());
    let (uid, gid) = ids();
    let overlay_tmpl = OVERLAY.replace("{{UID}}", &uid.to_string()).replace("{{GID}}", &gid.to_string());
    let hash = hex::encode(&Sha256::digest(format!("{base_df}\n---\n{overlay_tmpl}").as_bytes())[..6]);
    let base_tag = format!("akb-base/{}:{hash}", project.slug);
    let tag = format!("akb/{}:{hash}", project.slug);
    let context = project.container_context.clone().unwrap_or_else(|| project.repo_path.clone());
    log(format!("building base image {base_tag} (context {context})"));
    build(&base_tag, &base_df, Path::new(&context), log).await?;
    let overlay = overlay_tmpl.replace("{{BASE}}", &base_tag);
    let empty = app.config.tmp_dir().join("empty-context");
    tokio::fs::create_dir_all(&empty).await?;
    log(format!("building agent overlay {tag}"));
    build(&tag, &overlay, &empty, log).await?;
    sqlx::query("UPDATE projects SET container_image = ?, updated_at = ? WHERE id = ?")
        .bind(&tag)
        .bind(db::now())
        .bind(project.id)
        .execute(&app.db)
        .await?;
    app.bus.emit("projects.updated", Some(&project.slug), None, None, None);
    log(format!("image ready: {tag}"));
    Ok(tag)
}

pub struct ContainerLaunch {
    pub name: String,
    pub args: Vec<String>,
}

/// `docker run -i` arguments that run the agent's adapter in the project image with the worktree
/// and the repo's common git dir mounted at identical paths (so worktree gitdir pointers resolve).
pub async fn run_args(
    app: &AppState,
    project: &Project,
    agent: &AgentDefinition,
    run_id: i64,
    worktree: &Path,
    env: &[(String, String)],
) -> anyhow::Result<ContainerLaunch> {
    let image =
        project.container_image.clone().ok_or_else(|| anyhow::anyhow!("container image not built yet (Project settings → Build image)"))?;
    let name = format!("akb-run-{run_id}");
    let common = crate::git::common_dir(&project.repo_path).await?;
    let wt = worktree.to_string_lossy().to_string();
    let mut a: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--name".into(),
        name.clone(),
        "--label".into(),
        MANAGED_LABEL.into(),
        "--label".into(),
        format!("akb.run={run_id}"),
        "-v".into(),
        format!("{wt}:{wt}"),
        "-v".into(),
        format!("{common}:{common}"),
        "-v".into(),
        format!("akb-ccache-{}:/home/agent/.ccache", project.slug),
        "-w".into(),
        wt.clone(),
        "--add-host=host.docker.internal:host-gateway".into(),
    ];
    let mut envs: Vec<(String, String)> = env.to_vec();
    for (k, v) in agent.env.0.iter() {
        if k != "CODEX_CONFIG" && k != "INITIAL_AGENT_MODE" {
            envs.push((k.clone(), v.clone()));
        }
    }
    match agent.harness.as_str() {
        "claude" => {
            if let Some(t) = &app.config.secrets.claude_code_oauth_token {
                envs.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), t.clone()));
            }
        }
        "codex" => {
            envs.push(("INITIAL_AGENT_MODE".into(), "agent-full-access".into()));
            if let Some(home) = dirs::home_dir() {
                let auth = home.join(".codex/auth.json");
                if auth.exists() {
                    a.push("-v".into());
                    a.push(format!("{}:/home/agent/.codex/auth.json", auth.display()));
                }
            }
        }
        _ => {}
    }
    for (k, v) in &app.config.secrets.container_env {
        envs.push((k.clone(), v.clone()));
    }
    for (name_key, cfg) in [("GIT_AUTHOR_NAME", "user.name"), ("GIT_AUTHOR_EMAIL", "user.email")] {
        if let Ok(v) = crate::git::run(&project.repo_path, &["config", cfg]).await {
            envs.push((name_key.into(), v.clone()));
            envs.push((name_key.replace("AUTHOR", "COMMITTER"), v));
        }
    }
    for (k, v) in envs {
        a.push("-e".into());
        a.push(format!("{k}={v}"));
    }
    a.extend(project.container_extra_args.0.iter().cloned());
    a.push(image);
    let cmd: Vec<String> = match &agent.container_command {
        Some(c) if !c.0.is_empty() => c.0.clone(),
        _ => std::iter::once(agent.command.clone()).chain(agent.args.0.iter().cloned()).collect(),
    };
    a.extend(cmd);
    Ok(ContainerLaunch { name, args: a })
}

pub async fn kill(name: &str) {
    let _ = docker(&["rm", "-f", name]).await;
}

/// Remove containers left behind by a previous server process.
pub async fn sweep_orphans() {
    if let Ok(ids) = docker(&["ps", "-aq", "--filter", &format!("label={MANAGED_LABEL}")]).await {
        for id in ids.lines().filter(|l| !l.is_empty()) {
            let _ = docker(&["rm", "-f", id]).await;
        }
    }
}
