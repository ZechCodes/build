//! Branch and issue are the only work items.
//!
//! A branch work item is identified by `(project_id, branch_name)`. What the
//! bridge stores underneath — a run, an adopted worktree, an external worktree
//! Build never cut — is an implementation detail, and more than one of them can
//! describe the same branch at once. This module owns the two rules that turn
//! those sources into one feed:
//!
//! 1. **Folding.** Rows that share a key are the same work item seen twice; the
//!    source that knows the most about it wins (a run over a bare external
//!    worktree).
//! 2. **Dedup.** An issue whose implementation is still in flight speaks as
//!    that branch row alone — the branch row carries the `issue_id` and the
//!    issue's own row is suppressed.
//!
//! Both are pure functions over already-built rows, so the policy is testable
//! without a repo, a store, or an RPC.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// The kinds of work item, as they ship on the wire. Branch and issue are the
/// work; a capture is the thing the user said that has not become work yet, and
/// it holds a row of its own only until it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemKind {
    Branch,
    Issue,
    Capture,
}

impl WorkItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkItemKind::Branch => "branch",
            WorkItemKind::Issue => "issue",
            WorkItemKind::Capture => "capture",
        }
    }
}

/// What a branch row's facts were read off. The order of the variants is the
/// order they win in: a run knows the branch's lifecycle, conversation and
/// agents; a bare external worktree knows only what git can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BranchSource {
    Run,
    ExternalWorktree,
}

impl BranchSource {
    /// What this kind of holder is called on the wire, so a client picks the
    /// verb a held branch offers by reading a name rather than by re-deciding
    /// which of several stamps outranks the others.
    pub fn as_str(self) -> &'static str {
        match self {
            BranchSource::Run => "run",
            BranchSource::ExternalWorktree => "external_worktree",
        }
    }

    /// What this kind of holder is called, for a user being told which one has
    /// the branch they asked for.
    pub fn holder_noun(self) -> &'static str {
        match self {
            BranchSource::Run => "run",
            BranchSource::ExternalWorktree => "worktree",
        }
    }
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
    /// A capture is its own key: nothing else in the feed can be the same
    /// thing, because it is not yet a thing.
    Capture {
        capture_id: String,
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
        .map(|(_, candidate)| named_project_row(candidate.row.clone()))
        .collect()
}

/// Every row leaves the fold saying which project it belongs to: a row whose
/// builder could not resolve a display name falls back to the project id
/// rather than shipping "" — the inbox lists every project's work in one
/// list, so a row with no project reads as belonging to nothing.
fn named_project_row(mut row: Value) -> Value {
    let unnamed = row["project"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    if unnamed {
        if let Some(project_id) = row["project_id"].as_str().filter(|id| !id.is_empty()) {
            row["project"] = Value::String(project_id.to_string());
        }
    }
    row
}

/// What Done asks git about a branch before it deletes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchSync {
    /// Files with uncommitted changes in the checkout.
    pub uncommitted_files: u64,
    /// Commits the branch holds that [`comparison_ref`](Self::comparison_ref)
    /// does not. `None` = unknown.
    pub ahead: Option<u64>,
    /// The tracking branch, when the branch has one.
    pub upstream: Option<String>,
    /// The ref `ahead` is counted against: the upstream when there is one,
    /// otherwise the project's base branch. It is what makes the count
    /// readable — "3 unpushed" and "3 unmerged" are the same number against
    /// different refs, and Done says a different thing about each.
    pub comparison_ref: Option<String>,
}

/// One thing the user should know before Done destroys this work item.
///
/// A warning is not a refusal. Done deletes a branch and archives an issue on
/// the user's say-so; the bridge's job is to make what is about to be lost
/// legible BEFORE the destructive act, and then to do as it is told. Refusals
/// stay errors — an unknown branch, a git command that failed — because there
/// is nothing the user can confirm their way past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishWarning {
    /// Stable machine name, so a client can order or style them.
    pub code: &'static str,
    /// The whole warning in one sentence, ready to render.
    pub message: String,
    /// What the warning counts — commits, files — when it counts anything.
    pub count: Option<u64>,
    /// The ref the count is measured against, when there is one.
    pub reference: Option<String>,
}

