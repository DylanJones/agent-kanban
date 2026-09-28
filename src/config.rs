//! Runtime configuration: data directory layout, bind address, secrets.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Base URL humans and host agents use, e.g. `http://127.0.0.1:7878`.
    pub public_url: String,
    /// Base URL agents inside containers use to reach the API.
    pub container_url: String,
    /// Disable human auth (every request without a run token is treated as the admin).
    pub no_auth: bool,
    /// Let agents run directly on this machine for projects without a container
    /// (`serve --dangerously-allow-host-agents`). Off by default: agents only run in Docker.
    pub allow_host_agents: bool,
    pub secrets: Secrets,
}

/// Why an agent run was refused for a project without a container.
pub const HOST_AGENTS_OFF: &str = "agents only run in Docker containers: turn on \"Run agents in containers\" in the project's settings, \
or start the server with --dangerously-allow-host-agents to let agents run unsandboxed on this machine";

/// Contents of `secrets.toml` (mode 0600).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    /// Admin token used for the browser login link and as a bearer token.
    pub admin_token: String,
    /// Long-lived Claude OAuth token (`claude setup-token`), passed to containers only.
    #[serde(default)]
    pub claude_code_oauth_token: Option<String>,
    /// Extra env vars injected into containers only.
    #[serde(default)]
    pub container_env: std::collections::BTreeMap<String, String>,
}

impl Config {
    pub fn load(data_dir: Option<PathBuf>, bind: SocketAddr, no_auth: bool) -> anyhow::Result<Self> {
        let data_dir = data_dir.unwrap_or_else(default_data_dir);
        std::fs::create_dir_all(&data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        for sub in ["worktrees", "runs", "tmp"] {
            std::fs::create_dir_all(data_dir.join(sub))?;
        }
        let secrets = load_or_create_secrets(&data_dir.join("secrets.toml"))?;
        // Agents and links on this machine always use loopback; binding to all interfaces
        // (0.0.0.0) just makes the server reachable from other machines too.
        let host = if bind.ip().is_unspecified() || bind.ip().is_loopback() { "127.0.0.1".to_string() } else { bind.ip().to_string() };
        let public_url = format!("http://{host}:{}", bind.port());
        let container_url = format!("http://host.docker.internal:{}", bind.port());
        Ok(Config { data_dir, bind, public_url, container_url, no_auth, allow_host_agents: false, secrets })
    }

    /// Only one server may use a data directory: a second one would mark the first one's runs
    /// interrupted and sweep its containers at startup. The lock is held until the returned value drops.
    pub fn lock_for_serve(&self) -> anyhow::Result<ServeLock> {
        use std::os::fd::AsRawFd;
        let path = self.data_dir.join("serve.lock");
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).with_context(|| format!("opening {}", path.display()))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            anyhow::bail!(
                "another agent-kanban server is already using {}; give this one its own data directory (--data-dir or AKB_DATA_DIR)",
                self.data_dir.display()
            );
        }
        Ok(ServeLock(file))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("db.sqlite")
    }
    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir.join("worktrees")
    }
    pub fn runs_dir(&self) -> PathBuf {
        self.data_dir.join("runs")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }
    /// Re-read `secrets.toml`, so credentials added while the server runs (e.g. a Claude token
    /// for containers) take effect on the next run without a restart.
    pub fn current_secrets(&self) -> Secrets {
        std::fs::read_to_string(self.data_dir.join("secrets.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_else(|| self.secrets.clone())
    }

    /// Identifies this server instance (by data dir) on resources it creates, e.g. container labels,
    /// so several instances on one machine never clean up each other's containers.
    pub fn instance_id(&self) -> String {
        use sha2::Digest;
        hex::encode(&sha2::Sha256::digest(self.data_dir.to_string_lossy().as_bytes())[..6])
    }

    pub fn api_url(&self) -> String {
        format!("{}/api", self.public_url)
    }
}

pub struct ServeLock(#[allow(dead_code)] std::fs::File);

pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AKB_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".agent-kanban")
}

fn load_or_create_secrets(path: &Path) -> anyhow::Result<Secrets> {
    if path.exists() {
        let text = std::fs::read_to_string(path)?;
        let s: Secrets = toml::from_str(&text).context("parsing secrets.toml")?;
        if !s.admin_token.is_empty() {
            return Ok(s);
        }
    }
    let s = Secrets { admin_token: format!("akh_{}", crate::auth::random_token()), ..Default::default() };
    write_secrets(path, &s)?;
    Ok(s)
}

pub fn write_secrets(path: &Path, s: &Secrets) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(toml::to_string_pretty(s)?.as_bytes())?;
    Ok(())
}
