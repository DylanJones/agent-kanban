//! Workflow state machine: issue states, holds, board columns, and the transition table.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::actor::{Actor, Role};

/// Workflow state of an issue. The board column is derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum IssueState {
    /// Newly reported; waiting for a triage agent or human.
    Triage,
    Backlog,
    /// Triaged and ready for a fix agent.
    Ready,
    InProgress,
    /// A reviewer asked for changes; the fix agent picks it up.
    ChangesRequested,
    /// The PR no longer merges cleanly; the merge-prep agent picks it up.
    MergeConflict,
    InReview,
    /// Reviewer approved the current head; waiting for a human to merge.
    ReadyToMerge,
    Done,
    /// Closed without a merge (duplicate, invalid, wontfix...).
    Closed,
}

pub const ALL_STATES: [IssueState; 10] = [
    IssueState::Triage,
    IssueState::Backlog,
    IssueState::Ready,
    IssueState::InProgress,
    IssueState::ChangesRequested,
    IssueState::MergeConflict,
    IssueState::InReview,
    IssueState::ReadyToMerge,
    IssueState::Done,
    IssueState::Closed,
];

/// The four board columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Backlog,
    InProgress,
    InReview,
    Done,
}

/// Orthogonal pause flag. The card stays in its column; the scheduler skips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum Hold {
    /// A language/design decision is required from a human.
    NeedsDecision,
    /// Agents failed repeatedly or hit an unrecoverable setup problem.
    Stalled,
    /// Paused manually by a human.
    Paused,
}

impl IssueState {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueState::Triage => "triage",
            IssueState::Backlog => "backlog",
            IssueState::Ready => "ready",
            IssueState::InProgress => "in_progress",
            IssueState::ChangesRequested => "changes_requested",
            IssueState::MergeConflict => "merge_conflict",
            IssueState::InReview => "in_review",
            IssueState::ReadyToMerge => "ready_to_merge",
            IssueState::Done => "done",
            IssueState::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        ALL_STATES.into_iter().find(|st| st.as_str() == s)
    }

    pub fn column(self) -> Column {
        use IssueState::*;
        match self {
            Triage | Backlog | Ready => Column::Backlog,
            InProgress | ChangesRequested | MergeConflict => Column::InProgress,
            InReview | ReadyToMerge => Column::InReview,
            Done | Closed => Column::Done,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, IssueState::Done | IssueState::Closed)
    }

    /// The agent role the scheduler dispatches for an issue in this state, if any.
    pub fn dispatch_role(self) -> Option<Role> {
        use IssueState::*;
        match self {
            Triage => Some(Role::Triage),
            Ready | InProgress | ChangesRequested => Some(Role::Fix),
            MergeConflict => Some(Role::MergePrep),
            InReview => Some(Role::Review),
            _ => None,
        }
    }

    /// Scheduler precedence: finish work before starting new work (lower is sooner).
    pub fn precedence(self) -> i32 {
        use IssueState::*;
        match self {
            MergeConflict => 0,
            ChangesRequested => 1,
            InReview => 2,
            InProgress => 3,
            Ready => 4,
            Triage => 5,
            _ => 9,
        }
    }
}

impl Hold {
    pub fn as_str(self) -> &'static str {
        match self {
            Hold::NeedsDecision => "needs_decision",
            Hold::Stalled => "stalled",
            Hold::Paused => "paused",
        }
    }
}

/// Who may perform a transition.
#[derive(Debug, Clone, Copy)]
struct Rule {
    from: &'static [IssueState],
    to: &'static [IssueState],
    /// Agent roles allowed (only on the issue their run is bound to).
    agents: &'static [Role],
    system: bool,
}

use IssueState as S;

const NON_TERMINAL: &[IssueState] =
    &[S::Triage, S::Backlog, S::Ready, S::InProgress, S::ChangesRequested, S::MergeConflict, S::InReview, S::ReadyToMerge];

/// Transitions available to agents and the system. Humans may make any transition;
/// service-level guards (open PR, verdict at head...) still apply to everyone.
const RULES: &[Rule] = &[
    // Triage outcome.
    Rule { from: &[S::Triage], to: &[S::Backlog, S::Ready, S::Closed], agents: &[Role::Triage], system: false },
    // A fix run starting moves the issue into progress.
    Rule { from: &[S::Ready, S::ChangesRequested, S::Backlog], to: &[S::InProgress], agents: &[Role::Fix], system: true },
    // Fix agent hands off to review (requires an open PR ahead of base).
    Rule {
        from: &[S::InProgress, S::ChangesRequested, S::MergeConflict],
        to: &[S::InReview],
        agents: &[Role::Fix, Role::MergePrep],
        system: false,
    },
    // Review verdicts.
    Rule { from: &[S::InReview], to: &[S::ChangesRequested, S::ReadyToMerge], agents: &[Role::Review], system: false },
    // A new commit after approval returns the PR to review.
    Rule { from: &[S::ReadyToMerge], to: &[S::InReview], agents: &[], system: true },
    // Conflict scanner.
    Rule { from: &[S::InReview, S::ReadyToMerge], to: &[S::MergeConflict], agents: &[], system: true },
    Rule { from: &[S::MergeConflict], to: &[S::InReview], agents: &[], system: true },
    // Merge service.
    Rule { from: &[S::ReadyToMerge, S::InReview], to: &[S::Done], agents: &[], system: true },
    // PR closed without merge → back to backlog (system cleanup).
    Rule { from: NON_TERMINAL, to: &[S::Backlog], agents: &[], system: true },
];