/// Work in the tree that no commit holds: removing the checkout discards it.
pub const FINISH_WARNING_UNCOMMITTED: &str = "uncommitted";
/// The branch tracks a remote, and the remote does not have all of it.
pub const FINISH_WARNING_UNPUSHED: &str = "unpushed";
/// The branch tracks nothing, so the base branch is the only place its work
/// could survive — and it is not all there.
pub const FINISH_WARNING_UNMERGED: &str = "unmerged";
/// An issue no branch ever implemented.
pub const FINISH_WARNING_UNIMPLEMENTED: &str = "unimplemented";

impl FinishWarning {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "count": self.count,
            "ref": self.reference,
        })
    }
}

/// Render a set of warnings for the wire.
pub fn warnings_json(warnings: &[FinishWarning]) -> Value {
    Value::Array(warnings.iter().map(FinishWarning::to_json).collect())
}

fn plural(count: u64, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// What Done on this branch is about to lose.
///
/// Where the work could survive depends on whether anyone else has a copy of
/// the branch: with an upstream, the remote is that copy and the question is
/// whether it is current; with none, the base branch is the only place the
/// commits can outlive the branch, and the question is whether they landed
/// there. An unknown count answers neither question, so it warns.
pub fn branch_finish_warnings(branch: &str, sync: &BranchSync) -> Vec<FinishWarning> {
    let mut warnings = Vec::new();
    if sync.uncommitted_files > 0 {
        warnings.push(FinishWarning {
            code: FINISH_WARNING_UNCOMMITTED,
            message: format!(
                "{branch} has {} that no commit holds — removing the checkout discards them",
                plural(sync.uncommitted_files, "uncommitted file")
            ),
            count: Some(sync.uncommitted_files),
            reference: None,
        });
    }
    if sync.ahead == Some(0) {
        return warnings;
    }
    match &sync.upstream {
        Some(upstream) => warnings.push(FinishWarning {
            code: FINISH_WARNING_UNPUSHED,
            message: match sync.ahead {
                Some(ahead) => format!(
                    "{branch} has {} that {upstream} does not",
                    plural(ahead, "commit")
                ),
                None => format!("{branch} could not be compared with {upstream}"),
            },
            count: sync.ahead,
            reference: Some(upstream.clone()),
        }),
        None => warnings.push(FinishWarning {
            code: FINISH_WARNING_UNMERGED,
            message: match (sync.ahead, sync.comparison_ref.as_deref()) {
                (Some(ahead), Some(base)) => format!(
                    "{branch} has never been pushed, and has {} that {base} does not",
                    plural(ahead, "commit")
                ),
                (Some(ahead), None) => format!(
                    "{branch} has never been pushed, and has {} the base branch does not",
                    plural(ahead, "commit")
                ),
                (None, base) => format!(
                    "{branch} has never been pushed, and could not be compared with {}",
                    base.unwrap_or("the base branch")
                ),
            },
            count: sync.ahead,
            reference: sync.comparison_ref.clone(),
        }),
    }
    warnings
}

/// What Done on this issue is about to lose: an issue nothing was ever built
/// for is being filed away on the strength of the conversation alone.
pub fn issue_finish_warnings(implemented: bool) -> Vec<FinishWarning> {
    if implemented {
        return Vec::new();
    }
    vec![FinishWarning {
        code: FINISH_WARNING_UNIMPLEMENTED,
        message: "No branch has implemented this issue".to_string(),
        count: None,
        reference: None,
    }]
}

/// Put the feed in the order the inbox reads it: by anchor, oldest first.
///
/// Oldest at the top is the whole shape of the list — what you took on longest
/// ago is what you have been ignoring longest, and a fresh pickup appends to
/// the bottom rather than shoving everything down. A row with no anchor at all
/// (a checkout whose history could not be read) sorts last: unknown age is not
/// evidence of being old.
///
/// Stable, so rows that share an anchor keep the order the fold gave them.
pub fn sort_by_anchor(rows: &mut [Value]) {
    rows.sort_by(|left, right| {
        let key = |row: &Value| {
            row["anchor"]
                .as_str()
                .map(str::to_string)
                .map_or((true, String::new()), |anchor| (false, anchor))
        };
        key(left).cmp(&key(right))
    });
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

    /// A run knows the branch's lifecycle; the external scan only knows what
    /// git sees. One branch, one row, and it is the run's.
    #[test]
    fn a_run_wins_the_branch_it_shares_with_a_checkout_scan() {
        let folded = fold_work_items(vec![
            branch_candidate(
                "p1",
                "main",
                BranchSource::ExternalWorktree,
                "external-main",
            ),
            branch_candidate("p1", "main", BranchSource::Run, "run"),
            branch_candidate("p1", "feature", BranchSource::ExternalWorktree, "external"),
            branch_candidate("p1", "feature", BranchSource::Run, "run-feature"),
        ]);
        assert_eq!(labels(&folded), vec!["run", "run-feature"]);
    }

    /// Every distinct key keeps its own row — including two projects on the
    /// same branch name and a detached checkout with no name to fold on.
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
            branch_candidate("p1", "main", BranchSource::Run, "run"),
            branch_candidate(
                "p2",
                "main",
                BranchSource::ExternalWorktree,
                "other-project",
            ),
            detached,
        ]);
        assert_eq!(
            labels(&folded),
            vec!["run", "other-project", "detached"],
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

    /// A capture row is nobody else's duplicate: it keeps its place in the
    /// feed, suppresses nothing, and is suppressed by nothing.
    #[test]
    fn a_capture_row_folds_with_nothing() {
        let capture = WorkItemCandidate {
            kind: WorkItemKind::Capture,
            key: WorkItemKey::Capture {
                capture_id: "capture-1".to_string(),
            },
            source: None,
            issue_id: None,
            implementation_active: false,
            row: json!({ "from": "capture" }),
        };
        let mut implementation =
            branch_candidate("p1", "build/thing", BranchSource::Run, "implementation");
        implementation.issue_id = Some("plan-1".to_string());
        implementation.implementation_active = true;

        let folded = fold_work_items(vec![
            capture,
            issue_candidate("plan-1"),
            implementation,
            issue_candidate("plan-2"),
        ]);
        assert_eq!(labels(&folded), vec!["capture", "implementation", "plan-2"]);
    }

    /// Oldest first, so the top of the inbox is what has been waiting longest,
    /// and a row nobody can date sorts under the ones somebody can.
    #[test]
    fn the_feed_reads_oldest_first_with_undatable_rows_last() {
        let row = |label: &str, anchor: Value| json!({ "from": label, "anchor": anchor });
        let mut rows = vec![
            row("undatable", Value::Null),
            row("yesterday", json!("2026-08-14T09:00:00Z")),
            row("last-week", json!("2026-08-07T09:00:00Z")),
            row("also-last-week", json!("2026-08-07T09:00:00Z")),
        ];
        sort_by_anchor(&mut rows);
        assert_eq!(
            labels(&rows),
            vec!["last-week", "also-last-week", "yesterday", "undatable"],
            "ties keep the order the fold gave them"
        );
    }

    fn codes(warnings: &[FinishWarning]) -> Vec<&str> {
        warnings.iter().map(|warning| warning.code).collect()
    }

    /// A branch with somewhere else to be warns about nothing; the same branch
    /// with commits its remote does not have warns about exactly those.
    #[test]
    fn a_pushed_branch_warns_about_nothing_and_an_unpushed_one_counts_its_commits() {
        let pushed = BranchSync {
            uncommitted_files: 0,
            ahead: Some(0),
            upstream: Some("origin/feature".to_string()),
            comparison_ref: Some("origin/feature".to_string()),
        };
        assert!(branch_finish_warnings("feature", &pushed).is_empty());

        let unpushed = BranchSync {
            ahead: Some(2),
            ..pushed.clone()
        };
        let warnings = branch_finish_warnings("feature", &unpushed);
        assert_eq!(codes(&warnings), vec![FINISH_WARNING_UNPUSHED]);
        assert_eq!(warnings[0].count, Some(2));
        assert_eq!(warnings[0].reference.as_deref(), Some("origin/feature"));
        assert!(
            warnings[0].message.contains("2 commits")
                && warnings[0].message.contains("origin/feature"),
            "{:?}",
            warnings[0]
        );

        let unknown = BranchSync {
            ahead: None,
            ..pushed
        };
        assert_eq!(
            codes(&branch_finish_warnings("feature", &unknown)),
            vec![FINISH_WARNING_UNPUSHED],
            "a count nobody could read is not proof the remote has the work"
        );
    }

    #[test]
    fn a_row_that_could_not_name_its_project_says_the_id_instead_of_nothing() {
        let mut unnamed =
            branch_candidate("proj-1", "main", BranchSource::ExternalWorktree, "primary");
        unnamed.row["project"] = json!("");
        unnamed.row["project_id"] = json!("proj-1");
        let mut named = branch_candidate(
            "proj-2",
            "main",
            BranchSource::ExternalWorktree,
            "primary-2",
        );
        named.row["project"] = json!("Build");
        named.row["project_id"] = json!("proj-2");
        let rows = fold_work_items(vec![unnamed, named]);
        assert_eq!(rows[0]["project"], "proj-1", "{rows:?}");
        assert_eq!(rows[1]["project"], "Build", "{rows:?}");
    }

    /// With no upstream the base branch is the only place the work can outlive
    /// the branch, so the warning is about merging, not pushing.
    #[test]
    fn a_branch_with_no_upstream_warns_about_what_the_base_branch_lacks() {
        let unmerged = BranchSync {
            uncommitted_files: 0,
            ahead: Some(3),
            upstream: None,
            comparison_ref: Some("main".to_string()),
        };
        let warnings = branch_finish_warnings("feature", &unmerged);
        assert_eq!(codes(&warnings), vec![FINISH_WARNING_UNMERGED]);
        assert_eq!(warnings[0].count, Some(3));
        assert_eq!(warnings[0].reference.as_deref(), Some("main"));
        assert!(
            warnings[0].message.contains("3 commits") && warnings[0].message.contains("main"),
            "{:?}",
            warnings[0]
        );

        assert!(
            branch_finish_warnings(
                "feature",
                &BranchSync {
                    ahead: Some(0),
                    ..unmerged.clone()
                }
            )
            .is_empty(),
            "everything the branch holds is already in the base branch"
        );
        assert_eq!(
            codes(&branch_finish_warnings(
                "feature",
                &BranchSync {
                    ahead: None,
                    comparison_ref: None,
                    ..unmerged
                }
            )),
            vec![FINISH_WARNING_UNMERGED],
            "an unreadable comparison is not evidence the work landed"
        );
    }

    /// Uncommitted work is its own warning, and it stacks with the other one:
    /// two different things are about to be lost.
    #[test]
    fn uncommitted_work_warns_alongside_the_branch_comparison() {
        let warnings = branch_finish_warnings(
            "feature",
            &BranchSync {
                uncommitted_files: 1,
                ahead: Some(1),
                upstream: None,
                comparison_ref: Some("main".to_string()),
            },
        );
        assert_eq!(
            codes(&warnings),
            vec![FINISH_WARNING_UNCOMMITTED, FINISH_WARNING_UNMERGED]
        );
        assert!(
            warnings[0].message.contains("1 uncommitted file"),
            "{:?}",
            warnings[0]
        );
    }

    /// Done on an issue is only ever a warning about provenance: nothing was
    /// built for it.
    #[test]
    fn an_issue_warns_only_when_no_branch_ever_implemented_it() {
        assert!(issue_finish_warnings(true).is_empty());
        assert_eq!(
            codes(&issue_finish_warnings(false)),
            vec![FINISH_WARNING_UNIMPLEMENTED]
        );
    }

    #[test]
    fn a_warning_ships_its_code_message_count_and_ref() {
        let warnings = branch_finish_warnings(
            "feature",
            &BranchSync {
                uncommitted_files: 0,
                ahead: Some(2),
                upstream: Some("origin/feature".to_string()),
                comparison_ref: Some("origin/feature".to_string()),
            },
        );
        let wire = warnings_json(&warnings);
        assert_eq!(wire[0]["code"], "unpushed", "{wire:?}");
        assert_eq!(wire[0]["count"], 2, "{wire:?}");
        assert_eq!(wire[0]["ref"], "origin/feature", "{wire:?}");
        assert!(wire[0]["message"].is_string(), "{wire:?}");
    }
}
