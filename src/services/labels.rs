use sqlx::SqliteConnection;

use crate::db::Db;
use crate::domain::models::Label;

/// Default colors for labels created on the fly.
fn default_color(name: &str) -> &'static str {
    match name {
        "bug" => "d73a4a",
        "enhancement" => "a2eeef",
        "documentation" => "0075ca",
        "question" => "d876e3",
        "performance" => "fbca04",
        "found-by-agent" => "5319e7",
        _ => "8b949e",
    }
}

pub async fn ensure_label(conn: &mut SqliteConnection, project_id: i64, name: &str) -> sqlx::Result<i64> {
    let name = name.trim();
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM labels WHERE project_id = ? AND name = ?")
        .bind(project_id)
        .bind(name)
        .fetch_optional(&mut *conn)
        .await?
    {
        return Ok(id);
    }
    sqlx::query_scalar("INSERT INTO labels(project_id, name, color) VALUES (?, ?, ?) RETURNING id")
        .bind(project_id)
        .bind(name)
        .bind(default_color(name))
        .fetch_one(conn)
        .await
}

pub async fn set_issue_labels(conn: &mut SqliteConnection, project_id: i64, issue_id: i64, names: &[String]) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = ?").bind(issue_id).execute(&mut *conn).await?;
    add_issue_labels(conn, project_id, issue_id, names).await
}

pub async fn add_issue_labels(conn: &mut SqliteConnection, project_id: i64, issue_id: i64, names: &[String]) -> sqlx::Result<()> {
    for n in names.iter().filter(|n| !n.trim().is_empty()) {
        let lid = ensure_label(conn, project_id, n).await?;
        sqlx::query("INSERT OR IGNORE INTO issue_labels(issue_id, label_id) VALUES (?, ?)")
            .bind(issue_id)
            .bind(lid)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

pub async fn issue_labels(db: &Db, issue_id: i64) -> sqlx::Result<Vec<Label>> {
    sqlx::query_as::<_, Label>("SELECT l.* FROM labels l JOIN issue_labels il ON il.label_id = l.id WHERE il.issue_id = ? ORDER BY l.name")
        .bind(issue_id)
        .fetch_all(db)
        .await
}

pub async fn project_labels(db: &Db, project_id: i64) -> sqlx::Result<Vec<Label>> {
    sqlx::query_as::<_, Label>("SELECT * FROM labels WHERE project_id = ? ORDER BY name").bind(project_id).fetch_all(db).await
}
