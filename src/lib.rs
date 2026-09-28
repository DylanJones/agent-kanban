//! agent-kanban: a local kanban board that orchestrates coding agents over ACP.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

pub mod acp;
pub mod api;
pub mod auth;
pub mod bus;
pub mod config;
pub mod container;
pub mod db;
pub mod domain;
pub mod error;
pub mod git;
pub mod github;
pub mod jobs;
pub mod orchestrator;
pub mod services;
pub mod usage;
pub mod web;

use std::sync::Arc;

use bus::Bus;
use config::Config;
use db::Db;
use orchestrator::registry::Registry;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub bus: Bus,
    pub config: Arc<Config>,
    pub runs: Arc<Registry>,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let url = format!("sqlite://{}", config.db_path().display());
        let db = db::connect(&url).await?;
        Ok(AppState { db, bus: Bus::new(), config: Arc::new(config), runs: Arc::new(Registry::default()) })
    }

    /// A fresh data dir (and database) for tests, with the given externally reachable URL.
    pub async fn for_test(data_dir: &std::path::Path, public_url: &str) -> anyhow::Result<Self> {
        let mut config = Config::load(Some(data_dir.to_path_buf()), "127.0.0.1:0".parse().unwrap(), false)?;
        // Tests drive the fake agent directly on the host.
        config.allow_host_agents = true;
        config.public_url = public_url.to_string();
        Self::new(config).await
    }

    /// Start background workers (scheduler, limits, ref scanner, jobs).
    pub async fn start_background(&self) -> anyhow::Result<()> {
        orchestrator::reconcile(self).await?;
        orchestrator::start(self);
        git::scanner::spawn(self.clone());
        jobs::spawn_worker(self.clone());
        let app = self.clone();
        tokio::spawn(async move { usage::backfill(&app).await });
        Ok(())
    }
}
