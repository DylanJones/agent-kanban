//! Optional one-way mirror of local activity to GitHub. Each hook enqueues a background job
//! (see `jobs.rs`) when the corresponding per-project flag is enabled.

use serde_json::json;

use crate::AppState;
use crate::domain::models::Project;
use crate::jobs;

pub async fn on_pr_created(app: &AppState, project: &Project, pr_id: i64) {
    if project.github_repo.is_some() && (project.mirror_create_prs || project.mirror_push_branches) {
        jobs::enqueue(app, "github.push_pr", Some(project.id), json!({"pr_id": pr_id, "create_pr": project.mirror_create_prs})).await;
    }
}

pub async fn on_branch_updated(app: &AppState, project: &Project, pr_id: i64) {
    if project.github_repo.is_some() && project.mirror_push_branches {
        jobs::enqueue(app, "github.push_pr", Some(project.id), json!({"pr_id": pr_id, "create_pr": false})).await;
    }
}

pub async fn on_review(app: &AppState, project: &Project, pr_id: i64, summary: &str) {
    if project.github_repo.is_some() && project.mirror_post_verdicts {
        jobs::enqueue(app, "github.pr_comment", Some(project.id), json!({"pr_id": pr_id, "body": summary})).await;
    }
}

pub async fn on_merged(app: &AppState, project: &Project, pr_id: i64) {
    if project.github_repo.is_some() && project.mirror_push_branches {
        jobs::enqueue(app, "github.merged", Some(project.id), json!({"pr_id": pr_id})).await;
    }
}

pub async fn on_issue_state(app: &AppState, project: &Project, issue_id: i64) {
    if project.github_project_id.is_some() && project.mirror_sync_status {
        jobs::enqueue(app, "github.sync_status", Some(project.id), json!({"issue_id": issue_id})).await;
    }
}
