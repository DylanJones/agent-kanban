//! Idempotent import of a GitHub repo + Projects (v2) board into the local database.
//!
//! Keyed by GitHub node IDs so it can be re-run. Local edits win: an issue modified locally
//! since its last sync keeps its local title/body/state.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::AppState;
use crate::db::{self, begin_write};
use crate::domain::models::Project;
use crate::domain::{Hold, IssueState};
use crate::git;

const PROJECT_QUERY: &str = r#"
query($owner:String!, $number:Int!, $endCursor:String) {
  repositoryOwner(login:$owner) { ... on ProjectV2Owner { projectV2(number:$number) {
    id title
    fields(first:50) { nodes { ... on ProjectV2SingleSelectField { id name options { id name } } } }
    items(first:100, after:$endCursor) { pageInfo { hasNextPage endCursor } nodes {
      id
      content { __typename ... on Issue { id number } ... on PullRequest { id number } }
      fieldValues(first:30) { nodes {
        ... on ProjectV2ItemFieldSingleSelectValue { name field { ... on ProjectV2FieldCommon { name } } }
        ... on ProjectV2ItemFieldNumberValue { number field { ... on ProjectV2FieldCommon { name } } }
        ... on ProjectV2ItemFieldDateValue { date field { ... on ProjectV2FieldCommon { name } } }
      } }
    } }
  } } }
}"#;

const LABELS_QUERY: &str = r#"
query($owner:String!, $name:String!, $endCursor:String) { repository(owner:$owner, name:$name) {
  labels(first:100, after:$endCursor) { pageInfo { hasNextPage endCursor } nodes { id name color description } }
} }"#;

const ISSUES_QUERY: &str = r#"
query($owner:String!, $name:String!, $endCursor:String) { repository(owner:$owner, name:$name) {
  issues(first:40, after:$endCursor, orderBy:{field:CREATED_AT, direction:ASC}) { pageInfo { hasNextPage endCursor } nodes {
    id number title body state stateReason createdAt updatedAt closedAt url
    author { login }
    labels(first:30) { nodes { name } }
    parent { number }
    comments(first:100) { nodes { id body createdAt updatedAt author { login } } }
  } }
} }"#;

const PRS_QUERY: &str = r#"
query($owner:String!, $name:String!, $endCursor:String) { repository(owner:$owner, name:$name) {
  pullRequests(first:20, after:$endCursor, orderBy:{field:CREATED_AT, direction:ASC}) { pageInfo { hasNextPage endCursor } nodes {
    id number title body state merged mergedAt createdAt updatedAt closedAt url
    headRefName baseRefName headRefOid
    author { login }
    mergeCommit { oid }
    closingIssuesReferences(first:10) { nodes { number } }
    comments(first:100) { nodes { id body createdAt updatedAt author { login } } }
    reviews(first:50) { nodes { id state body submittedAt author { login } commit { oid } } }
    reviewThreads(first:100) { nodes {
      id isResolved isOutdated path line originalLine startLine diffSide
      comments(first:50) { nodes { id body createdAt updatedAt author { login } commit { oid } originalCommit { oid } diffHunk } }
    } }
  } }
} }"#;

#[derive(Debug, Default, Clone, serde::Deserialize, Serialize)]
pub struct ImportData {
    pub project: Option<Value>,
    pub items: Vec<Value>,
    pub labels: Vec<Value>,
    pub issues: Vec<Value>,
    pub prs: Vec<Value>,
}

#[derive(Debug, Default, Clone, Serialize, ToSchema)]
pub struct ImportStats {
    pub board_items: usize,
    pub labels: usize,
    pub issues_created: usize,
    pub issues_updated: usize,
    pub issues_skipped_local_edits: usize,
    pub prs_created: usize,
    pub prs_updated: usize,
    pub stub_issues: usize,
    pub comments: usize,
    pub reviews: usize,
    pub threads: usize,
    pub branches_fetched: usize,
}

fn nodes<'a>(v: &'a Value, path: &[&str]) -> Vec<&'a Value> {
    let mut cur = v;
    for p in path {
        match cur.get(p) {
            Some(n) => cur = n,
            None => return vec![],
        }
    }
    cur.get("nodes").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn login(v: &Value) -> String {
    v.get("author").and_then(|a| a.get("login")).and_then(Value::as_str).unwrap_or("ghost").to_string()
}

