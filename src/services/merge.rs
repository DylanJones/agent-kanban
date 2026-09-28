//! Human merge: builds the merge commit with plumbing and advances the base branch with a
//! compare-and-swap, without touching anyone's working checkout.

use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::{pulls, record_event};
use crate::AppState;
use crate::db::{self, begin_write};
use crate::domain::models::Project;
use crate::domain::{Actor, IssueState};
use crate::error::{ApiError, ApiResult};
use crate::git;

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct MergeRequest {
    /// `squash`, `merge` or `rebase`; defaults to the project setting.
    pub strategy: Option<String>,
    /// Commit message for squash/merge commits; defaults to the PR title plus `Closes #n` lines.
    pub message: Option<String>,
    /// Merge even without an approving review at the current head.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MergeResult {
    pub merged_sha: String,
    pub strategy: String,
    pub base_branch: String,
}

pub async fn default_message(app: &AppState, pr_id: i64, title: &str) -> String {
    let nums = pulls::linked_issue_numbers(&app.db, pr_id).await.unwrap_or_default();
    let mut msg = title.trim().to_string();
    if !nums.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&nums.iter().map(|n| format!("Closes #{n}")).collect::<Vec<_>>().join("\n"));
    }
    msg
}

pub async fn merge(app: &AppState, project: &Project, number: i64, actor: &Actor, req: MergeRequest) -> ApiResult<MergeResult> {
    if !actor.is_human() {
        return Err(ApiError::forbidden("only a human can merge"));
    }
    let pr = super::pull(&app.db, project.id, number).await?;
    if pr.state != "open" {
        return Err(ApiError::conflict(format!("PR #{number} is {}", pr.state)));
    }
    let pr = pulls::refresh_head(app, project, &pr).await?;
    let repo = project.repo_path.as_str();
    let head = pr.head_sha.clone().ok_or_else(|| ApiError::conflict("PR branch has no head"))?;
    let base_ref = format!("refs/heads/{}", pr.base_branch);
    let base = git::rev_parse(repo, &base_ref).await.ok_or_else(|| ApiError::conflict(format!("base `{}` not found", pr.base_branch)))?;

    let issue_ids = super::pr_issue_ids(&app.db, pr.id).await?;
    let mut issues = Vec::new();
    for id in &issue_ids {
        issues.push(super::issue_by_id(&app.db, *id).await?);
    }
    if !req.force {
        if pr.approved_sha.as_deref() != Some(head.as_str()) {
            return Err(ApiError::conflict("no approving review at the current head; review first or pass `force: true`"));
        }
        if let Some(i) = issues.iter().find(|i| i.state != IssueState::ReadyToMerge) {
            return Err(ApiError::conflict(format!("issue #{} is `{}`, not ready to merge", i.number, i.state.as_str())));
        }
    }
    if git::commits_ahead(repo, &base, &head).await.unwrap_or(0) == 0 {
        return Err(ApiError::conflict("nothing to merge: branch has no commits ahead of base"));
    }
    let mt = git::merge_tree(repo, &base, &head).await?;
    if !mt.clean {
        return Err(ApiError::conflict(format!("merge conflicts in: {}", mt.conflicts.join(", "))));
    }

    let strategy = req.strategy.clone().unwrap_or_else(|| project.merge_strategy.clone());
    let message = match &req.message {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => default_message(app, pr.id, &pr.title).await,
    };
    if strategy != "rebase"
        && let Some(re) = project.commit_msg_regex.as_deref().and_then(|r| regex::Regex::new(r).ok())
        && !re.is_match(message.lines().next().unwrap_or_default())
    {
        return Err(ApiError::bad(format!("commit message must match `{}`", re.as_str())));
    }

    let new = match strategy.as_str() {
        "squash" => git::run(repo, &["commit-tree", &mt.tree, "-p", &base, "-m", &message]).await?,
        "merge" => git::run(repo, &["commit-tree", &mt.tree, "-p", &base, "-p", &head, "-m", &message]).await?,
        "rebase" => {
            let branch_ref = format!("refs/heads/{}", pr.branch);
            let range = format!("{base}..{branch_ref}");
            let out = git::run(repo, &["replay", "--ref-action=print", "--onto", &base, &range])
                .await
                .map_err(|e| ApiError::conflict(format!("rebase via `git replay` failed ({e}); use squash or merge")))?;
            out.lines()
                .filter_map(|l| l.strip_prefix("update "))
                .find_map(|l| {
                    let mut p = l.split_whitespace();
                    (p.next() == Some(branch_ref.as_str())).then(|| p.next().map(str::to_string)).flatten()
                })
                .ok_or_else(|| ApiError::conflict("git replay produced no result; use squash or merge"))?
        }
        s => return Err(ApiError::bad(format!("unknown merge strategy `{s}`"))),
    };

    // Advance base. If base is checked out somewhere, fast-forward that worktree so it stays consistent.
    match git::worktree::checked_out_at(repo, &pr.base_branch).await? {
        Some(wt) => {
            if git::is_dirty(&wt).await {
                return Err(ApiError::conflict(format!(
                    "`{}` is checked out with local changes at {}; commit or stash them, then merge again",
                    pr.base_branch,
                    wt.display()
                )));
            }
            let cur = git::run(&wt, &["rev-parse", "HEAD"]).await?;
            if cur != base {
                return Err(ApiError::conflict("base branch moved during merge; retry"));
            }
            git::run(&wt, &["merge", "--ff-only", "--quiet", &new]).await?;
        }
        None => {
            git::run(repo, &["update-ref", "-m", &format!("agent-kanban: merge PR #{number}"), &base_ref, &new, &base])
                .await
                .map_err(|e| ApiError::conflict(format!("base branch moved during merge; retry ({e})")))?;
        }
    }

    let now = db::now();
    let mut tx = begin_write(&app.db).await?;
    sqlx::query(
        "UPDATE pull_requests SET state = 'merged', merged_sha = ?, merged_at = ?, merge_strategy = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&new)
    .bind(&now)
    .bind(&strategy)
    .bind(&now)
    .bind(pr.id)
    .execute(&mut *tx)
    .await?;
    super::comments::insert(
        &mut tx,
        project.id,
        super::comments::Target::Pr(pr.id),
        actor,
        "system",
        &format!("🎉 Merged into `{}` as `{}` ({strategy})", pr.base_branch, &new[..10.min(new.len())]),
    )
    .await?;
    record_event(&mut tx, Some(project.id), None, Some(pr.id), actor, "pr.merged", json!({"sha": new, "strategy": strategy})).await?;
    tx.commit().await?;

    for i in &issues {
        let i = super::issue_by_id(&app.db, i.id).await?;
        if !i.state.is_terminal() {
            super::issues::set_state(app, project, &i, IssueState::Done, actor, Some(&format!("Merged via PR #{number}.")), None).await?;
        }
    }
    app.bus.emit("pr.merged", Some(&project.slug), None, Some(number), None);
    app.bus.scan.notify_one();
    crate::github::mirror::on_merged(app, project, pr.id).await;
    Ok(MergeResult { merged_sha: new, strategy, base_branch: pr.base_branch })
}

#[cfg(test)]
mod tests {
    #[test]
    fn emoji_commit_regex() {
        let re = regex::Regex::new(r"^\p{Extended_Pictographic}").unwrap();
        for ok in ["🦁 Fix sorting", "🍿 Let ↩️ give", "⤴️ thing", "🐛 bug"] {
            assert!(re.is_match(ok), "{ok}");
        }
        assert!(!re.is_match("Fix sorting"));
    }
}
