use std::net::SocketAddr;
use std::path::PathBuf;

use agent_kanban::api::projects::NewProject;
use agent_kanban::config::Config;
use agent_kanban::{AppState, api, services};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-kanban", version, about = "Kanban board that orchestrates coding agents over ACP")]
struct Cli {
    /// Data directory (default: ~/.agent-kanban, or $AKB_DATA_DIR).
    #[arg(long, global = true, env = "AKB_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server (default).
    Serve {
        #[arg(long, default_value = "127.0.0.1:7878", env = "AKB_BIND")]
        bind: SocketAddr,
        /// Treat unauthenticated requests as the admin (local development only).
        #[arg(long)]
        no_auth: bool,
        /// Also dispatch agents automatically (same as the scheduler switch in the UI).
        #[arg(long)]
        scheduler: bool,
    },
    /// Register a git repository as a project.
    AddProject {
        slug: String,
        repo_path: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        base_branch: Option<String>,
        /// `owner/name` on GitHub.
        #[arg(long)]
        github_repo: Option<String>,
        /// GitHub Projects (v2) board owner and number, e.g. `DylanJones/1`.
        #[arg(long)]
        github_project: Option<String>,
    },
    /// Import issues, PRs, reviews and board status from GitHub.
    ImportGithub {
        #[arg(long)]
        project: String,
    },
    /// Create an API token for scripts.
    Token { name: String },
    /// Print the browser login URL.
    LoginUrl,
    /// Apply database migrations and exit.
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn,tower_http=warn".into()))
        .init();
    let cli = Cli::parse();
    let default_bind: SocketAddr = std::env::var("AKB_BIND").ok().and_then(|b| b.parse().ok()).unwrap_or(([127, 0, 0, 1], 7878).into());
    match cli.cmd.unwrap_or(Cmd::Serve { bind: default_bind, no_auth: false, scheduler: false }) {
        Cmd::Serve { bind, no_auth, scheduler } => {
            let config = Config::load(cli.data_dir, bind, no_auth)?;
            let login = format!("{}/login?t={}", config.public_url, config.secrets.admin_token);
            let app = AppState::new(config).await?;
            if scheduler {
                agent_kanban::db::set_setting(&app.db, "scheduler_enabled", &true).await?;
            }
            app.start_background().await?;
            let listener = tokio::net::TcpListener::bind(bind).await?;
            tracing::info!("agent-kanban listening on http://{bind}");
            println!("\n  Open: {login}\n  API docs: http://{bind}/api/docs\n");
            axum::serve(listener, api::app(app)).with_graceful_shutdown(shutdown()).await?;
        }
        Cmd::AddProject { slug, repo_path, name, base_branch, github_repo, github_project } => {
            let app = AppState::new(Config::load(cli.data_dir, default_bind, false)?).await?;
            let (owner, number) = match github_project.as_deref().and_then(|p| p.split_once('/')) {
                Some((o, n)) => (Some(o.to_string()), n.parse().ok()),
                None => (None, None),
            };
            let p = api::projects::create_project(
                &app,
                NewProject { slug, name, repo_path, base_branch, github_repo, github_project_owner: owner, github_project_number: number },
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("created project {} ({}, base {})", p.slug, p.repo_path, p.base_branch);
        }
        Cmd::ImportGithub { project } => {
            let app = AppState::new(Config::load(cli.data_dir, default_bind, false)?).await?;
            let p = services::project(&app.db, &project).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut log = |s: String| println!("{s}");
            let stats = agent_kanban::github::import::run(&app, &p, &mut log).await?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        Cmd::Token { name } => {
            let app = AppState::new(Config::load(cli.data_dir, default_bind, false)?).await?;
            let (_, token) = api::meta::create_api_token(&app.db, &name).await?;
            println!("{token}");
        }
        Cmd::LoginUrl => {
            let c = Config::load(cli.data_dir, default_bind, false)?;
            println!("{}/login?t={}", c.public_url, c.secrets.admin_token);
        }
        Cmd::Migrate => {
            AppState::new(Config::load(cli.data_dir, default_bind, false)?).await?;
            println!("ok");
        }
    }
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
