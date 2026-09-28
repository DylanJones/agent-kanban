use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Human;
use crate::db;
use crate::domain::models::{Job, Label, Project};
use crate::domain::{Actor, Role};
use crate::error::{ApiError, ApiResult};
use crate::{jobs, services};

#[derive(Debug, Deserialize, ToSchema)]
pub struct NewProject {
    /// URL-safe identifier, e.g. `emojicode`.
    pub slug: String,
    pub name: Option<String>,
    /// Absolute path of an existing git checkout.
    pub repo_path: String,
    pub base_branch: Option<String>,
    /// `owner/name` on GitHub, for import/mirroring.
    pub github_repo: Option<String>,
    pub github_project_owner: Option<String>,
    pub github_project_number: Option<i64>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub repo_path: Option<String>,
    pub base_branch: Option<String>,
    pub branch_prefix: Option<String>,
    pub merge_strategy: Option<String>,
    /// Empty string clears.
    pub commit_msg_regex: Option<String>,
    pub setup_script: Option<String>,
    pub agent_instructions: Option<String>,
    /// 0 clears the per-project cap.
    pub max_concurrent_runs: Option<i64>,
    pub container_enabled: Option<bool>,
    pub container_dockerfile: Option<String>,
    pub container_context: Option<String>,
    pub container_extra_args: Option<Vec<String>>,
    pub github_repo: Option<String>,
    pub github_project_owner: Option<String>,
    pub github_project_number: Option<i64>,
    pub mirror_push_branches: Option<bool>,
    pub mirror_create_prs: Option<bool>,
    pub mirror_sync_status: Option<bool>,
    pub mirror_create_issues: Option<bool>,
    pub mirror_post_verdicts: Option<bool>,
}

/// List projects.
#[utoipa::path(operation_id = "projects_list", get, path = "/api/projects", tag = "projects", responses((status = 200, body = Vec<Project>)))]
pub async fn list(State(app): State<AppState>, _a: Actor) -> ApiResult<Json<Vec<Project>>> {
    Ok(Json(sqlx::query_as::<_, Project>("SELECT * FROM projects ORDER BY name").fetch_all(&app.db).await?))
}

/// Register a git repository as a project.
#[utoipa::path(operation_id = "projects_create", post, path = "/api/projects", tag = "projects", request_body = NewProject, responses((status = 201, body = Project)))]
pub async fn create(State(app): State<AppState>, Human(_a): Human, Json(req): Json<NewProject>) -> ApiResult<(StatusCode, Json<Project>)> {
    Ok((StatusCode::CREATED, Json(create_project(&app, req).await?)))
}

pub async fn create_project(app: &AppState, req: NewProject) -> ApiResult<Project> {
    let slug = req.slug.trim().to_lowercase();
    if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::bad("slug must be alphanumeric (with - or _)"));
    }
    let repo = std::path::Path::new(&req.repo_path);
    if !repo.join(".git").exists() {
        return Err(ApiError::bad(format!("{} is not a git checkout", req.repo_path)));
    }
    let repo_path = repo.canonicalize().map_err(|e| ApiError::bad(e.to_string()))?.to_string_lossy().into_owned();
    let base = match req.base_branch {
        Some(b) => b,
        None => crate::git::run(&repo_path, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .await
            .ok()
            .and_then(|s| s.strip_prefix("origin/").map(str::to_string))
            .unwrap_or_else(|| "main".into()),
    };
    let now = db::now();
    sqlx::query(
        "INSERT INTO projects(slug, name, repo_path, base_branch, github_repo, github_project_owner, github_project_number, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&slug)
    .bind(req.name.unwrap_or_else(|| slug.clone()))
    .bind(&repo_path)
    .bind(&base)
    .bind(&req.github_repo)
    .bind(&req.github_project_owner)
    .bind(req.github_project_number)
    .bind(&now)
    .bind(&now)
    .execute(&app.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::conflict(format!("project `{slug}` exists")),
        e => e.into(),
    })?;
    app.bus.emit("projects.updated", Some(&slug), None, None, None);
    services::project(&app.db, &slug).await
}

/// Get a project.
#[utoipa::path(operation_id = "projects_get", get, path = "/api/projects/{p}", tag = "projects", params(("p" = String, Path)), responses((status = 200, body = Project)))]
pub async fn get(State(app): State<AppState>, _a: Actor, Path(p): Path<String>) -> ApiResult<Json<Project>> {
    Ok(Json(services::project(&app.db, &p).await?))
}

