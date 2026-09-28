//! Subscription usage-limit detection, pausing of limit groups, and automatic resume.

use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use serde_json::Value;

use crate::AppState;
use crate::acp::AcpError;
use crate::db;

#[derive(Debug, Clone, PartialEq)]
pub enum LimitSignal {
    /// Subscription quota exhausted (5-hour / weekly window, credits...).
    Quota {
        resets_at: Option<DateTime<Utc>>,
        reason: String,
    },
    /// Short-term rate limiting; retry soon.
    Rate {
        retry_at: Option<DateTime<Utc>>,
        reason: String,
    },
    /// The adapter isn't logged in.
    Auth {
        reason: String,
    },
    None,
}

pub struct Evidence<'a> {
    pub error: Option<&'a AcpError>,
    /// Text of the last agent message in the turn.
    pub last_message: &'a str,
    pub stderr_tail: &'a str,
    /// Most recent `_claude/rateLimit` snapshot seen in a `usage_update`.
    pub rate_limit_meta: Option<&'a Value>,
    pub now: DateTime<Utc>,
    pub tz: Tz,
}

const CLAUDE_PREFIXES: &[&str] = &[
    "You've hit your",
    "You’ve hit your",
    "You've reached your",
    "You’ve reached your",
    "You're out of usage credits",
    "You’re out of usage credits",
    "You're out of extra usage",
    "You’re out of extra usage",
    "Your org is out of usage",
    "Claude AI usage limit reached",
];

pub fn local_tz() -> Tz {
    if let Ok(tz) = std::env::var("TZ")
        && let Ok(t) = tz.trim_start_matches(':').parse()
    {
        return t;
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("zoneinfo/")
            && let Ok(t) = s[idx + 9..].parse()
        {
            return t;
        }
    }
    Tz::UTC
}

fn err_data_str<'a>(e: &'a AcpError, key: &str) -> Option<&'a str> {
    let d = e.data.as_ref()?;
    match d.get(key)? {
        Value::String(s) => Some(s.as_str()),
        // codexErrorInfo may be an object like {"usageLimitExceeded": {...}}
        Value::Object(m) => m.keys().next().map(String::as_str),
        _ => None,
    }
}

pub fn classify(ev: &Evidence) -> LimitSignal {
    let mut texts: Vec<&str> = Vec::new();
    if let Some(e) = ev.error {
        texts.push(&e.message);
        if let Some(m) = e.data.as_ref().and_then(|d| d.get("message")).and_then(Value::as_str) {
            texts.push(m);
        }
        if let Some(m) = e.data.as_ref().and_then(|d| d.get("additionalDetails")).and_then(Value::as_str) {
            texts.push(m);
        }
    }
    texts.push(ev.last_message);
    let combined = texts.join("\n");
    let with_stderr = format!("{combined}\n{}", ev.stderr_tail);

    let mut kind: Option<&str> = None;
    if let Some(e) = ev.error {
        if e.code == -32000 && !is_limit_text(&with_stderr) {
            return LimitSignal::Auth { reason: first_line(&e.message) };
        }
        match err_data_str(e, "errorKind") {
            Some("rate_limit") | Some("billing_error") => kind = Some("quota"),
            Some("authentication_failed") => return LimitSignal::Auth { reason: first_line(&e.message) },
            _ => {}
        }
        match err_data_str(e, "codexErrorInfo") {
            Some("usageLimitExceeded") | Some("UsageLimitExceeded") => kind = Some("quota"),
            Some("rateLimitExceeded") | Some("RateLimitExceeded") => kind = kind.or(Some("rate")),
            Some("unauthorized") | Some("Unauthorized") => return LimitSignal::Auth { reason: first_line(&e.message) },
            _ => {}
        }
        if kind.is_none() && is_limit_text(&with_stderr) {
            kind = Some(if is_rate_text(&with_stderr) { "rate" } else { "quota" });
        }
    } else if CLAUDE_PREFIXES.iter().any(|p| ev.last_message.trim_start().starts_with(p)) {
        // The turn "succeeded" but the only output is a usage-limit notice.
        kind = Some("quota");
    }
    let rejected_meta = ev.rate_limit_meta.filter(|m| m.get("status").and_then(Value::as_str) == Some("rejected"));
    if kind.is_none() && rejected_meta.is_some() && ev.error.is_some() {
        kind = Some("quota");
    }
    let Some(kind) = kind else { return LimitSignal::None };

    let reason = texts
        .iter()
        .find(|t| is_limit_text(t))
        .map(|t| first_line(t))
        .unwrap_or_else(|| ev.error.map(|e| first_line(&e.message)).unwrap_or_default());
    let resets_at = rejected_meta
        .and_then(|m| m.get("resetsAt").and_then(Value::as_i64))
        .and_then(|s| Utc.timestamp_opt(s, 0).single())
        .or_else(|| parse_reset_time(&with_stderr, ev.now, ev.tz));
    match kind {
        "rate" => LimitSignal::Rate { retry_at: resets_at, reason },
        _ => LimitSignal::Quota { resets_at, reason },
    }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(300).collect()
}

