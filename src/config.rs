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
    pub secrets: Secrets,
}

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
        let public_url = format!("http://{bind}");
        let container_url = format!("http://host.docker.internal:{}", bind.port());
        Ok(Config { data_dir, bind, public_url, container_url, no_auth, secrets })
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
    pub fn api_url(&self) -> String {
        format!("{}/api", self.public_url)
    }
}

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