/// Update project settings.
#[utoipa::path(operation_id = "projects_patch", patch, path = "/api/projects/{p}", tag = "projects", params(("p" = String, Path)), request_body = ProjectPatch, responses((status = 200, body = Project)))]
pub async fn patch(
    State(app): State<AppState>,
    Human(_a): Human,
    Path(p): Path<String>,
    Json(req): Json<ProjectPatch>,
) -> ApiResult<Json<Project>> {
    let project = services::project(&app.db, &p).await?;
    if let Some(s) = &req.merge_strategy
        && !matches!(s.as_str(), "squash" | "merge" | "rebase")
    {
        return Err(ApiError::bad("merge_strategy must be squash, merge or rebase"));
    }
    if let Some(r) = req.commit_msg_regex.as_deref().filter(|r| !r.is_empty()) {
        regex::Regex::new(r).map_err(|e| ApiError::bad(format!("invalid regex: {e}")))?;
    }
    macro_rules! set {
        ($field:ident, $col:literal) => {
            if let Some(v) = &req.$field {
                sqlx::query(concat!("UPDATE projects SET ", $col, " = ? WHERE id = ?")).bind(v).bind(project.id).execute(&app.db).await?;
            }
        };
    }
    macro_rules! set_opt_str {
        ($field:ident, $col:literal) => {
            if let Some(v) = &req.$field {
                sqlx::query(concat!("UPDATE projects SET ", $col, " = NULLIF(?, '') WHERE id = ?"))
                    .bind(v)
                    .bind(project.id)
                    .execute(&app.db)
                    .await?;
            }
        };
    }
    set!(name, "name");
    set!(repo_path, "repo_path");
    set!(base_branch, "base_branch");
    set!(branch_prefix, "branch_prefix");
    set!(merge_strategy, "merge_strategy");
    set_opt_str!(commit_msg_regex, "commit_msg_regex");
    set_opt_str!(setup_script, "setup_script");
    set_opt_str!(agent_instructions, "agent_instructions");
    set!(container_enabled, "container_enabled");
    set_opt_str!(container_dockerfile, "container_dockerfile");
    set_opt_str!(container_context, "container_context");
    set_opt_str!(github_repo, "github_repo");
    set_opt_str!(github_project_owner, "github_project_owner");
    set!(github_project_number, "github_project_number");
    set!(mirror_push_branches, "mirror_push_branches");
    set!(mirror_create_prs, "mirror_create_prs");
    set!(mirror_sync_status, "mirror_sync_status");
    set!(mirror_create_issues, "mirror_create_issues");
    set!(mirror_post_verdicts, "mirror_post_verdicts");
    if let Some(n) = req.max_concurrent_runs {
        sqlx::query("UPDATE projects SET max_concurrent_runs = NULLIF(?, 0) WHERE id = ?")
            .bind(n)
            .bind(project.id)
            .execute(&app.db)
            .await?;
    }
    if let Some(a) = &req.container_extra_args {
        sqlx::query("UPDATE projects SET container_extra_args = ? WHERE id = ?")
            .bind(serde_json::to_string(a).unwrap())
            .bind(project.id)
            .execute(&app.db)
            .await?;
    }
    sqlx::query("UPDATE projects SET updated_at = ? WHERE id = ?").bind(db::now()).bind(project.id).execute(&app.db).await?;
    app.bus.emit("projects.updated", Some(&p), None, None, None);
    Ok(Json(services::project(&app.db, &p).await?))
}

