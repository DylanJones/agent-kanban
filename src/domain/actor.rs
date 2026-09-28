//! Who is making a request: a human, an agent run, or the system itself.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The job an agent run performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum Role {
    /// Reads a newly reported issue and decides whether/how it should be worked.
    Triage,
    /// Implements a fix, opens a PR, and addresses review feedback.
    Fix,
    /// Reviews a PR and records a verdict.
    Review,
    /// Resolves merge conflicts with the base branch.
    MergePrep,
}

pub const ALL_ROLES: [Role; 4] = [Role::Triage, Role::Fix, Role::Review, Role::MergePrep];

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Triage => "triage",
            Role::Fix => "fix",
            Role::Review => "review",
            Role::MergePrep => "merge_prep",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        ALL_ROLES.into_iter().find(|r| r.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Actor {
    Human {
        name: String,
    },
    Agent {
        run_id: i64,
        role: Role,
        /// The issue the run is bound to (probes have none).
        issue_id: Option<i64>,
        project_id: i64,
        agent_name: String,
    },
    System,
}

impl Actor {
    pub fn human(name: impl Into<String>) -> Self {
        Actor::Human { name: name.into() }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Actor::Human { .. } => "human",
            Actor::Agent { .. } => "agent",
            Actor::System => "system",
        }
    }

    pub fn name(&self) -> String {
        match self {
            Actor::Human { name } => name.clone(),
            Actor::Agent { agent_name, role, run_id, .. } => format!("{agent_name} ({} run #{run_id})", role.as_str()),
            Actor::System => "system".into(),
        }
    }

    pub fn run_id(&self) -> Option<i64> {
        match self {
            Actor::Agent { run_id, .. } => Some(*run_id),
            _ => None,
        }
    }

    pub fn is_human(&self) -> bool {
        matches!(self, Actor::Human { .. })
    }
}
