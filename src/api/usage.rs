use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::domain::Actor;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Deserialize, IntoParams)]
pub struct UsageQuery {
    /// Start (RFC3339 or YYYY-MM-DD). Default: 30 days ago.
    pub from: Option<String>,
    /// End (exclusive). Default: now.
    pub to: Option<String>,
    /// Project slug filter.
    pub project: Option<String>,
    /// `model` (default), `subscription`, `role`, `agent`, `project`, `issue`, `day`, `harness`.
    pub group_by: Option<String>,
}

#[derive(Debug, Default, Serialize, ToSchema, sqlx::FromRow)]
pub struct UsageRow {
    /// Group key (model name, role, day...).
    pub key: String,
    pub runs: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    /// API-equivalent cost where the agent reports one (Claude); null otherwise.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageReport {
    pub from: String,
    pub to: String,
    pub group_by: String,
    pub totals: UsageRow,
    pub rows: Vec<UsageRow>,
    /// Daily totals split by subscription, for charts.
    pub daily: Vec<DailyUsage>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct DailyUsage {
    pub day: String,
    pub subscription: String,
    pub total_tokens: i64,
    pub output_tokens: i64,
}

fn parse_bound(s: &str) -> Option<String> {
    if let Some(t) = crate::db::parse_time(s) {
        return Some(crate::db::fmt_time(t));
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| crate::db::fmt_time(d.and_hms_opt(0, 0, 0).unwrap().and_utc()))
}

const SUMS: &str = "COUNT(DISTINCT u.run_id) AS runs, COALESCE(SUM(u.input_tokens),0) AS input_tokens,
    COALESCE(SUM(u.cached_input_tokens),0) AS cached_input_tokens, COALESCE(SUM(u.cache_write_tokens),0) AS cache_write_tokens,
    COALESCE(SUM(u.output_tokens),0) AS output_tokens, COALESCE(SUM(u.reasoning_tokens),0) AS reasoning_tokens,
    COALESCE(SUM(u.total_tokens),0) AS total_tokens, SUM(u.cost_usd) AS cost_usd";

/// Token usage across agent runs, grouped by a dimension.
#[utoipa::path(operation_id = "usage_report", get, path = "/api/usage", tag = "usage", params(UsageQuery), responses((status = 200, body = UsageReport)))]
pub async fn report(State(app): State<AppState>, _a: Actor, Query(q): Query<UsageQuery>) -> ApiResult<Json<UsageReport>> {
    let from = match &q.from {
        Some(f) => parse_bound(f).ok_or_else(|| ApiError::bad("bad `from`"))?,
        None => crate::db::fmt_time(chrono::Utc::now() - chrono::Duration::days(30)),
    };
    let to = match &q.to {
        Some(t) => parse_bound(t).ok_or_else(|| ApiError::bad("bad `to`"))?,
        None => crate::db::fmt_time(chrono::Utc::now() + chrono::Duration::seconds(1)),
    };
    let project_id: Option<i64> = match &q.project {
        Some(p) => Some(crate::services::project(&app.db, p).await?.id),
        None => None,
    };
    let group_by = q.group_by.clone().unwrap_or_else(|| "model".into());
    let key = match group_by.as_str() {
        "model" => "u.model",
        "subscription" => "u.limit_group",
        "role" => "u.role",
        "agent" => "u.agent",
        "harness" => "u.harness",
        "project" => "(SELECT slug FROM projects p WHERE p.id = u.project_id)",
        "issue" => "COALESCE('#' || (SELECT number FROM issues i WHERE i.id = u.issue_id) || ' ' || (SELECT title FROM issues i WHERE i.id = u.issue_id), '(no issue)')",
        "day" => "date(u.started_at, 'localtime')",
        other => return Err(ApiError::bad(format!("unknown group_by `{other}`"))),
    };
    let filter = "u.started_at >= ? AND u.started_at < ? AND (? IS NULL OR u.project_id = ?)";
    let order = if group_by == "day" { "key DESC" } else { "total_tokens DESC" };
    let rows = sqlx::query_as::<_, UsageRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {key} AS key, {SUMS} FROM run_usage u WHERE {filter} GROUP BY 1 ORDER BY {order} LIMIT 500"
    )))
    .bind(&from)
    .bind(&to)
    .bind(project_id)
    .bind(project_id)
    .fetch_all(&app.db)
    .await?;
    let totals = sqlx::query_as::<_, UsageRow>(sqlx::AssertSqlSafe(format!("SELECT 'total' AS key, {SUMS} FROM run_usage u WHERE {filter}")))
        .bind(&from)
        .bind(&to)
        .bind(project_id)
        .bind(project_id)
        .fetch_one(&app.db)
        .await?;
    let daily = sqlx::query_as::<_, DailyUsage>(sqlx::AssertSqlSafe(format!(
        "SELECT date(u.started_at, 'localtime') AS day, u.limit_group AS subscription,
                COALESCE(SUM(u.total_tokens),0) AS total_tokens, COALESCE(SUM(u.output_tokens),0) AS output_tokens
           FROM run_usage u WHERE {filter} GROUP BY 1, 2 ORDER BY 1"
    )))
    .bind(&from)
    .bind(&to)
    .bind(project_id)
    .bind(project_id)
    .fetch_all(&app.db)
    .await?;
    Ok(Json(UsageReport { from, to, group_by, totals, rows, daily }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageWindow {
    /// e.g. `five_hour`, `seven_day`, `primary`.
    pub name: String,
    /// 0–100.
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionUsage {
    /// Limit group (e.g. `claude`, `codex`).
    pub name: String,
    pub agents: Vec<String>,
    pub paused: bool,
    pub pause_kind: Option<String>,
    pub paused_until: Option<String>,
    pub plan: Option<String>,
    /// Latest usage-window readings reported by the agent.
    pub windows: Vec<UsageWindow>,
    pub tokens_24h: i64,
    pub tokens_7d: i64,
    pub updated_at: String,
}

fn ts(secs: Option<f64>) -> Option<String> {
    secs.and_then(|s| chrono::DateTime::from_timestamp(s as i64, 0)).map(crate::db::fmt_time)
}

/// Normalise Claude (`unifiedWindows`, `utilization` 0–1) and Codex (`primary`/`secondary`, `used_percent`) snapshots.
fn windows(snap: &Value) -> (Vec<UsageWindow>, Option<String>) {
    let mut out = vec![];
    if let Some(w) = snap.get("unifiedWindows").and_then(Value::as_object) {
        for (name, v) in w {
            out.push(UsageWindow {
                name: name.clone(),
                used_percent: v.get("utilization").and_then(Value::as_f64).map(|u| if u <= 1.0 { u * 100.0 } else { u }),
                resets_at: ts(v.get("resetsAt").and_then(Value::as_f64)),
            });
        }
    } else if let Some(u) = snap.get("utilization").and_then(Value::as_f64) {
        out.push(UsageWindow {
            name: snap.get("rateLimitType").and_then(Value::as_str).unwrap_or("window").into(),
            used_percent: Some(if u <= 1.0 { u * 100.0 } else { u }),
            resets_at: ts(snap.get("resetsAt").and_then(Value::as_f64)),
        });
    }
    for key in ["primary", "secondary"] {
        if let Some(v) = snap.get(key).filter(|v| v.is_object()) {
            let mins = v.get("window_minutes").and_then(Value::as_i64);
            let name = match mins {
                Some(m) if m >= 1440 => format!("{} day", m / 1440),
                Some(m) if m >= 60 => format!("{} hour", m / 60),
                Some(m) => format!("{m} min"),
                None => key.into(),
            };
            out.push(UsageWindow { name, used_percent: v.get("used_percent").and_then(Value::as_f64), resets_at: ts(v.get("resets_at").and_then(Value::as_f64)) });
        }
    }
    (out, snap.get("plan_type").and_then(Value::as_str).map(str::to_string))
}

/// Subscription (limit group) status: pause state, usage windows and recent token totals.
#[utoipa::path(operation_id = "usage_subscriptions", get, path = "/api/usage/subscriptions", tag = "usage", responses((status = 200, body = Vec<SubscriptionUsage>)))]
pub async fn subscriptions(State(app): State<AppState>, _a: Actor) -> ApiResult<Json<Vec<SubscriptionUsage>>> {
    let groups = sqlx::query_as::<_, crate::domain::models::LimitGroup>("SELECT * FROM limit_groups ORDER BY name").fetch_all(&app.db).await?;
    let d1 = crate::db::fmt_time(chrono::Utc::now() - chrono::Duration::days(1));
    let d7 = crate::db::fmt_time(chrono::Utc::now() - chrono::Duration::days(7));
    let mut out = vec![];
    for g in groups {
        let agents: Vec<String> = sqlx::query_scalar("SELECT name FROM agent_definitions WHERE limit_group = ? ORDER BY id").bind(&g.name).fetch_all(&app.db).await?;
        let sum = |since: String| {
            let app = app.clone();
            let name = g.name.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(total_tokens),0) FROM run_usage WHERE limit_group = ? AND started_at >= ?")
                    .bind(name)
                    .bind(since)
                    .fetch_one(&app.db)
                    .await
            }
        };
        let (windows, plan) = g.last_snapshot.as_ref().map(|s| windows(&s.0)).unwrap_or_default();
        out.push(SubscriptionUsage {
            tokens_24h: sum(d1.clone()).await?,
            tokens_7d: sum(d7.clone()).await?,
            name: g.name,
            agents,
            paused: g.paused,
            pause_kind: g.pause_kind,
            paused_until: g.paused_until,
            plan,
            windows,
            updated_at: g.updated_at,
        });
    }
    Ok(Json(out))
}