/// List a project's labels.
#[utoipa::path(operation_id = "projects_labels", get, path = "/api/projects/{p}/labels", tag = "projects", params(("p" = String, Path)), responses((status = 200, body = Vec<Label>)))]
pub async fn labels(State(app): State<AppState>, _a: Actor, Path(p): Path<String>) -> ApiResult<Json<Vec<Label>>> {
    let project = services::project(&app.db, &p).await?;
    Ok(Json(services::labels::project_labels(&app.db, project.id).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LabelInput {
    pub name: String,
    /// Hex color without `#`.
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Create or update a label.
#[utoipa::path(operation_id = "projects_upsert_label", post, path = "/api/projects/{p}/labels", tag = "projects", params(("p" = String, Path)), request_body = LabelInput, responses((status = 200, body = Vec<Label>)))]
pub async fn upsert_label(
    State(app): State<AppState>,
    Human(_a): Human,
    Path(p): Path<String>,
    Json(req): Json<LabelInput>,
) -> ApiResult<Json<Vec<Label>>> {
    let project = services::project(&app.db, &p).await?;
    sqlx::query(
        "INSERT INTO labels(project_id, name, color, description) VALUES (?, ?, COALESCE(?, '8b949e'), ?)
         ON CONFLICT(project_id, name) DO UPDATE SET color = COALESCE(excluded.color, color), description = COALESCE(excluded.description, description)",
    )
    .bind(project.id)
    .bind(req.name.trim())
    .bind(req.color.map(|c| c.trim_start_matches('#').to_string()))
    .bind(req.description)
    .execute(&app.db)
    .await?;
    app.bus.emit("labels.updated", Some(&p), None, None, None);
    Ok(Json(services::labels::project_labels(&app.db, project.id).await?))
}

/// Delete a label.
#[utoipa::path(operation_id = "projects_delete_label", delete, path = "/api/projects/{p}/labels/{name}", tag = "projects", params(("p" = String, Path), ("name" = String, Path)), responses((status = 204)))]
pub async fn delete_label(State(app): State<AppState>, Human(_a): Human, Path((p, name)): Path<(String, String)>) -> ApiResult<StatusCode> {
    let project = services::project(&app.db, &p).await?;
    sqlx::query("DELETE FROM labels WHERE project_id = ? AND name = ?").bind(project.id).bind(name).execute(&app.db).await?;
    app.bus.emit("labels.updated", Some(&p), None, None, None);
    Ok(StatusCode::NO_CONTENT)
}

/// Agent definition slug assigned to each role for this project (falls back to the global default).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RoleAgents {
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub roles: BTreeMap<String, String>,
}

/// Which agent runs each role in this project.
#[utoipa::path(operation_id = "projects_get_roles", get, path = "/api/projects/{p}/roles", tag = "projects", params(("p" = String, Path)), responses((status = 200, body = RoleAgents)))]
pub async fn get_roles(State(app): State<AppState>, _a: Actor, Path(p): Path<String>) -> ApiResult<Json<RoleAgents>> {
    let project = services::project(&app.db, &p).await?;
    let mut roles: BTreeMap<String, String> = db::get_setting(&app.db, "default_role_agents").await.unwrap_or_default();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT r.role, a.slug FROM project_role_agents r JOIN agent_definitions a ON a.id = r.agent_definition_id WHERE r.project_id = ?",
    )
    .bind(project.id)
    .fetch_all(&app.db)
    .await?;
    roles.extend(rows);
    Ok(Json(RoleAgents { roles }))
}

/// Assign agents to roles for this project.
#[utoipa::path(operation_id = "projects_put_roles", put, path = "/api/projects/{p}/roles", tag = "projects", params(("p" = String, Path)), request_body = RoleAgents, responses((status = 200, body = RoleAgents)))]
pub async fn put_roles(
    State(app): State<AppState>,
    Human(a): Human,
    Path(p): Path<String>,
    Json(req): Json<RoleAgents>,
) -> ApiResult<Json<RoleAgents>> {
    let project = services::project(&app.db, &p).await?;
    for (role, slug) in &req.roles {
        let role = Role::parse(role).ok_or_else(|| ApiError::bad(format!("unknown role {role}")))?;
        let aid: i64 = sqlx::query_scalar("SELECT id FROM agent_definitions WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(|| ApiError::bad(format!("unknown agent {slug}")))?;
        sqlx::query("INSERT INTO project_role_agents(project_id, role, agent_definition_id) VALUES (?, ?, ?) ON CONFLICT DO UPDATE SET agent_definition_id = excluded.agent_definition_id")
            .bind(project.id)
            .bind(role.as_str())
            .bind(aid)
            .execute(&app.db)
            .await?;
    }
    app.bus.emit("projects.updated", Some(&p), None, None, None);
    get_roles(State(app), a, Path(p)).await
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PromptTemplate {
    pub role: String,
    /// minijinja template.
    pub body: String,
    /// True if this project overrides the built-in default.
    #[serde(default)]
    pub customized: bool,
}

/// Get the prompt template for a role.
#[utoipa::path(operation_id = "projects_get_prompt", get, path = "/api/projects/{p}/prompts/{role}", tag = "projects", params(("p" = String, Path), ("role" = String, Path)), responses((status = 200, body = PromptTemplate)))]
pub async fn get_prompt(
    State(app): State<AppState>,
    _a: Actor,
    Path((p, role)): Path<(String, String)>,
) -> ApiResult<Json<PromptTemplate>> {
    let project = services::project(&app.db, &p).await?;
    let role = Role::parse(&role).ok_or_else(|| ApiError::bad("unknown role"))?;
    let custom: Option<String> = sqlx::query_scalar("SELECT body FROM prompt_templates WHERE project_id = ? AND role = ?")
        .bind(project.id)
        .bind(role.as_str())
        .fetch_optional(&app.db)
        .await?;
    Ok(Json(PromptTemplate {
        role: role.as_str().into(),
        customized: custom.is_some(),
        body: custom.unwrap_or_else(|| crate::orchestrator::prompt::default_template(role).to_string()),
    }))
}

/// Override (or with an empty body, reset) the prompt template for a role.
#[utoipa::path(operation_id = "projects_put_prompt", put, path = "/api/projects/{p}/prompts/{role}", tag = "projects", params(("p" = String, Path), ("role" = String, Path)), request_body = PromptTemplate, responses((status = 200, body = PromptTemplate)))]
pub async fn put_prompt(
    State(app): State<AppState>,
    Human(a): Human,
    Path((p, role)): Path<(String, String)>,
    Json(req): Json<PromptTemplate>,
) -> ApiResult<Json<PromptTemplate>> {
    let project = services::project(&app.db, &p).await?;
    let r = Role::parse(&role).ok_or_else(|| ApiError::bad("unknown role"))?;
    if req.body.trim().is_empty() {
        sqlx::query("DELETE FROM prompt_templates WHERE project_id = ? AND role = ?")
            .bind(project.id)
            .bind(r.as_str())
            .execute(&app.db)
            .await?;
    } else {
        minijinja::Environment::new().template_from_str(&req.body).map_err(|e| ApiError::bad(format!("template error: {e}")))?;
        sqlx::query("INSERT INTO prompt_templates(project_id, role, body, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at")
            .bind(project.id)
            .bind(r.as_str())
            .bind(&req.body)
            .bind(db::now())
            .execute(&app.db)
            .await?;
    }
    get_prompt(State(app), a, Path((p, role))).await
}

/// Import (or re-sync) issues, PRs, reviews and board status from GitHub. Runs as a background job.
#[utoipa::path(operation_id = "projects_github_import", post, path = "/api/projects/{p}/github/import", tag = "github", params(("p" = String, Path)), responses((status = 202, body = Job)))]
pub async fn github_import(State(app): State<AppState>, Human(_a): Human, Path(p): Path<String>) -> ApiResult<(StatusCode, Json<Job>)> {
    let project = services::project(&app.db, &p).await?;
    if project.github_repo.is_none() {
        return Err(ApiError::bad("set github_repo on the project first"));
    }
    let id = jobs::enqueue(&app, "github.import", Some(project.id), json!({})).await;
    Ok((StatusCode::ACCEPTED, Json(job_by_id(&app, id).await?)))
}

/// Push the board status of every open issue to GitHub (requires `mirror_sync_status`).
#[utoipa::path(operation_id = "projects_github_sync", post, path = "/api/projects/{p}/github/sync", tag = "github", params(("p" = String, Path)), responses((status = 202)))]
pub async fn github_sync(State(app): State<AppState>, Human(_a): Human, Path(p): Path<String>) -> ApiResult<StatusCode> {
    let project = services::project(&app.db, &p).await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM issues WHERE project_id = ? AND github_node_id IS NOT NULL")
        .bind(project.id)
        .fetch_all(&app.db)
        .await?;
    for id in ids {
        jobs::enqueue(&app, "github.sync_status", Some(project.id), json!({"issue_id": id})).await;
    }
    Ok(StatusCode::ACCEPTED)
}

/// Build the project's agent container image. Runs as a background job.
#[utoipa::path(operation_id = "projects_container_build", post, path = "/api/projects/{p}/container/build", tag = "projects", params(("p" = String, Path)), responses((status = 202, body = Job)))]
pub async fn container_build(State(app): State<AppState>, Human(_a): Human, Path(p): Path<String>) -> ApiResult<(StatusCode, Json<Job>)> {
    let project = services::project(&app.db, &p).await?;
    let id = jobs::enqueue(&app, "container.build", Some(project.id), json!({"at": db::now()})).await;
    Ok((StatusCode::ACCEPTED, Json(job_by_id(&app, id).await?)))
}

async fn job_by_id(app: &AppState, id: i64) -> ApiResult<Job> {
    sqlx::query_as::<_, Job>("SELECT * FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("job {id}")))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct JobQuery {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// List background jobs.
#[utoipa::path(operation_id = "projects_list_jobs", get, path = "/api/jobs", tag = "jobs", params(JobQuery), responses((status = 200, body = Vec<Job>)))]
pub async fn list_jobs(State(app): State<AppState>, _a: Actor, Query(q): Query<JobQuery>) -> ApiResult<Json<Vec<Job>>> {
    Ok(Json(
        sqlx::query_as::<_, Job>("SELECT * FROM jobs WHERE (? IS NULL OR kind = ?) AND (? IS NULL OR status = ?) ORDER BY id DESC LIMIT ?")
            .bind(&q.kind)
            .bind(&q.kind)
            .bind(&q.status)
            .bind(&q.status)
            .bind(q.limit.unwrap_or(50))
            .fetch_all(&app.db)
            .await?,
    ))
}

/// Get a background job and its log.
#[utoipa::path(operation_id = "projects_get_job", get, path = "/api/jobs/{id}", tag = "jobs", params(("id" = i64, Path)), responses((status = 200, body = Job)))]
pub async fn get_job(State(app): State<AppState>, _a: Actor, Path(id): Path<i64>) -> ApiResult<Json<Job>> {
    Ok(Json(job_by_id(&app, id).await?))
}