fn is_limit_text(s: &str) -> bool {
    if CLAUDE_PREFIXES.iter().any(|p| s.contains(p)) {
        return true;
    }
    let re = Regex::new(r"(?i)(usage limit|rate limit|quota (exceeded|exhausted)|limit reached|out of (usage|credits)|too many requests|hit your limit|reached your limit)").unwrap();
    re.is_match(s)
}

fn is_rate_text(s: &str) -> bool {
    Regex::new(r"(?i)(rate limit|too many requests|429)").unwrap().is_match(s)
        && !Regex::new(r"(?i)usage limit|hit your|reached your").unwrap().is_match(s)
}

fn at_local(date: NaiveDate, time: NaiveTime, tz: Tz) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&date.and_time(time)).earliest().map(|d| d.with_timezone(&Utc))
}

fn hm(h: u32, m: u32, ampm: Option<&str>) -> Option<NaiveTime> {
    let mut h = h;
    match ampm.map(|s| s.to_ascii_lowercase()) {
        Some(p) if p.starts_with('p') && h < 12 => h += 12,
        Some(p) if p.starts_with('a') && h == 12 => h = 0,
        _ => {}
    }
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Parse a reset time out of free-form limit messages.
pub fn parse_reset_time(text: &str, now: DateTime<Utc>, default_tz: Tz) -> Option<DateTime<Utc>> {
    // Legacy Claude format: "Claude AI usage limit reached|1759000000"
    if let Some(c) = Regex::new(r"limit reached\|(\d{10})").unwrap().captures(text) {
        return Utc.timestamp_opt(c[1].parse().ok()?, 0).single();
    }
    // "try again in 5 minutes" / "in 2 hours" / "in 3h 20m"
    if let Some(c) = Regex::new(
        r"(?i)(?:try again|retry|resets?) in (\d+)\s*(s|sec|second|m|min|minute|h|hr|hour|d|day)s?\b(?:\s*(\d+)\s*(m|min|minute)s?)?",
    )
    .unwrap()
    .captures(text)
    {
        let n: i64 = c[1].parse().ok()?;
        let secs = match c[2].to_ascii_lowercase().chars().next()? {
            's' => n,
            'm' => n * 60,
            'h' => n * 3600,
            'd' => n * 86400,
            _ => return None,
        };
        let extra: i64 = c.get(3).and_then(|m| m.as_str().parse::<i64>().ok()).unwrap_or(0) * 60;
        return Some(now + chrono::Duration::seconds(secs + extra));
    }
    // "resets Oct 3, 5pm (America/Los_Angeles)" / "resets Oct 3 at 5:30 PM"
    let month_re = Regex::new(
        r"(?i)(?:resets?|try again|available)(?: at| on)? (jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]* (\d{1,2}),?(?: at)? (\d{1,2})(?::(\d{2}))?\s*(am|pm)?(?:\s*\(([^)]+)\))?",
    )
    .unwrap();
    if let Some(c) = month_re.captures(text) {
        let tz: Tz = c.get(6).and_then(|m| m.as_str().trim().parse().ok()).unwrap_or(default_tz);
        let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
            .iter()
            .position(|m| c[1].to_ascii_lowercase().starts_with(m))? as u32
            + 1;
        let day: u32 = c[2].parse().ok()?;
        let t = hm(c[3].parse().ok()?, c.get(4).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0), c.get(5).map(|m| m.as_str()))?;
        let local_now = now.with_timezone(&tz);
        let mut date = NaiveDate::from_ymd_opt(local_now.year(), month, day)?;
        let mut at = at_local(date, t, tz)?;
        if at < now - chrono::Duration::days(1) {
            date = NaiveDate::from_ymd_opt(local_now.year() + 1, month, day)?;
            at = at_local(date, t, tz)?;
        }
        return Some(at);
    }
    // "resets 5pm (America/Los_Angeles)" / "try again at 3:05 PM" / "resets at 17:00"
    let time_re =
        Regex::new(r"(?i)(?:resets?|try again|available again)(?: at)? (\d{1,2})(?::(\d{2}))?\s*(am|pm)?(?:\s*\(([^)]+)\))?").unwrap();
    if let Some(c) = time_re.captures(text) {
        if c.get(2).is_none() && c.get(3).is_none() {
            return None; // A bare number isn't a time.
        }
        let tz: Tz = c.get(4).and_then(|m| m.as_str().trim().parse().ok()).unwrap_or(default_tz);
        let t = hm(c[1].parse().ok()?, c.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0), c.get(3).map(|m| m.as_str()))?;
        let today = now.with_timezone(&tz).date_naive();
        let mut at = at_local(today, t, tz)?;
        if at <= now {
            at = at_local(today.succ_opt()?, t, tz)?;
        }
        return Some(at);
    }
    None
}

