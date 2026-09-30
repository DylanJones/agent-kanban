//! SQLite pool, migrations, and small shared helpers.

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool, Transaction};

pub type Db = SqlitePool;
pub type Tx = Transaction<'static, Sqlite>;

pub async fn connect(url: &str) -> anyhow::Result<Db> {
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(10))
        .foreign_keys(true);
    let in_memory = url.contains(":memory:");
    let pool = SqlitePoolOptions::new()
        // An in-memory database exists per connection, so tests use exactly one.
        .max_connections(if in_memory { 1 } else { 8 })
        .connect_with(opts)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    seed_defaults(&pool).await?;
    Ok(pool)
}

/// Begin a write transaction that takes the write lock up front, so check-then-write is atomic.
pub async fn begin_write(db: &Db) -> sqlx::Result<Tx> {
    db.begin_with("BEGIN IMMEDIATE").await
}

pub fn now() -> String {
    fmt_time(Utc::now())
}

pub fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

pub async fn get_setting<T: DeserializeOwned>(db: &Db, key: &str) -> Option<T> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?").bind(key).fetch_optional(db).await.ok().flatten();
    v.and_then(|s| serde_json::from_str(&s).ok())
}

pub async fn set_setting<T: Serialize>(db: &Db, key: &str, value: &T) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(serde_json::to_string(value).unwrap())
        .execute(db)
        .await?;
    Ok(())
}

/// Default agent definitions and settings, inserted once.
async fn seed_defaults(db: &Db) -> anyhow::Result<()> {
    let defaults: &[(&str, serde_json::Value)] = &[
        ("max_concurrent_runs", serde_json::json!(3)),
        ("scheduler_enabled", serde_json::json!(false)),
        ("run_timeouts_minutes", serde_json::json!({"triage": 20, "fix": 120, "review": 40, "merge_prep": 60})),
        ("default_role_agents", serde_json::json!({"triage": "claude", "fix": "claude", "review": "codex", "merge_prep": "claude"})),
        ("max_failures", serde_json::json!(3)),
    ];
    for (k, v) in defaults {
        sqlx::query("INSERT OR IGNORE INTO settings(key, value) VALUES (?, ?)").bind(*k).bind(v.to_string()).execute(db).await?;
    }

    let now = now();
    let codex_host_config = serde_json::json!({
        "sandbox_workspace_write": { "network_access": true }
    })
    .to_string();
    let agents: &[(&str, &str, &str, &str, serde_json::Value, serde_json::Value, Option<&str>, Option<&str>)] = &[
        (
            "claude",
            "Claude Code",
            "claude",
            "npx",
            serde_json::json!(["-y", "@agentclientprotocol/claude-agent-acp@latest"]),
            serde_json::json!({}),
            Some("auto"),
            Some("bypassPermissions"),
        ),
        (
            "codex",
            "Codex",
            "codex",
            "npx",
            serde_json::json!(["-y", "@agentclientprotocol/codex-acp@latest"]),
            serde_json::json!({"CODEX_CONFIG": codex_host_config, "INITIAL_AGENT_MODE": "agent", "NO_BROWSER": "1"}),
            Some("agent"),
            Some("agent-full-access"),
        ),
        ("opencode", "OpenCode", "opencode", "opencode", serde_json::json!(["acp"]), serde_json::json!({}), None, None),
    ];
    for (slug, name, harness, cmd, args, env, mode, cmode) in agents {
        let container_cmd = match *harness {
            "claude" => Some(serde_json::json!(["claude-agent-acp"]).to_string()),
            "codex" => Some(serde_json::json!(["codex-acp"]).to_string()),
            _ => None,
        };
        sqlx::query(
            "INSERT OR IGNORE INTO agent_definitions
               (slug, name, harness, command, args, env, container_command, limit_group, max_concurrent,
                permission_policy, container_permission_policy, permission_rules, session_mode_id, container_session_mode_id,
                enabled, needs_auth, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 2, 'allowlist', 'auto_allow', ?, ?, ?, 1, 0, ?, ?)",
        )
        .bind(*slug)
        .bind(*name)
        .bind(*harness)
        .bind(*cmd)
        .bind(args.to_string())
        .bind(env.to_string())
        .bind(container_cmd)
        .bind(*harness)
        .bind(serde_json::to_string(&crate::orchestrator::permissions::default_rules()).unwrap())
        .bind(*mode)
        .bind(*cmode)
        .bind(&now)
        .bind(&now)
        .execute(db)
        .await?;
        sqlx::query("INSERT OR IGNORE INTO limit_groups(name, updated_at) VALUES (?, ?)").bind(*harness).bind(&now).execute(db).await?;
    }
    // Without a version, `npx -y` keeps running whatever copy is already installed or cached, so
    // new adapter releases (and the models they add) never arrive. Upgrade the old unedited defaults.
    for pkg in ["@agentclientprotocol/claude-agent-acp", "@agentclientprotocol/codex-acp"] {
        sqlx::query("UPDATE agent_definitions SET args = ? WHERE args = ?")
            .bind(serde_json::json!(["-y", format!("{pkg}@latest")]).to_string())
            .bind(serde_json::json!(["-y", pkg]).to_string())
            .execute(db)
            .await?;
    }
    Ok(())
}