pub async fn fetch(
    repo: &str,
    project_owner: Option<&str>,
    project_number: Option<i64>,
    log: &mut (dyn FnMut(String) + Send),
) -> anyhow::Result<ImportData> {
    let (owner, name) = super::split_repo(repo)?;
    let rv = |extra: Vec<(&'static str, String)>| {
        let mut v = vec![("owner", owner.to_string()), ("name", name.to_string())];
        v.extend(extra);
        v
    };
    let mut data = ImportData::default();
    if let (Some(po), Some(pn)) = (project_owner, project_number) {
        log(format!("fetching project {po}#{pn}"));
        let pages = super::graphql(PROJECT_QUERY, &[("owner", po.to_string()), ("number", pn.to_string())], true).await?;
        for p in &pages {
            let proj = &p["data"]["repositoryOwner"]["projectV2"];
            if proj.is_null() {
                anyhow::bail!("project {po}#{pn} not found (does gh have the `project` scope? run `gh auth refresh -s project`)");
            }
            if data.project.is_none() {
                data.project = Some(proj.clone());
            }
            data.items.extend(nodes(proj, &["items"]).into_iter().cloned());
        }
    }
    log(format!("fetching labels of {repo}"));
    for p in super::graphql(LABELS_QUERY, &rv(vec![]), true).await? {
        data.labels.extend(nodes(&p["data"]["repository"], &["labels"]).into_iter().cloned());
    }
    log("fetching issues".into());
    for p in super::graphql(ISSUES_QUERY, &rv(vec![]), true).await? {
        data.issues.extend(nodes(&p["data"]["repository"], &["issues"]).into_iter().cloned());
    }
    log("fetching pull requests".into());
    for p in super::graphql(PRS_QUERY, &rv(vec![]), true).await? {
        data.prs.extend(nodes(&p["data"]["repository"], &["pullRequests"]).into_iter().cloned());
    }
    log(format!(
        "fetched {} board items, {} labels, {} issues, {} PRs",
        data.items.len(),
        data.labels.len(),
        data.issues.len(),
        data.prs.len()
    ));
    Ok(data)
}

#[derive(Debug, Default, Clone)]
struct BoardItem {
    item_id: String,
    status: Option<String>,
    priority: Option<String>,
    size: Option<String>,
    estimate: Option<f64>,
    start: Option<String>,
    target: Option<String>,
}

fn board_items(data: &ImportData) -> HashMap<(String, i64), BoardItem> {
    let mut m = HashMap::new();
    for it in &data.items {
        let c = &it["content"];
        let (Some(kind), Some(num)) = (s(c, "__typename"), c.get("number").and_then(Value::as_i64)) else { continue };
        let mut b = BoardItem { item_id: s(it, "id").unwrap_or_default().to_string(), ..Default::default() };
        for fv in nodes(it, &["fieldValues"]) {
            let Some(field) = fv.get("field").and_then(|f| f.get("name")).and_then(Value::as_str) else { continue };
            match field {
                "Status" => b.status = s(fv, "name").map(str::to_string),
                "Priority" => b.priority = s(fv, "name").map(str::to_string),
                "Size" => b.size = s(fv, "name").map(str::to_string),
                "Estimate" => b.estimate = fv.get("number").and_then(Value::as_f64),
                "Start date" => b.start = s(fv, "date").map(str::to_string),
                "Target date" => b.target = s(fv, "date").map(str::to_string),
                _ => {}
            }
        }
        m.insert((kind.to_string(), num), b);
    }
    m
}

/// GitHub board status → local state + hold.
pub fn map_status(
    status: Option<&str>,
    open: bool,
    state_reason: Option<&str>,
    has_open_pr: bool,
) -> (IssueState, Option<Hold>, Option<String>) {
    if !open {
        return match state_reason {
            Some("NOT_PLANNED") => (IssueState::Closed, None, Some("not_planned".into())),
            Some("DUPLICATE") => (IssueState::Closed, None, Some("duplicate".into())),
            _ => (IssueState::Done, None, None),
        };
    }
    match status {
        Some("Ready") => (IssueState::Ready, None, None),
        Some("In progress") => (IssueState::InProgress, None, None),
        Some("In review") => (IssueState::InReview, None, None),
        Some("Changes requested") => (IssueState::ChangesRequested, None, None),
        Some("Needs decision") => (if has_open_pr { IssueState::InReview } else { IssueState::Backlog }, Some(Hold::NeedsDecision), None),
        Some("Ready to merge") => (IssueState::ReadyToMerge, None, None),
        Some("Done") => (IssueState::Done, None, None),
        _ => (IssueState::Backlog, None, None),
    }
}