/// Pause a limit group in response to a signal. Returns a human-readable summary.
pub async fn apply_signal(app: &AppState, group: &str, agent_id: i64, signal: &LimitSignal) -> anyhow::Result<String> {
    let now = Utc::now();
    let (kind, until, next_probe, reason) = match signal {
        LimitSignal::Quota { resets_at, reason } => (
            "quota",
            resets_at.map(|t| t + chrono::Duration::seconds(60)),
            if resets_at.is_none() { Some(now + chrono::Duration::minutes(15)) } else { None },
            reason.clone(),
        ),
        LimitSignal::Rate { retry_at, reason } => {
            ("rate", Some(retry_at.unwrap_or(now + chrono::Duration::minutes(5))), None, reason.clone())
        }
        LimitSignal::Auth { reason } => {
            sqlx::query("UPDATE agent_definitions SET needs_auth = 1, updated_at = ? WHERE id = ?")
                .bind(db::now())
                .bind(agent_id)
                .execute(&app.db)
                .await?;
            ("auth", None, None, reason.clone())
        }
        LimitSignal::None => return Ok(String::new()),
    };
    sqlx::query(
        "INSERT INTO limit_groups(name, paused, pause_kind, pause_reason, paused_until, next_probe_at, probe_attempts, updated_at)
         VALUES (?, 1, ?, ?, ?, ?, 0, ?)
         ON CONFLICT(name) DO UPDATE SET paused = 1, pause_kind = excluded.pause_kind, pause_reason = excluded.pause_reason,
           paused_until = excluded.paused_until, next_probe_at = excluded.next_probe_at, probe_attempts = 0, updated_at = excluded.updated_at",
    )
    .bind(group)
    .bind(kind)
    .bind(&reason)
    .bind(until.map(db::fmt_time))
    .bind(next_probe.map(db::fmt_time))
    .bind(db::now())
    .execute(&app.db)
    .await?;
    app.bus.emit("limits.updated", None, None, None, None);
    let when = match (until, next_probe) {
        (Some(u), _) => format!("until {}", u.with_timezone(&local_tz()).format("%a %H:%M %Z")),
        (None, Some(_)) => "until a probe succeeds".into(),
        _ => "until resumed manually".into(),
    };
    Ok(format!("`{group}` paused ({kind}) {when}: {reason}"))
}

