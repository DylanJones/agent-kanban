//! Host-mode adapter commands must pick up new adapter releases (and with them new models).

use agent_kanban::db;

async fn args_of(pool: &db::Db, slug: &str) -> String {
    sqlx::query_scalar("SELECT args FROM agent_definitions WHERE slug = ?").bind(slug).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn default_adapters_run_the_latest_release() {
    let tmp = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", tmp.path().join("db.sqlite").display());
    let pool = db::connect(&url).await.unwrap();
    // Plain `npx -y pkg` keeps using whatever copy is already installed or cached.
    assert_eq!(args_of(&pool, "codex").await, r#"["-y","@agentclientprotocol/codex-acp@latest"]"#);
    assert_eq!(args_of(&pool, "claude").await, r#"["-y","@agentclientprotocol/claude-agent-acp@latest"]"#);

    // Databases seeded before this change are upgraded; edited args are left alone.
    sqlx::query("UPDATE agent_definitions SET args = ? WHERE slug = 'codex'")
        .bind(r#"["-y","@agentclientprotocol/codex-acp"]"#)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_definitions SET args = ? WHERE slug = 'claude'")
        .bind(r#"["-y","@agentclientprotocol/claude-agent-acp@0.84.0"]"#)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let pool = db::connect(&url).await.unwrap();
    assert_eq!(args_of(&pool, "codex").await, r#"["-y","@agentclientprotocol/codex-acp@latest"]"#);
    assert_eq!(args_of(&pool, "claude").await, r#"["-y","@agentclientprotocol/claude-agent-acp@0.84.0"]"#);
}
