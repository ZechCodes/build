//! Branch and issue are the only work items.
//!
//! A branch work item is identified by `(project_id, branch_name)`. What the
//! bridge stores underneath — a run, an adopted worktree, an external worktree
//! Build never cut, the primary checkout — is an implementation detail, and
//! more than one of them can describe the same branch at once. This module owns
//! the two rules that turn those sources into one feed:
//!
//! 1. **Folding.** Rows that share a key are the same work item seen twice; the
//!    source that knows the most about it wins (a run over the primary
//!    checkout, the primary checkout over a bare external worktree).
//! 2. **Dedup.** An issue whose implementation is still in flight speaks as
//!    that branch row alone — the branch row carries the `issue_id` and the
//!    issue's own row is suppressed.
//!
//! Both are pure functions over already-built rows, so the policy is testable
//! without a repo, a store, or an RPC.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// The two kinds of work item, as they ship on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemKind {
    Branch,
    Issue,
}

impl WorkItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkItemKind::Branch => "branch",
            WorkItemKind::Issue => "issue",
        }
    }
}

/// What a branch row's facts were read off. The order of the variants is the
/// order they win in: a run knows the branch's lifecycle, conversation and
/// agents; the primary checkout knows it is the repository; a bare external
/// worktree knows only what git can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BranchSource {
    Run,
    PrimaryCheckout,
    ExternalWorktree,
}

/// The identity rows fold on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkItemKey {
    /// A branch, identified the way the design doc identifies one.
    Branch {
        project_id: String,
        branch: String,
    },
    /// A checkout with no branch to name it by (a detached worktree). It is
    /// still work, so it still gets a row; it just cannot collide with one.
    Checkout {
        worktree_id: String,
    },
    Issue {
        issue_id: String,
    },
}

/// One row as it goes into the fold: its key, where it came from, and the facts
/// the fold decides on. `row` is the wire payload and is never inspected here.
#[derive(Debug, Clone)]
pub struct WorkItemCandidate {
    pub kind: WorkItemKind,
    pub key: WorkItemKey,
    /// `None` for issue rows — an issue has no source to compete over.
    pub source: Option<BranchSource>,
    /// The issue this branch implements, when it implements one.
    pub issue_id: Option<String>,
    /// Whether the implementation behind this branch row is still in flight.
    /// Only a live implementation suppresses its issue's row: once it is
    /// merged or abandoned, the issue speaks for itself again.
    pub implementation_active: bool,
    pub row: Value,
}

/// Fold the candidates into the feed's `items[]`.
///
/// Input order is the output order: callers decide how the feed sorts, and a
/// row that loses its key never reorders the one that won it.
pub fn fold_work_items(candidates: Vec<WorkItemCandidate>) -> Vec<Value> {
    let mut winner_of: HashMap<WorkItemKey, usize> = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        match winner_of.get(&candidate.key) {
            Some(&held) => {
                if candidate.source < candidates[held].source {
                    winner_of.insert(candidate.key.clone(), index);
                }
            }
            None => {
                winner_of.insert(candidate.key.clone(), index);
            }
        }
    }
    let survivors: HashSet<usize> = winner_of.values().copied().collect();
    let spoken_for: HashSet<&str> = candidates
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            survivors.contains(index)
                && candidate.kind == WorkItemKind::Branch
                && candidate.implementation_active
        })
        .filter_map(|(_, candidate)| candidate.issue_id.as_deref())
        .collect();
    candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| survivors.contains(index))
        .filter(|(_, candidate)| {
            candidate.kind != WorkItemKind::Issue
                || !candidate
                    .issue_id
                    .as_deref()
                    .is_some_and(|issue_id| spoken_for.contains(issue_id))
        })
        .map(|(_, candidate)| candidate.row.clone())
        .collect()
}

/// What a branch row's Done button asks of git: nothing left in the tree, and
/// nothing left to push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchSync {
    /// Files with uncommitted changes in the checkout.
    pub uncommitted_files: u64,
    /// Commits the branch holds that its upstream does not. `None` = unknown.
    pub ahead: Option<u64>,
    /// The tracking branch, when the branch has one.
    pub upstream: Option<String>,
}