pub async fn resume(app: &AppState, group: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE limit_groups SET paused = 0, pause_kind = NULL, pause_reason = NULL, paused_until = NULL, next_probe_at = NULL,
                probe_attempts = 0, updated_at = ? WHERE name = ?",
    )
    .bind(db::now())
    .bind(group)
    .execute(&app.db)
    .await?;
    sqlx::query("UPDATE agent_definitions SET needs_auth = 0 WHERE limit_group = ?").bind(group).execute(&app.db).await?;
    app.bus.emit("limits.updated", None, None, None, None);
    Ok(())
}

pub async fn pause_manual(app: &AppState, group: &str, until: Option<DateTime<Utc>>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO limit_groups(name, paused, pause_kind, pause_reason, paused_until, updated_at) VALUES (?, 1, 'manual', 'paused by a human', ?, ?)
         ON CONFLICT(name) DO UPDATE SET paused = 1, pause_kind = 'manual', pause_reason = 'paused by a human', paused_until = excluded.paused_until,
           next_probe_at = NULL, updated_at = excluded.updated_at",
    )
    .bind(group)
    .bind(until.map(db::fmt_time))
    .bind(db::now())
    .execute(&app.db)
    .await?;
    app.bus.emit("limits.updated", None, None, None, None);
    Ok(())
}

/// Store the latest usage snapshot for display.
pub async fn record_snapshot(app: &AppState, group: &str, snapshot: &Value) {
    let _ = sqlx::query("UPDATE limit_groups SET last_snapshot = ?, updated_at = ? WHERE name = ?")
        .bind(snapshot.to_string())
        .bind(db::now())
        .bind(group)
        .execute(&app.db)
        .await;
    app.bus.emit("limits.updated", None, None, None, None);
}