/// Returns `Ok(())` if `actor` may move an issue from `from` to `to`.
pub fn check_transition(from: IssueState, to: IssueState, actor: &Actor) -> Result<(), String> {
    if from == to {
        return Err(format!("issue is already {}", to.as_str()));
    }
    match actor {
        Actor::Human { .. } => Ok(()),
        Actor::System => {
            if RULES.iter().any(|r| r.system && r.from.contains(&from) && r.to.contains(&to)) {
                Ok(())
            } else {
                Err(format!("system may not move {} → {}", from.as_str(), to.as_str()))
            }
        }
        Actor::Agent { role, .. } => {
            if RULES.iter().any(|r| r.agents.contains(role) && r.from.contains(&from) && r.to.contains(&to)) {
                Ok(())
            } else {
                Err(format!("a {} agent may not move an issue {} → {}", role.as_str(), from.as_str(), to.as_str()))
            }
        }
    }
}

/// All states `actor` may move an issue in `from` to.
pub fn allowed_transitions(from: IssueState, actor: &Actor) -> Vec<IssueState> {
    ALL_STATES.into_iter().filter(|&to| check_transition(from, to, actor).is_ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(role: Role) -> Actor {
        Actor::Agent { run_id: 1, role, issue_id: Some(1), project_id: 1, agent_name: "a".into() }
    }

    #[test]
    fn columns() {
        assert_eq!(S::Triage.column(), Column::Backlog);
        assert_eq!(S::MergeConflict.column(), Column::InProgress);
        assert_eq!(S::ReadyToMerge.column(), Column::InReview);
        assert_eq!(S::Closed.column(), Column::Done);
    }

    #[test]
    fn humans_can_do_anything_but_noop() {
        let h = Actor::Human { name: "dylan".into() };
        for from in ALL_STATES {
            for to in ALL_STATES {
                assert_eq!(check_transition(from, to, &h).is_ok(), from != to);
            }
        }
    }

    #[test]
    fn agent_table() {
        // Exhaustive: every (from, to, role) pair, compared against the expected allow-list.
        let expected: &[(Role, IssueState, IssueState)] = &[
            (Role::Triage, S::Triage, S::Backlog),
            (Role::Triage, S::Triage, S::Ready),
            (Role::Triage, S::Triage, S::Closed),
            (Role::Fix, S::Ready, S::InProgress),
            (Role::Fix, S::ChangesRequested, S::InProgress),
            (Role::Fix, S::Backlog, S::InProgress),
            (Role::Fix, S::InProgress, S::InReview),
            (Role::Fix, S::ChangesRequested, S::InReview),
            (Role::Fix, S::MergeConflict, S::InReview),
            (Role::MergePrep, S::InProgress, S::InReview),
            (Role::MergePrep, S::ChangesRequested, S::InReview),
            (Role::MergePrep, S::MergeConflict, S::InReview),
            (Role::Review, S::InReview, S::ChangesRequested),
            (Role::Review, S::InReview, S::ReadyToMerge),
        ];
        for role in [Role::Triage, Role::Fix, Role::Review, Role::MergePrep] {
            for from in ALL_STATES {
                for to in ALL_STATES {
                    let want = expected.contains(&(role, from, to));
                    let got = check_transition(from, to, &agent(role)).is_ok();
                    assert_eq!(got, want, "{:?} {:?}->{:?}", role, from, to);
                }
            }
        }
    }

    #[test]
    fn system_rules() {
        let s = Actor::System;
        assert!(check_transition(S::ReadyToMerge, S::InReview, &s).is_ok());
        assert!(check_transition(S::InReview, S::MergeConflict, &s).is_ok());
        assert!(check_transition(S::MergeConflict, S::InReview, &s).is_ok());
        assert!(check_transition(S::ReadyToMerge, S::Done, &s).is_ok());
        assert!(check_transition(S::Ready, S::InProgress, &s).is_ok());
        assert!(check_transition(S::InReview, S::ReadyToMerge, &s).is_err());
        assert!(check_transition(S::Done, S::Backlog, &s).is_err());
    }
}
