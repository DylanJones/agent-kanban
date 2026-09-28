//! HTTP API. Every route is annotated for OpenAPI; the spec is served at `/api/openapi.json`
//! and interactive docs at `/api/docs`.

pub mod agents;
pub mod board;
pub mod credentials;
pub mod issues;
pub mod mcp;
pub mod meta;
pub mod projects;
pub mod pulls;
pub mod usage;

use axum::Router;
use axum::routing::get;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_scalar::{Scalar, Servable};

use crate::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "agent-kanban API",
        description = r#"Local kanban board that orchestrates coding agents (Claude Code, Codex, OpenCode) over the Agent Client Protocol.

**Auth:** send `Authorization: Bearer <token>`. Humans use the admin token or an API token; agents receive a per-run token (`akr_…`) scoped to their project and issue.

**Agents:** start with `GET /api/agent-guide` — a task-oriented guide with copy-paste curl commands. The fastest way to record a bug you noticed is:

```sh
curl -sS -X POST "$AKB_API/projects/$P/issues" -H "Authorization: Bearer $AKB_AUTH" \
  -H 'Content-Type: text/plain' --data-binary $'Title\nDetails'
```

**Workflow:** `triage → ready → in_progress → in_review → ready_to_merge → done`, with `changes_requested` and `merge_conflict` sending work back to the fix agent, and a `needs_decision` hold pausing any issue for a human. Errors are RFC 7807 problem+json."#,
        version = env!("CARGO_PKG_VERSION"),
    ),
    modifiers(&SecurityAddon),
    security(("bearer" = [])),
    tags(
        (name = "issues", description = "Issues, comments, workflow transitions and decisions"),
        (name = "board", description = "Kanban board view"),
        (name = "pulls", description = "Local pull requests, diffs and merging"),
        (name = "reviews", description = "Inline review threads and verdicts"),
        (name = "runs", description = "Agent runs, live transcripts and permission prompts"),
        (name = "agents", description = "Agent definitions (ACP adapters) and subscription limit groups"),
        (name = "projects", description = "Projects, labels, role assignments and prompt templates"),
        (name = "github", description = "GitHub import and mirroring"),
        (name = "settings", description = "Global settings and API tokens"),
        (name = "jobs", description = "Background jobs"),
        (name = "usage", description = "Token usage reports by model, subscription, role, agent, project and day"),
        (name = "meta", description = "Health, identity, live events and the agent guide"),
    )
)]
pub struct ApiDoc;

struct SecurityAddon;
impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let c = openapi.components.get_or_insert_with(Default::default);
        c.add_security_scheme("bearer", SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()));
    }
}

pub fn router() -> (Router<AppState>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(meta::health))
        .routes(routes!(meta::me))
        .routes(routes!(meta::agent_guide))
        .routes(routes!(meta::get_settings, meta::patch_settings))
        .routes(routes!(meta::list_tokens, meta::create_token))
        .routes(routes!(meta::revoke_token))
        .routes(routes!(meta::events_stream))
        .routes(routes!(projects::list, projects::create))
        .routes(routes!(projects::get, projects::patch))
        .routes(routes!(projects::labels, projects::upsert_label))
        .routes(routes!(projects::delete_label))
        .routes(routes!(projects::get_roles, projects::put_roles))
        .routes(routes!(projects::get_prompt, projects::put_prompt))
        .routes(routes!(projects::github_import))
        .routes(routes!(projects::github_sync))
        .routes(routes!(projects::container_build))
        .routes(routes!(projects::list_jobs))
        .routes(routes!(projects::get_job))
        .routes(routes!(board::board))
        .routes(routes!(issues::list, issues::create))
        .routes(routes!(issues::get, issues::patch))
        .routes(routes!(issues::transition))
        .routes(routes!(issues::decision_request))
        .routes(routes!(issues::decision))
        .routes(routes!(issues::put_hold, issues::delete_hold))
        .routes(routes!(issues::list_comments, issues::add_comment))
        .routes(routes!(issues::events))
        .routes(routes!(issues::edit_comment, issues::delete_comment))
        .routes(routes!(pulls::list, pulls::create))
        .routes(routes!(pulls::get, pulls::patch))
        .routes(routes!(pulls::diff))
        .routes(routes!(pulls::commits))
        .routes(routes!(pulls::list_comments, pulls::add_comment))
        .routes(routes!(pulls::list_threads, pulls::create_thread))
        .routes(routes!(pulls::reply))
        .routes(routes!(pulls::resolve))
        .routes(routes!(pulls::unresolve))
        .routes(routes!(pulls::list_reviews, pulls::submit_review))
        .routes(routes!(pulls::mergeability))
        .routes(routes!(pulls::do_merge))
        .routes(routes!(pulls::close))
        .routes(routes!(agents::list, agents::create))
        .routes(routes!(agents::get, agents::patch, agents::delete))
        .routes(routes!(agents::test))
        .routes(routes!(agents::limit_groups))
        .routes(routes!(agents::pause))
        .routes(routes!(agents::resume))
        .routes(routes!(agents::probe_now))
        .routes(routes!(agents::list_runs))
        .routes(routes!(agents::current))
        .routes(routes!(agents::get_run))
        .routes(routes!(agents::run_events))
        .routes(routes!(agents::run_stream))
        .routes(routes!(agents::cancel))
        .routes(routes!(agents::message))
        .routes(routes!(agents::start))
        .routes(routes!(agents::permissions))
        .routes(routes!(agents::answer_permission))
        .routes(routes!(credentials::get))
        .routes(routes!(credentials::put_claude, credentials::delete_claude))
        .routes(routes!(usage::report))
        .routes(routes!(usage::subscriptions))
        .split_for_parts()
}

pub fn app(state: AppState) -> Router {
    let (api, spec) = router();
    let spec_json = serde_json::to_string_pretty(&spec).unwrap();
    Router::new()
        .merge(api)
        .route("/api/openapi.json", get(move || async move { ([(axum::http::header::CONTENT_TYPE, "application/json")], spec_json) }))
        .merge(Scalar::with_url("/api/docs", spec))
        .route("/login", get(meta::login))
        .route("/mcp", axum::routing::post(mcp::post).get(mcp::get))
        .fallback(crate::web::static_handler)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