/// Background task: resume groups whose reset time has passed; probe groups with unknown reset times.
pub fn spawn(app: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&app).await {
                tracing::warn!("limits tick failed: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

async fn tick(app: &AppState) -> anyhow::Result<()> {
    let now = db::now();
    let expired: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM limit_groups WHERE paused = 1 AND paused_until IS NOT NULL AND paused_until <= ? AND pause_kind IN ('quota','rate','manual')",
    )
    .bind(&now)
    .fetch_all(&app.db)
    .await?;
    for g in expired {
        tracing::info!("limit group {g}: reset time passed, resuming");
        resume(app, &g).await?;
    }
    let probe: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, probe_attempts FROM limit_groups WHERE paused = 1 AND paused_until IS NULL AND next_probe_at IS NOT NULL
           AND next_probe_at <= ? AND pause_kind IN ('quota','rate')",
    )
    .bind(&now)
    .fetch_all(&app.db)
    .await?;
    for (g, attempts) in probe {
        // Push the next probe out first so a slow probe doesn't get started twice.
        let backoff = (15i64 * 2i64.pow(attempts.min(4) as u32)).min(120);
        sqlx::query("UPDATE limit_groups SET probe_attempts = probe_attempts + 1, next_probe_at = ? WHERE name = ?")
            .bind(db::fmt_time(Utc::now() + chrono::Duration::minutes(backoff)))
            .bind(&g)
            .execute(&app.db)
            .await?;
        let app = app.clone();
        tokio::spawn(async move {
            match super::probe::probe_group(&app, &g).await {
                Ok(true) => {
                    tracing::info!("limit group {g}: probe succeeded, resuming");
                    let _ = resume(&app, &g).await;
                }
                Ok(false) => tracing::info!("limit group {g}: still limited"),
                Err(e) => tracing::warn!("limit group {g}: probe failed: {e:#}"),
            }
            app.bus.emit("limits.updated", None, None, None, None);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        // 2026-09-27 20:00 UTC = 13:00 in Los Angeles.
        Utc.with_ymd_and_hms(2026, 9, 27, 20, 0, 0).unwrap()
    }

    fn ev<'a>(error: Option<&'a AcpError>, msg: &'a str, meta: Option<&'a Value>) -> Evidence<'a> {
        Evidence { error, last_message: msg, stderr_tail: "", rate_limit_meta: meta, now: now(), tz: chrono_tz::America::Los_Angeles }
    }

    #[test]
    fn claude_quota_with_tz() {
        let e = AcpError {
            code: -32603,
            message: "You've hit your limit · resets 5pm (America/Los_Angeles)".into(),
            data: Some(json!({"errorKind": "rate_limit"})),
        };
        match classify(&ev(Some(&e), "", None)) {
            LimitSignal::Quota { resets_at: Some(t), .. } => assert_eq!(t, Utc.with_ymd_and_hms(2026, 9, 28, 0, 0, 0).unwrap()),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn claude_meta_reset() {
        let e = AcpError { code: -32603, message: "Internal error".into(), data: Some(json!({"errorKind": "rate_limit"})) };
        let meta = json!({"status": "rejected", "resetsAt": 1790000000, "rateLimitType": "five_hour"});
        match classify(&ev(Some(&e), "", Some(&meta))) {
            LimitSignal::Quota { resets_at: Some(t), .. } => assert_eq!(t.timestamp(), 1790000000),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn codex_usage_limit() {
        let e = AcpError {
            code: -32603,
            message: "You've hit your usage limit. Upgrade to Pro or try again at 3:05 PM.".into(),
            data: Some(json!({"codexErrorInfo": "usageLimitExceeded"})),
        };
        match classify(&ev(Some(&e), "", None)) {
            LimitSignal::Quota { resets_at: Some(t), .. } => {
                assert_eq!(t, chrono_tz::America::Los_Angeles.with_ymd_and_hms(2026, 9, 27, 15, 5, 0).unwrap().with_timezone(&Utc))
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn codex_rate_limit_and_relative() {
        let e = AcpError {
            code: -32603,
            message: "Rate limit reached, try again in 20 seconds".into(),
            data: Some(json!({"codexErrorInfo": "rateLimitExceeded"})),
        };
        match classify(&ev(Some(&e), "", None)) {
            LimitSignal::Rate { retry_at: Some(t), .. } => assert_eq!(t, now() + chrono::Duration::seconds(20)),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn auth_and_ordinary_errors() {
        let e = AcpError { code: -32000, message: "Authentication required".into(), data: None };
        assert!(matches!(classify(&ev(Some(&e), "", None)), LimitSignal::Auth { .. }));
        let e = AcpError { code: -32603, message: "Internal error: tool crashed".into(), data: None };
        assert_eq!(classify(&ev(Some(&e), "", None)), LimitSignal::None);
        assert_eq!(classify(&ev(None, "All done, tests pass.", None)), LimitSignal::None);
    }

    #[test]
    fn success_turn_with_limit_message() {
        match classify(&ev(None, "You've hit your limit · resets Oct 3, 9am", None)) {
            LimitSignal::Quota { resets_at: Some(t), .. } => {
                assert_eq!(t, chrono_tz::America::Los_Angeles.with_ymd_and_hms(2026, 10, 3, 9, 0, 0).unwrap().with_timezone(&Utc))
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn legacy_pipe_format() {
        assert_eq!(parse_reset_time("Claude AI usage limit reached|1790000000", now(), Tz::UTC).unwrap().timestamp(), 1790000000);
    }
}