/// A branch is finishable once it is committed AND pushed: an unpushed commit
/// or an unsaved edit is work that only exists on this machine, and Done
/// archives the entry.
pub fn branch_can_finish(sync: &BranchSync) -> bool {
    sync.uncommitted_files == 0 && sync.upstream.is_some() && sync.ahead == Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn branch_candidate(
        project_id: &str,
        branch: &str,
        source: BranchSource,
        label: &str,
    ) -> WorkItemCandidate {
        WorkItemCandidate {
            kind: WorkItemKind::Branch,
            key: WorkItemKey::Branch {
                project_id: project_id.to_string(),
                branch: branch.to_string(),
            },
            source: Some(source),
            issue_id: None,
            implementation_active: false,
            row: json!({ "branch": branch, "from": label }),
        }
    }

    fn issue_candidate(issue_id: &str) -> WorkItemCandidate {
        WorkItemCandidate {
            kind: WorkItemKind::Issue,
            key: WorkItemKey::Issue {
                issue_id: issue_id.to_string(),
            },
            source: None,
            issue_id: Some(issue_id.to_string()),
            implementation_active: false,
            row: json!({ "issue_id": issue_id }),
        }
    }

    fn labels(rows: &[Value]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                row["from"]
                    .as_str()
                    .or_else(|| row["issue_id"].as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// A run knows the branch's lifecycle; the primary-checkout summary and the
    /// external scan only know what git sees. One branch, one row, and it is
    /// the run's.
    #[test]
    fn a_run_wins_the_branch_it_shares_with_a_checkout_scan() {
        let folded = fold_work_items(vec![
            branch_candidate("p1", "main", BranchSource::PrimaryCheckout, "primary"),
            branch_candidate("p1", "main", BranchSource::Run, "run"),
            branch_candidate("p1", "feature", BranchSource::ExternalWorktree, "external"),
            branch_candidate("p1", "feature", BranchSource::Run, "run-feature"),
        ]);
        assert_eq!(labels(&folded), vec!["run", "run-feature"]);
    }

    /// The primary checkout beats a bare external worktree, and every distinct
    /// key keeps its own row — including two projects on the same branch name
    /// and a detached checkout with no name to fold on.
    #[test]
    fn distinct_keys_each_keep_a_row() {
        let detached = WorkItemCandidate {
            kind: WorkItemKind::Branch,
            key: WorkItemKey::Checkout {
                worktree_id: "wt-detached".to_string(),
            },
            source: Some(BranchSource::ExternalWorktree),
            issue_id: None,
            implementation_active: false,
            row: json!({ "from": "detached" }),
        };
        let folded = fold_work_items(vec![
            branch_candidate("p1", "main", BranchSource::ExternalWorktree, "external"),
            branch_candidate("p1", "main", BranchSource::PrimaryCheckout, "primary"),
            branch_candidate("p2", "main", BranchSource::PrimaryCheckout, "other-project"),
            detached,
        ]);
        assert_eq!(
            labels(&folded),
            vec!["primary", "other-project", "detached"],
            "the loser never reorders the winner"
        );
    }

    /// Once implementation starts, the issue's work surfaces as the branch row
    /// only — and that row is what carries the issue id.
    #[test]
    fn a_live_implementation_suppresses_its_issue_row() {
        let mut implementation =
            branch_candidate("p1", "build/thing", BranchSource::Run, "implementation");
        implementation.issue_id = Some("plan-1".to_string());
        implementation.implementation_active = true;

        let folded = fold_work_items(vec![
            issue_candidate("plan-1"),
            implementation,
            issue_candidate("plan-2"),
        ]);
        assert_eq!(labels(&folded), vec!["implementation", "plan-2"]);
    }

    /// A finished implementation stops speaking for its issue: the issue is a
    /// project-level work item again.
    #[test]
    fn a_terminal_implementation_leaves_the_issue_row_alone() {
        let mut implementation =
            branch_candidate("p1", "build/thing", BranchSource::Run, "implementation");
        implementation.issue_id = Some("plan-1".to_string());
        implementation.implementation_active = false;

        let folded = fold_work_items(vec![issue_candidate("plan-1"), implementation]);
        assert_eq!(labels(&folded), vec!["plan-1", "implementation"]);
    }

    /// A suppressing branch row that lost its key suppresses nothing: the row
    /// that won the key is the one that speaks for the issue.
    #[test]
    fn only_a_surviving_branch_row_suppresses_an_issue() {
        let mut shadowed =
            branch_candidate("p1", "build/thing", BranchSource::ExternalWorktree, "stale");
        shadowed.issue_id = Some("plan-1".to_string());
        shadowed.implementation_active = true;

        let folded = fold_work_items(vec![
            issue_candidate("plan-1"),
            shadowed,
            branch_candidate("p1", "build/thing", BranchSource::Run, "adopted"),
        ]);
        assert_eq!(labels(&folded), vec!["plan-1", "adopted"]);
    }

    #[test]
    fn a_branch_is_finishable_only_when_it_is_committed_and_pushed() {
        let pushed = BranchSync {
            uncommitted_files: 0,
            ahead: Some(0),
            upstream: Some("origin/feature".to_string()),
        };
        assert!(branch_can_finish(&pushed));

        assert!(
            !branch_can_finish(&BranchSync {
                uncommitted_files: 1,
                ..pushed.clone()
            }),
            "unsaved edits are work that exists nowhere else"
        );
        assert!(
            !branch_can_finish(&BranchSync {
                ahead: Some(2),
                ..pushed.clone()
            }),
            "unpushed commits are work that exists nowhere else"
        );
        assert!(
            !branch_can_finish(&BranchSync {
                upstream: None,
                ahead: None,
                ..pushed.clone()
            }),
            "a branch with no upstream has never been pushed"
        );
        assert!(
            !branch_can_finish(&BranchSync {
                ahead: None,
                ..pushed
            }),
            "an unknown ahead count is not a level branch"
        );
    }
}