/// Local state → GitHub board status name (used by the status mirror).
pub fn status_name(state: IssueState, hold: Option<Hold>) -> &'static str {
    if hold == Some(Hold::NeedsDecision) {
        return "Needs decision";
    }
    match state {
        IssueState::Triage | IssueState::Backlog => "Backlog",
        IssueState::Ready => "Ready",
        IssueState::InProgress | IssueState::MergeConflict => "In progress",
        IssueState::ChangesRequested => "Changes requested",
        IssueState::InReview => "In review",
        IssueState::ReadyToMerge => "Ready to merge",
        IssueState::Done | IssueState::Closed => "Done",
    }
}

fn valid_priority(p: Option<String>) -> Option<String> {
    p.filter(|p| matches!(p.as_str(), "P0" | "P1" | "P2"))
}
fn valid_size(p: Option<String>) -> Option<String> {
    p.filter(|p| matches!(p.as_str(), "XS" | "S" | "M" | "L" | "XL"))
}

pub async fn apply(
    app: &AppState,
    project: &Project,
    data: &ImportData,
    log: &mut (dyn FnMut(String) + Send),
) -> anyhow::Result<ImportStats> {
    let mut st = ImportStats { board_items: data.items.len(), ..Default::default() };
    let now = db::now();
    let board = board_items(data);
    let mut tx = begin_write(&app.db).await?;

    // Project-level GitHub ids (for the status mirror).
    if let Some(p) = &data.project {
        let mut options = serde_json::Map::new();
        let mut field_id = None;
        for f in nodes(p, &["fields"]) {
            if s(f, "name") == Some("Status") {
                field_id = s(f, "id").map(str::to_string);
                for o in f.get("options").and_then(Value::as_array).into_iter().flatten() {
                    if let (Some(n), Some(i)) = (s(o, "name"), s(o, "id")) {
                        options.insert(n.to_string(), Value::String(i.to_string()));
                    }
                }
            }
        }
        sqlx::query(
            "UPDATE projects SET github_project_id = ?, github_status_field_id = ?, github_status_options = ?, updated_at = ? WHERE id = ?",
        )
        .bind(s(p, "id"))
        .bind(field_id)
        .bind(Value::Object(options).to_string())
        .bind(&now)
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    }

    for l in &data.labels {
        let Some(name) = s(l, "name") else { continue };
        sqlx::query(
            "INSERT INTO labels(project_id, name, color, description, github_node_id) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(project_id, name) DO UPDATE SET color = excluded.color, description = excluded.description,
                                                         github_node_id = excluded.github_node_id",
        )
        .bind(project.id)
        .bind(name)
        .bind(s(l, "color").unwrap_or("888888"))
        .bind(s(l, "description"))
        .bind(s(l, "id"))
        .execute(&mut *tx)
        .await?;
        st.labels += 1;
    }

    // Which issues have an open PR that closes them.
    let mut open_pr_issues: HashSet<i64> = HashSet::new();
    let mut max_number = 0i64;
    for pr in &data.prs {
        max_number = max_number.max(pr["number"].as_i64().unwrap_or(0));
        if s(pr, "state") == Some("OPEN") {
            for n in nodes(pr, &["closingIssuesReferences"]) {
                if let Some(n) = n["number"].as_i64() {
                    open_pr_issues.insert(n);
                }
            }
        }
    }
    for i in &data.issues {
        max_number = max_number.max(i["number"].as_i64().unwrap_or(0));
    }
    let cur_next: i64 = sqlx::query_scalar("SELECT next_number FROM projects WHERE id = ?").bind(project.id).fetch_one(&mut *tx).await?;
    let mut next_number = cur_next.max(max_number + 1);

    // Issues.
    let mut parent_links = Vec::new();
    for i in &data.issues {
        let Some(node) = s(i, "id") else { continue };
        let number = i["number"].as_i64().unwrap_or(0);
        let b = board.get(&("Issue".to_string(), number)).cloned().unwrap_or_default();
        let open = s(i, "state") == Some("OPEN");
        let (state, hold, close_reason) = map_status(b.status.as_deref(), open, s(i, "stateReason"), open_pr_issues.contains(&number));
        let labels: Vec<String> = nodes(i, &["labels"]).iter().filter_map(|l| s(l, "name").map(str::to_string)).collect();
        let existing: Option<(i64, String, Option<String>)> =
            sqlx::query_as("SELECT id, updated_at, gh_synced_at FROM issues WHERE github_node_id = ?")
                .bind(node)
                .fetch_optional(&mut *tx)
                .await?;
        let issue_id = match existing {
            None => {
                let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM issues WHERE project_id = ? AND number = ?)")
                    .bind(project.id)
                    .bind(number)
                    .fetch_one(&mut *tx)
                    .await?;
                let local_number = if taken {
                    next_number += 1;
                    next_number - 1
                } else {
                    number
                };
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO issues(project_id, number, title, body, state, hold, hold_reason, priority, size, estimate, start_date,
                        target_date, rank, source, author_name, close_reason, github_node_id, github_number, github_project_item_id,
                        gh_synced_at, created_at, updated_at, closed_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'github', ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(project.id)
                .bind(local_number)
                .bind(s(i, "title").unwrap_or("(untitled)"))
                .bind(s(i, "body").unwrap_or(""))
                .bind(state)
                .bind(hold)
                .bind(hold.map(|_| "imported from GitHub board: Needs decision"))
                .bind(valid_priority(b.priority.clone()))
                .bind(valid_size(b.size.clone()))
                .bind(b.estimate)
                .bind(&b.start)
                .bind(&b.target)
                .bind(number as f64)
                .bind(login(i))
                .bind(&close_reason)
                .bind(node)
                .bind(number)
                .bind((!b.item_id.is_empty()).then_some(&b.item_id))
                .bind(&now)
                .bind(s(i, "createdAt").unwrap_or(&now))
                .bind(&now)
                .bind(s(i, "closedAt"))
                .fetch_one(&mut *tx)
                .await?;
                st.issues_created += 1;
                id
            }
            Some((id, updated, synced)) => {
                if synced.as_deref().is_some_and(|sy| updated.as_str() <= sy) {
                    sqlx::query(
                        "UPDATE issues SET title = ?, body = ?, state = ?, hold = ?, priority = ?, size = ?, estimate = ?,
                            start_date = ?, target_date = ?, close_reason = ?, github_project_item_id = ?, closed_at = ?,
                            gh_synced_at = ?, updated_at = ? WHERE id = ?",
                    )
                    .bind(s(i, "title").unwrap_or("(untitled)"))
                    .bind(s(i, "body").unwrap_or(""))
                    .bind(state)
                    .bind(hold)
                    .bind(valid_priority(b.priority.clone()))
                    .bind(valid_size(b.size.clone()))
                    .bind(b.estimate)
                    .bind(&b.start)
                    .bind(&b.target)
                    .bind(&close_reason)
                    .bind((!b.item_id.is_empty()).then_some(&b.item_id))
                    .bind(s(i, "closedAt"))
                    .bind(&now)
                    .bind(&now)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    st.issues_updated += 1;
                } else {
                    st.issues_skipped_local_edits += 1;
                }
                id
            }
        };
        crate::services::labels::set_issue_labels(&mut tx, project.id, issue_id, &labels).await?;
        if let Some(p) = i.get("parent").and_then(|p| p.get("number")).and_then(Value::as_i64) {
            parent_links.push((issue_id, p));
        }
        for c in nodes(i, &["comments"]) {
            st.comments += insert_comment(&mut tx, project.id, ("issue_id", issue_id), c).await?;
        }
    }
    for (id, parent) in parent_links {
        sqlx::query("UPDATE issues SET parent_issue_id = (SELECT id FROM issues WHERE project_id = ? AND github_number = ?) WHERE id = ?")
            .bind(project.id)
            .bind(parent)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    // Pull requests.
    for pr in &data.prs {
        let Some(node) = s(pr, "id") else { continue };
        let number = pr["number"].as_i64().unwrap_or(0);
        let gh_state = s(pr, "state").unwrap_or("CLOSED");
        let state = match (gh_state, pr["merged"].as_bool().unwrap_or(false)) {
            (_, true) => "merged",
            ("OPEN", _) => "open",
            _ => "closed",
        };
        let branch = s(pr, "headRefName").unwrap_or_default().to_string();
        let base = s(pr, "baseRefName").unwrap_or(&project.base_branch).to_string();
        let remote_head = s(pr, "headRefOid").map(str::to_string);
        let head =
            if state == "open" { git::branch_sha(&project.repo_path, &branch).await.or(remote_head.clone()) } else { remote_head.clone() };
        let merge_base = match &head {
            Some(h) if state == "open" => git::merge_base(&project.repo_path, &base, h).await,
            _ => None,
        };
        let merged_sha = pr.get("mergeCommit").and_then(|m| m.get("oid")).and_then(Value::as_str);
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM pull_requests WHERE github_node_id = ?").bind(node).fetch_optional(&mut *tx).await?;
        let pr_id = match existing {
            None => {
                let taken: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pull_requests WHERE project_id = ? AND number = ?)
                        OR EXISTS(SELECT 1 FROM issues WHERE project_id = ? AND number = ? AND github_number IS NOT ?)",
                )
                .bind(project.id)
                .bind(number)
                .bind(project.id)
                .bind(number)
                .bind(number)
                .fetch_one(&mut *tx)
                .await?;
                let local_number = if taken {
                    next_number += 1;
                    next_number - 1
                } else {
                    number
                };
                st.prs_created += 1;
                sqlx::query_scalar(
                    "INSERT INTO pull_requests(project_id, number, title, body, state, branch, base_branch, head_sha, merge_base_sha,
                        merged_sha, merged_at, author_kind, author_name, github_node_id, github_number, github_url, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'github', ?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(project.id)
                .bind(local_number)
                .bind(s(pr, "title").unwrap_or_default())
                .bind(s(pr, "body").unwrap_or_default())
                .bind(state)
                .bind(&branch)
                .bind(&base)
                .bind(&head)
                .bind(&merge_base)
                .bind(merged_sha)
                .bind(s(pr, "mergedAt"))
                .bind(login(pr))
                .bind(node)
                .bind(number)
                .bind(s(pr, "url"))
                .bind(s(pr, "createdAt").unwrap_or(&now))
                .bind(&now)
                .fetch_one(&mut *tx)
                .await?
            }
            Some(id) => {
                st.prs_updated += 1;
                // Keep local state if it was merged/closed locally.
                sqlx::query(
                    "UPDATE pull_requests SET title = ?, body = ?, state = CASE WHEN state = 'open' THEN ? ELSE state END,
                        merged_sha = COALESCE(merged_sha, ?), merged_at = COALESCE(merged_at, ?), github_url = ?,
                        head_sha = CASE WHEN state = 'open' THEN COALESCE(?, head_sha) ELSE head_sha END
                     WHERE id = ?",
                )
                .bind(s(pr, "title").unwrap_or_default())
                .bind(s(pr, "body").unwrap_or_default())
                .bind(state)
                .bind(merged_sha)
                .bind(s(pr, "mergedAt"))
                .bind(s(pr, "url"))
                .bind(&head)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                id
            }
        };

        // Link issues this PR closes.
        let mut linked = 0;
        for n in nodes(pr, &["closingIssuesReferences"]) {
            if let Some(n) = n["number"].as_i64() {
                let r = sqlx::query(
                    "INSERT OR IGNORE INTO pull_request_issues(pr_id, issue_id)
                     SELECT ?, id FROM issues WHERE project_id = ? AND github_number = ?",
                )
                .bind(pr_id)
                .bind(project.id)
                .bind(n)
                .execute(&mut *tx)
                .await?;
                linked += r.rows_affected();
                if state == "open" {
                    sqlx::query(
                        "UPDATE issues SET branch_name = ? WHERE project_id = ? AND github_number = ? AND state NOT IN ('done','closed')",
                    )
                    .bind(&branch)
                    .bind(project.id)
                    .bind(n)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        let already_linked: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pull_request_issues WHERE pr_id = ?)").bind(pr_id).fetch_one(&mut *tx).await?;

        // PR-only board cards (or open PRs with no issue) get a tracking issue so they appear on the board.
        let board_item = board.get(&("PullRequest".to_string(), number)).cloned();
        if linked == 0 && !already_linked && (board_item.is_some() || state == "open") {
            let b = board_item.unwrap_or_default();
            let status = b.status.clone().or_else(|| (state == "open").then(|| "In review".to_string()));
            let (istate, hold, reason) =
                map_status(status.as_deref(), state == "open" || status.as_deref() != Some("Done"), None, state == "open");
            let istate = if state == "merged" { IssueState::Done } else { istate };
            let stub_node = format!("stub:{node}");
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT id FROM issues WHERE github_node_id = ?").bind(&stub_node).fetch_optional(&mut *tx).await?;
            let iid = match exists {
                Some(id) => id,
                None => {
                    next_number += 1;
                    st.stub_issues += 1;
                    sqlx::query_scalar(
                        "INSERT INTO issues(project_id, number, title, body, state, hold, priority, size, rank, source, author_name,
                            close_reason, branch_name, github_node_id, github_project_item_id, gh_synced_at, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'github', ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                    )
                    .bind(project.id)
                    .bind(next_number - 1)
                    .bind(s(pr, "title").unwrap_or_default())
                    .bind(format!("Tracking issue for GitHub PR #{number}, which was on the board without a linked issue."))
                    .bind(istate)
                    .bind(hold)
                    .bind(valid_priority(b.priority.clone()))
                    .bind(valid_size(b.size.clone()))
                    .bind(number as f64)
                    .bind(login(pr))
                    .bind(reason)
                    .bind((state == "open").then_some(&branch))
                    .bind(&stub_node)
                    .bind((!b.item_id.is_empty()).then_some(&b.item_id))
                    .bind(&now)
                    .bind(s(pr, "createdAt").unwrap_or(&now))
                    .bind(&now)
                    .fetch_one(&mut *tx)
                    .await?
                }
            };
            crate::services::labels::add_issue_labels(&mut tx, project.id, iid, &["pr-only".to_string()]).await?;
            sqlx::query("INSERT OR IGNORE INTO pull_request_issues(pr_id, issue_id) VALUES (?, ?)")
                .bind(pr_id)
                .bind(iid)
                .execute(&mut *tx)
                .await?;
        }

        for c in nodes(pr, &["comments"]) {
            st.comments += insert_comment(&mut tx, project.id, ("pr_id", pr_id), c).await?;
        }
        for r in nodes(pr, &["reviews"]) {
            let verdict = match s(r, "state") {
                Some("APPROVED") => "approve",
                Some("CHANGES_REQUESTED") => "changes_requested",
                Some("COMMENTED") if !s(r, "body").unwrap_or("").trim().is_empty() => "comment",
                _ => continue,
            };
            let res = sqlx::query(
                "INSERT OR IGNORE INTO reviews(pr_id, verdict, body, commit_sha, author_kind, author_name, github_node_id, created_at)
                 VALUES (?, ?, ?, ?, 'github', ?, ?, ?)",
            )
            .bind(pr_id)
            .bind(verdict)
            .bind(s(r, "body").unwrap_or(""))
            .bind(r.get("commit").and_then(|c| c.get("oid")).and_then(Value::as_str))
            .bind(login(r))
            .bind(s(r, "id"))
            .bind(s(r, "submittedAt").unwrap_or(&now))
            .execute(&mut *tx)
            .await?;
            st.reviews += res.rows_affected() as usize;
        }
        for t in nodes(pr, &["reviewThreads"]) {
            let Some(tnode) = s(t, "id") else { continue };
            let cs = nodes(t, &["comments"]);
            let first = cs.first().copied();
            let commit = first.and_then(|c| c.get("commit")).and_then(|c| c.get("oid")).and_then(Value::as_str);
            let orig_commit = first.and_then(|c| c.get("originalCommit")).and_then(|c| c.get("oid")).and_then(Value::as_str);
            let line = t["line"].as_i64();
            let outdated = t["isOutdated"].as_bool().unwrap_or(false) || line.is_none();
            let resolved = t["isResolved"].as_bool().unwrap_or(false);
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM review_threads WHERE github_node_id = ?").bind(tnode).fetch_optional(&mut *tx).await?;
            let tid = match existing {
                Some(id) => {
                    sqlx::query("UPDATE review_threads SET resolved = ?, outdated = MAX(outdated, ?) WHERE id = ?")
                        .bind(resolved)
                        .bind(outdated)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    id
                }
                None => {
                    st.threads += 1;
                    sqlx::query_scalar(
                        "INSERT INTO review_threads(pr_id, path, line, start_line, side, commit_sha, original_line, original_commit_sha,
                            diff_hunk, severity, resolved, outdated, github_node_id, created_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'blocking', ?, ?, ?, ?) RETURNING id",
                    )
                    .bind(pr_id)
                    .bind(s(t, "path").unwrap_or_default())
                    .bind(line.or(t["originalLine"].as_i64()))
                    .bind(t["startLine"].as_i64())
                    .bind(s(t, "diffSide").unwrap_or("RIGHT"))
                    .bind(commit.or(orig_commit))
                    .bind(t["originalLine"].as_i64())
                    .bind(orig_commit)
                    .bind(first.and_then(|c| s(c, "diffHunk")))
                    .bind(resolved)
                    .bind(outdated)
                    .bind(tnode)
                    .bind(first.and_then(|c| s(c, "createdAt")).unwrap_or(&now))
                    .fetch_one(&mut *tx)
                    .await?
                }
            };
            for c in cs {
                st.comments += insert_comment(&mut tx, project.id, ("thread_id", tid), c).await?;
            }
        }
    }

    // Review handoff SHAs for imported open PRs, from the board state of their issues.
    sqlx::query(
        "UPDATE pull_requests SET approved_sha = head_sha, last_reviewed_sha = COALESCE(last_reviewed_sha, head_sha)
          WHERE project_id = ? AND state = 'open' AND approved_sha IS NULL AND id IN (
            SELECT pi.pr_id FROM pull_request_issues pi JOIN issues i ON i.id = pi.issue_id WHERE i.state = 'ready_to_merge')",
    )
    .bind(project.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE pull_requests SET review_requested_sha = head_sha
          WHERE project_id = ? AND state = 'open' AND review_requested_sha IS NULL AND id IN (
            SELECT pi.pr_id FROM pull_request_issues pi JOIN issues i ON i.id = pi.issue_id WHERE i.state IN ('in_review','ready_to_merge'))",
    )
    .bind(project.id)
    .execute(&mut *tx)
    .await?;

    let max_local: i64 = sqlx::query_scalar(
        "SELECT MAX(n) FROM (SELECT MAX(number) AS n FROM issues WHERE project_id = ? UNION ALL SELECT MAX(number) FROM pull_requests WHERE project_id = ?)",
    )
    .bind(project.id)
    .bind(project.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(0);
    sqlx::query("UPDATE projects SET next_number = MAX(next_number, ?) WHERE id = ?")
        .bind(max_local + 1)
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    log(format!("{st:?}"));
    Ok(st)
}

async fn insert_comment(tx: &mut crate::db::Tx, project_id: i64, target: (&str, i64), c: &Value) -> anyhow::Result<usize> {
    let Some(node) = s(c, "id") else { return Ok(0) };
    let sql = format!(
        "INSERT OR IGNORE INTO comments(project_id, {}, kind, body, author_kind, author_name, github_node_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'github', ?, ?, ?, ?)",
        target.0
    );
    let body = s(c, "body").unwrap_or("");
    let kind = if body.contains("Review verdict:") { "review_summary" } else { "comment" };
    let r = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(project_id)
        .bind(target.1)
        .bind(kind)
        .bind(body)
        .bind(login(c))
        .bind(node)
        .bind(s(c, "createdAt").unwrap_or_default())
        .bind(s(c, "updatedAt").or(s(c, "createdAt")).unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    Ok(r.rows_affected() as usize)
}

/// Make sure branches of open imported PRs exist locally (fetching from `origin`), fast-forwarding
/// local copies that are strictly behind.
pub async fn sync_open_pr_branches(app: &AppState, project: &Project, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<usize> {
    let repo = &project.repo_path;
    if git::run(repo, &["remote", "get-url", "origin"]).await.is_err() {
        log("no `origin` remote; skipping branch fetch".into());
        return Ok(0);
    }
    // Bring the base branch up to date (fast-forward only) so PR diffs and conflict checks are accurate.
    let base = project.base_branch.clone();
    let spec = format!("+refs/heads/{base}:refs/remotes/origin/{base}");
    if git::run(repo, &["fetch", "--quiet", "origin", &spec]).await.is_ok()
        && let (Some(local), Some(remote)) =
            (git::branch_sha(repo, &base).await, git::rev_parse(repo, &format!("refs/remotes/origin/{base}")).await)
        && local != remote
        && git::is_ancestor(repo, &local, &remote).await
    {
        match git::worktree::checked_out_at(repo, &base).await? {
            None => {
                git::run(repo, &["update-ref", &format!("refs/heads/{base}"), &remote, &local]).await?;
                log(format!("fast-forwarded {base} to {}", &remote[..10]));
            }
            Some(wt) if !git::is_dirty(&wt).await => {
                git::run(&wt, &["merge", "--ff-only", "--quiet", &remote]).await?;
                log(format!("fast-forwarded {base} (checked out at {}) to {}", wt.display(), &remote[..10]));
            }
            Some(wt) => log(format!("{base} is behind origin but has local changes at {}; leaving it", wt.display())),
        }
    }
    let branches: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, branch FROM pull_requests WHERE project_id = ? AND state = 'open' AND github_node_id IS NOT NULL")
            .bind(project.id)
            .fetch_all(&app.db)
            .await?;
    let mut n = 0;
    for (pr_id, b) in branches {
        let spec = format!("+refs/heads/{b}:refs/remotes/origin/{b}");
        if let Err(e) = git::run(repo, &["fetch", "--quiet", "origin", &spec]).await {
            log(format!("fetch {b}: {e}"));
            continue;
        }
        let Some(remote) = git::rev_parse(repo, &format!("refs/remotes/origin/{b}")).await else { continue };
        match git::branch_sha(repo, &b).await {
            None => {
                git::run(repo, &["branch", &b, &remote]).await?;
                n += 1;
                log(format!("created local branch {b}"));
            }
            Some(local) if local != remote && git::is_ancestor(repo, &local, &remote).await => {
                if git::worktree::checked_out_at(repo, &b).await?.is_none() {
                    git::run(repo, &["update-ref", &format!("refs/heads/{b}"), &remote, &local]).await?;
                    n += 1;
                    log(format!("fast-forwarded {b}"));
                } else {
                    log(format!("{b} is behind origin but checked out; leaving it"));
                }
            }
            _ => {}
        }
        if let Some(h) = git::branch_sha(repo, &b).await {
            let mb = git::merge_base(repo, &project.base_branch, &h).await;
            sqlx::query("UPDATE pull_requests SET head_sha = ?, merge_base_sha = ? WHERE id = ?")
                .bind(&h)
                .bind(mb)
                .bind(pr_id)
                .execute(&app.db)
                .await?;
        }
    }
    Ok(n)
}

/// Fetch + apply + branch sync.
pub async fn run(app: &AppState, project: &Project, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<ImportStats> {
    let repo = project.github_repo.clone().ok_or_else(|| anyhow::anyhow!("project has no github_repo configured"))?;
    let data = fetch(&repo, project.github_project_owner.as_deref(), project.github_project_number, log).await?;
    let mut stats = apply(app, project, &data, log).await?;
    stats.branches_fetched = sync_open_pr_branches(app, project, log).await?;
    app.bus.emit("board.reloaded", Some(&project.slug), None, None, None);
    app.bus.scan.notify_one();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(map_status(Some("Backlog"), true, None, false).0, IssueState::Backlog);
        assert_eq!(map_status(Some("Ready to merge"), true, None, true).0, IssueState::ReadyToMerge);
        let (s, h, _) = map_status(Some("Needs decision"), true, None, true);
        assert_eq!((s, h), (IssueState::InReview, Some(Hold::NeedsDecision)));
        let (s, h, _) = map_status(Some("Needs decision"), true, None, false);
        assert_eq!((s, h), (IssueState::Backlog, Some(Hold::NeedsDecision)));
        assert_eq!(map_status(Some("In review"), false, Some("NOT_PLANNED"), false).0, IssueState::Closed);
        assert_eq!(map_status(None, false, Some("COMPLETED"), false).0, IssueState::Done);
        assert_eq!(status_name(IssueState::MergeConflict, None), "In progress");
        assert_eq!(status_name(IssueState::InReview, Some(Hold::NeedsDecision)), "Needs decision");
    }
}
