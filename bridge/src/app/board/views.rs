use crate::app::{run_state_str, DigestScope, ExternalWorktreeRows, PRIMARY_SUMMARY_TTL};
use crate::run::RunState;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use std::collections::HashMap;

use serde_json::{json, Value};

use super::super::AppState;

/// One inbox row that no entity stands behind: a project's primary checkout, or
/// a branch checked out somewhere Build never cut.
///
/// It has no entity id, so it is identified by what it is, and no conversation,
/// so the line a dismissal draws is the commit it is sitting on.
pub(in crate::app) struct EntitylessRow {
    /// Where the dismissal is written in the attention map.
    pub(in crate::app) key: String,
    /// The commit the row is on, as the feed reads it. `None` when its history
    /// cannot be read at all.
    pub(in crate::app) head: Option<String>,
    pub(in crate::app) project_id: String,
    /// The branch the row shows, or `None` for a detached checkout.
    pub(in crate::app) branch: Option<String>,
    /// Whether this is the project's own checkout — the repository itself.
    pub(in crate::app) primary: bool,
}

/// The +/− block every work-item row carries, in one shape whatever source it
/// was read off, plus the facts Done warns about (see
/// [`crate::branch::branch_finish_warnings`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct WorkItemStat {
    /// Everything the branch carries against its base.
    pub(in crate::app) files_changed: u64,
    pub(in crate::app) insertions: u64,
    pub(in crate::app) deletions: u64,
    /// What is sitting in the tree unsaved.
    pub(in crate::app) uncommitted_files: u64,
    pub(in crate::app) uncommitted_insertions: u64,
    pub(in crate::app) uncommitted_deletions: u64,
    pub(in crate::app) ahead: Option<u64>,
    pub(in crate::app) behind: Option<u64>,
    pub(in crate::app) upstream: Option<String>,
    /// The one ref both counts are measured against: the tracking branch when
    /// the branch has one, otherwise the project's base branch. It is what
    /// makes ahead/behind readable — "3 unpushed" and "3 unmerged" are the same
    /// number against different refs, and Done warns differently about each.
    pub(in crate::app) comparison_ref: Option<String>,
    /// When this branch last got a commit (RFC 3339), or `None` when the
    /// checkout could not be read.
    pub(in crate::app) head_committed_at: Option<String>,
}

/// The `state` a branch row reports when nothing is driving it: a checkout
/// exists on that branch, and no run owns its lifecycle.
pub(in crate::app) const CHECKOUT_IDLE_STATE: &str = "idle";

/// How long the turn in flight has been running, for the toolbar's clock.
/// `since` is what a ticking client re-reads; `seconds` is the same fact
/// resolved against the bridge's clock, so a client with a skewed one still
/// agrees. Null when nothing is being worked.
pub(in crate::app) fn working_time_json(since: Option<&str>) -> Value {
    let Some(since) = since else {
        return Value::Null;
    };
    json!({ "since": since, "seconds": seconds_since(since) })
}

/// Whole seconds between an RFC 3339 timestamp and now, never negative.
/// `None` when the timestamp cannot be parsed.
pub(in crate::app) fn seconds_since(started_at: &str) -> Option<u64> {
    let started =
        time::OffsetDateTime::parse(started_at, &time::format_description::well_known::Rfc3339)
            .ok()?;
    Some(
        (time::OffsetDateTime::now_utc() - started)
            .whole_seconds()
            .max(0) as u64,
    )
}

impl WorkItemStat {
    /// Read off a run's cached diffstat ([`AppState::run_stat`]). A terminal
    /// run reports a null stat, which reads as an empty one.
    pub(in crate::app) fn from_run_stat(stat: &Value) -> Self {
        Self {
            files_changed: stat["files_changed"].as_u64().unwrap_or(0),
            insertions: stat["insertions"].as_u64().unwrap_or(0),
            deletions: stat["deletions"].as_u64().unwrap_or(0),
            uncommitted_files: stat["uncommitted"]["files_changed"].as_u64().unwrap_or(0),
            uncommitted_insertions: stat["uncommitted"]["insertions"].as_u64().unwrap_or(0),
            uncommitted_deletions: stat["uncommitted"]["deletions"].as_u64().unwrap_or(0),
            ahead: stat["ahead"].as_u64(),
            behind: stat["behind"].as_u64(),
            upstream: stat["upstream"].as_str().map(str::to_string),
            comparison_ref: stat["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: stat["head_committed_at"].as_str().map(str::to_string),
        }
    }

    /// Read off one entry of the external-worktree scan. `dirty_files` is the
    /// status count, so it sees untracked files the diff cannot.
    pub(in crate::app) fn from_external_entry(entry: &Value) -> Self {
        Self {
            files_changed: entry["diffstat"]["files_changed"].as_u64().unwrap_or(0),
            insertions: entry["diffstat"]["insertions"].as_u64().unwrap_or(0),
            deletions: entry["diffstat"]["deletions"].as_u64().unwrap_or(0),
            uncommitted_files: entry["dirty_files"].as_u64().unwrap_or(0),
            uncommitted_insertions: entry["uncommitted"]["insertions"].as_u64().unwrap_or(0),
            uncommitted_deletions: entry["uncommitted"]["deletions"].as_u64().unwrap_or(0),
            ahead: entry["ahead"].as_u64(),
            behind: entry["behind"].as_u64(),
            upstream: entry["upstream"].as_str().map(str::to_string),
            comparison_ref: entry["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: entry["head_committed_at"].as_str().map(str::to_string),
        }
    }

    /// Read off one entry of the primary-changes summary, whose counts are the
    /// working tree against HEAD — uncommitted work, and all a checkout with no
    /// base to compare against can honestly report.
    pub(in crate::app) fn from_primary_entry(entry: &Value) -> Self {
        let files_changed = entry["files_changed"].as_u64().unwrap_or(0);
        let insertions = entry["insertions"].as_u64().unwrap_or(0);
        let deletions = entry["deletions"].as_u64().unwrap_or(0);
        Self {
            files_changed,
            insertions,
            deletions,
            uncommitted_files: files_changed,
            uncommitted_insertions: insertions,
            uncommitted_deletions: deletions,
            ahead: entry["ahead"].as_u64(),
            behind: entry["behind"].as_u64(),
            upstream: entry["upstream"].as_str().map(str::to_string),
            comparison_ref: entry["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: entry["head_committed_at"].as_str().map(str::to_string),
        }
    }

    pub(in crate::app) fn sync(&self) -> crate::branch::BranchSync {
        crate::branch::BranchSync {
            uncommitted_files: self.uncommitted_files,
            ahead: self.ahead,
            upstream: self.upstream.clone(),
            comparison_ref: self.comparison_ref.clone(),
        }
    }

    /// What Done on this branch is about to lose, ready for the wire.
    pub(in crate::app) fn finish_warnings_json(&self, branch: &str) -> Value {
        crate::branch::warnings_json(&crate::branch::branch_finish_warnings(branch, &self.sync()))
    }

    pub(in crate::app) fn to_json(&self) -> Value {
        json!({
            "files_changed": self.files_changed,
            "insertions": self.insertions,
            "deletions": self.deletions,
            "uncommitted": {
                "files_changed": self.uncommitted_files,
                "insertions": self.uncommitted_insertions,
                "deletions": self.uncommitted_deletions,
            },
            "ahead": self.ahead,
            "behind": self.behind,
            "upstream": self.upstream,
            "comparison_ref": self.comparison_ref,
        })
    }
}

impl AppState {
    /// When somebody last spoke on this work item, extended through the end of
    /// its most recent in-flight turn. Git, files, tools and terminal output do
    /// not make an inbox entry recent.
    ///
    /// Every input is already in hand — no clock here starts new git work.
    pub(in crate::app) fn last_activity_of(
        &self,
        entity_id: Option<&str>,
        conversation: Option<&crate::thread::Thread>,
    ) -> Option<String> {
        if let Some(id) = entity_id {
            if let Ok(roster) = self.entity_agents(id) {
                let conversation_at = roster
                    .iter()
                    .filter_map(|agent| {
                        let thread = self
                            .agent_conversation(id, Some(&agent.id))
                            .unwrap_or(&agent.thread);
                        thread.conversation_activity_at()
                    })
                    .max()
                    .map(str::to_string);
                let worked_at = self
                    .board
                    .attention()
                    .attention(id)
                    .and_then(|attention| attention.last_worked_at.clone());
                let first_observed_at = self
                    .board
                    .attention()
                    .attention(id)
                    .and_then(|attention| attention.first_observed_at.clone());
                return conversation_at
                    .into_iter()
                    .chain(worked_at)
                    .max()
                    .or(first_observed_at)
                    .or_else(|| Some(self.anchor_of(id)));
            }
        }
        conversation
            .and_then(crate::thread::Thread::conversation_activity_at)
            .map(str::to_string)
    }

    /// Every project's external worktrees, ride-along shape for `task.list`
    /// (spec §5.3): scan order per project, projects concatenated in
    /// registration order. A per-project scan failure is already logged inside
    /// `external_worktrees`; it just contributes nothing here.
    pub(in crate::app) fn external_worktrees_json(&mut self) -> ExternalWorktreeRows {
        // Resolved up front: the loop below holds a &mut borrow of the scan
        // cache, and attention_json needs &self.
        let attention_of: std::collections::HashMap<String, Value> = self
            .board
            .attention()
            .attention_ids()
            .map(|id| (id.to_string(), self.attention_json(id)))
            .collect();
        // Read off the tab registry, which is where an agent can be — there is
        // no longer anywhere else for one to run. The worktree id is derived
        // from the tab's root, so a worktree reports its own agent whatever
        // entity (or none) currently owns it.
        let agent_signals: HashMap<String, (bool, bool)> = self
            .session_registry
            .agent_working_roots()
            .into_iter()
            .map(|(root, working)| {
                (
                    crate::worktree::external_worktree_id(&root),
                    (working, !working),
                )
            })
            .collect();
        let projects: Vec<(String, String, String)> = self
            .projects
            .iter()
            .filter(|project| project.is_git)
            .map(|p| (p.id.clone(), p.name.clone(), p.base_branch.clone()))
            .collect();
        let mut rows = ExternalWorktreeRows::default();
        for (project_id, project_name, base_branch) in projects {
            let scan = self.external_worktrees(&project_id);
            rows.scanning |= !scan.settled;
            for w in scan.worktrees {
                let adoptable = w.branch.as_deref().is_some_and(|b| b != base_branch);
                let (agent_working, can_finish) =
                    agent_signals.get(&w.id).copied().unwrap_or((false, false));
                rows.rows.push(json!({
                    "worktree_id": w.id,
                    "project_id": project_id,
                    "project": project_name,
                    "path": w.path.display().to_string(),
                    "isolation": w.isolation.wire(),
                    "branch": w.branch,
                    "head_sha": w.head_sha,
                    "head_subject": w.head_subject,
                    "head_age_seconds": w.head_age_seconds,
                    "head_committed_at": w.head_committed_at,
                    "dirty_files": w.dirty_files,
                    // Ahead and behind always share one comparison ref. The
                    // working-tree delta is reported separately below.
                    "comparison_ref": w.comparison_ref,
                    "ahead": w.ahead,
                    "behind": w.behind,
                    "base_branch": base_branch,
                    "unpushed": w.unpushed,
                    "upstream": w.upstream,
                    "diffstat": w.diffstat.to_json(),
                    // What is sitting in the tree unsaved — the rail's +/−.
                    "uncommitted": w.uncommitted.to_json(),
                    "adoptable": adoptable,
                    "agent_working": agent_working,
                    "can_finish": can_finish,
                    // A worktree Build cut carries attention from birth, so it
                    // surfaces in the rail as something waiting for you. One made
                    // outside Build has none until you act on it here, and stays
                    // in the Worktrees row until then.
                    "attention": attention_of.get(&w.id).cloned().unwrap_or_else(|| json!({
                        "resume_at": Value::Null,
                        "interacted": false,
                        "seen": false,
                    })),
                }));
            }
        }
        rows
    }

    /// Every project's primary-checkout changes summary, held per project for
    /// [`PRIMARY_SUMMARY_TTL`] (spec §5.3) and then served stale while it
    /// refreshes — the `task.list` ride-along for the sidebar "main" row and the
    /// project page's MAIN bucket. A per-project failure (unborn HEAD, fs error)
    /// logs and contributes nothing, same posture as `external_worktrees_json`.
    pub(in crate::app) fn primary_changes_json(&mut self) -> Vec<Value> {
        // Who owns each primary checkout, so the main row can route to its run
        // after a reload. Resolved up front: the loop below holds a &mut borrow
        // of the summary cache.
        let owners: HashMap<String, String> = self
            .projects
            .iter()
            .filter_map(|project| {
                self.primary_run_of(&project.id)
                    .map(|run_id| (project.id.clone(), run_id))
            })
            .collect();
        // Ownership changes on its own schedule, so it is stamped onto the
        // outgoing entry rather than into the cached git summary.
        let with_owner = |mut entry: Value, project_id: &str| {
            entry["run_id"] = owners
                .get(project_id)
                .cloned()
                .map_or(Value::Null, Value::String);
            entry
        };

        let project_ids: Vec<String> = self
            .projects
            .iter()
            .filter(|project| project.is_git)
            .map(|project| project.id.clone())
            .collect();
        project_ids
            .into_iter()
            .filter_map(|project_id| {
                let summary = self.primary_summary(&project_id)?;
                Some(with_owner(summary, &project_id))
            })
            .collect()
    }

    /// One project's primary-checkout summary as the last walk left it, or
    /// `None` until the first one lands. Claims the walk it needs; never takes
    /// one itself.
    pub(in crate::app) fn primary_summary(&mut self, project_id: &str) -> Option<Value> {
        if let Some(refresh) = self.primary_summary_refresh(project_id) {
            let computed_at = self.primary_summary_of(project_id).map(|(at, _)| at);
            self.refresh_if_stale(computed_at, PRIMARY_SUMMARY_TTL, refresh);
        }
        self.primary_summary_of(project_id)
            .map(|(_, summary)| summary.clone())
    }

    // ---- Board + views --------------------------------------------------------

    /// The board: workspace branches and runs (each run carries a live
    /// diffstat), plus the ride-along external-worktree and primary-changes
    /// summaries. Legacy issues remain available through their direct read
    /// APIs, but no longer participate in this active-work surface.
    pub(in crate::app) fn board_list(&mut self) -> Value {
        self.sweep_vanished_runs();
        let runs: Vec<Value> = {
            let ids: Vec<String> = self
                .runs
                .iter()
                .filter(|(_, active)| active.run.state != RunState::Archived)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .map(|id| {
                    let stat = self.run_stat(&id).unwrap_or(Value::Null);
                    let active = self.runs.get(&id).expect("listed above");
                    let mut view =
                        self.run_view(&id, active, ThreadDetail::Digest, DigestScope::List);
                    view.as_object_mut()
                        .expect("run_view returns an object")
                        .insert("stat".to_string(), stat);
                    view
                })
                .collect()
        };
        let checkouts = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        let projects = self
            .projects
            .iter()
            .map(|project| {
                json!({
                    "project_id": project.id,
                    "name": project.name,
                    "path": project.repo_path.display().to_string(),
                    "base_branch": project.base_branch,
                    "is_git": project.is_git,
                })
            })
            .collect::<Vec<_>>();
        // The inbox is in-flight work the user started in Build, nothing else:
        // a branch Build never cut or adopted (no run behind it, and it is not
        // the project's own primary checkout) earns no row here, ever — not
        // while an agent happens to be active in it, not on a fresh commit.
        // Adopting it (or sending it a first message, which adopts on the way)
        // is what brings it in; from there its row leaves the same way every
        // other row does — Done, deleted, or dismissed — never on its own.
        // `branch.get` still resolves it directly (deep-linking); this filter
        // is the feed list's alone.
        let items: Vec<Value> = self
            .work_items(&checkouts.rows, &primary_changes)
            .into_iter()
            .filter(|row| {
                row["kind"] != crate::branch::WorkItemKind::Branch.as_str()
                    || !row["run_id"].is_null()
                    || row["primary"] == json!(true)
            })
            .collect();
        json!({
            // The active feed contains workspace-backed work only. Legacy
            // issue records are intentionally absent from both the folded
            // items and the compatibility collections.
            "items": items,
            "projects": projects,
            "runs": runs,
            "external_worktrees": checkouts.rows,
            // Lifecycle verbs whose git is running right now. A checkout being
            // cut is on the board from the moment it is asked for, under the id
            // it will settle as.
            "pending": self.pending_rows_json(),
            // The rail has not finished looking. An empty list under this flag
            // is a board still working, not a project with no checkouts, and
            // the scan that lands invalidates the board so the client asks
            // again.
            "scanning": checkouts.scanning,
            "primary_changes": primary_changes,
        })
    }

    /// The feed's work items: one row per branch or issue.
    ///
    /// Branch rows fold the four ways a branch can be stored — a run, an
    /// adopted worktree, a worktree Build never cut, the primary checkout —
    /// into one shape keyed `(project_id, branch)`, and the primary checkout is
    /// the `main` row. An issue whose implementation is still in flight is
    /// spoken for by that implementation's branch row and emits none of its
    /// own. Both rules live in [`crate::branch`].
    ///
    /// Every branch a worktree names resolves here, whether or not it belongs
    /// on the INBOX list — `branch.get` (deep-linking) and `board_list`'s
    /// `items` both read this, and only the latter additionally filters out a
    /// worktree Build never adopted with no agent presently in it (see
    /// `board_list`). This function stays the unfiltered source of truth for
    /// "what branch is this," not "what does the inbox show."
    ///
    /// The scans are passed in rather than taken again: `board_list` already
    /// paid for them, and re-running them here would double every poll's git
    /// work.
    pub(in crate::app) fn work_items(
        &mut self,
        external_worktrees: &[Value],
        primary_changes: &[Value],
    ) -> Vec<Value> {
        self.observe_conversationless_rows(external_worktrees, primary_changes);
        self.reconcile_crossed_dismissal_lines();
        let run_ids: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, active)| active.run.state != RunState::Archived)
            .map(|(id, _)| id.clone())
            .collect();
        // Diffstats first: they are the one part of a row that needs `&mut`.
        let stats: HashMap<String, Value> = run_ids
            .iter()
            .filter_map(|run_id| Some((run_id.clone(), self.run_stat(run_id)?)))
            .collect();
        let mut candidates: Vec<crate::branch::WorkItemCandidate> = run_ids
            .iter()
            .map(|run_id| {
                self.branch_candidate_from_run(run_id, stats.get(run_id).unwrap_or(&Value::Null))
            })
            .collect();
        candidates.extend(
            primary_changes
                .iter()
                .filter_map(|entry| self.branch_candidate_from_primary(entry)),
        );
        candidates.extend(
            external_worktrees
                .iter()
                .map(|entry| self.branch_candidate_from_external(entry)),
        );
        candidates.extend(self.capture_candidates());
        let mut items = crate::branch::fold_work_items(candidates);
        // The inbox reads oldest first, and the order it reads in is decided
        // here rather than by every client that renders it.
        crate::branch::sort_by_anchor(&mut items);
        items
    }

    pub(in crate::app) fn reconcile_crossed_dismissal_lines(&mut self) {
        let entity_ids: Vec<String> = self.plans.keys().chain(self.runs.keys()).cloned().collect();
        let crossed: Vec<String> = entity_ids
            .into_iter()
            .filter(|id| {
                let Some(attention) = self.board.attention().attention(id) else {
                    return false;
                };
                attention.dismissal_tracks_messages
                    && self.dismissal_lines(id).iter().enumerate().any(
                        |(position, (agent_id, sequence))| match attention
                            .dismissed_line_for(agent_id, position == 0)
                        {
                            Some(line) => *sequence > line,
                            None => *sequence > 0 && attention.has_message_dismissal(),
                        },
                    )
            })
            .collect();
        if crossed.is_empty() {
            return;
        }
        for id in crossed {
            self.board.attention_mut().invalidate_dismissal(&id);
        }
        self.persist_attention();
    }

    pub(in crate::app) fn observe_conversationless_rows(
        &mut self,
        external_worktrees: &[Value],
        primary_changes: &[Value],
    ) {
        let now = now_rfc3339();
        let primary_keys = primary_changes.iter().filter_map(|entry| {
            entry["project_id"]
                .as_str()
                .map(crate::attention::primary_row_key)
        });
        let external_keys = external_worktrees.iter().filter_map(|entry| {
            let project_id = entry["project_id"].as_str()?;
            Some(match entry["branch"].as_str() {
                Some(branch) => crate::attention::branch_row_key(project_id, branch),
                None => entry["worktree_id"].as_str()?.to_string(),
            })
        });
        let mut changed = false;
        for key in primary_keys.chain(external_keys) {
            changed |= self.board.attention_mut().observe_row(&key, &now);
        }
        if changed {
            self.persist_attention();
        }
    }

    /// The branch row for a run: the source that knows the most, because it is
    /// the only one that carries a lifecycle, a conversation and agents.
    pub(in crate::app) fn branch_candidate_from_run(
        &self,
        run_id: &str,
        stat: &Value,
    ) -> crate::branch::WorkItemCandidate {
        let active = self.runs.get(run_id).expect("caller listed this run");
        let branch = active.worktree.branch();
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let sync = WorkItemStat::from_run_stat(stat);
        let thread = self.conversation_thread_for_run(active);
        let unread = self.unread_for(run_id, thread);
        let working = self.entity_agents_working(run_id);
        let working_since = working
            .then(|| self.working_since_for(run_id, thread))
            .flatten();
        let primary = self.owns_primary_checkout(run_id, active);
        let title = if active.run.goal.trim().is_empty() {
            branch.clone()
        } else {
            active.run.goal.clone()
        };
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": self.projects.project_id_of(run_id).unwrap_or_default(),
            "project": self.project_name_of(run_id),
            "branch": branch,
            "title": title,
            "state": run_state_str(&active.run.state),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            "working": working,
            "working_time": working_time_json(working_since.as_deref()),
            "agents": self.agent_digests(run_id, DigestScope::List),
            "stat": sync.to_json(),
            "resume_at": self.attention_json(run_id)["resume_at"],
            // Where this row sits in the inbox, and how long it has been quiet.
            // Every row carries both, whatever it was read off.
            "anchor": self.anchor_of(run_id),
            "last_activity": self.last_activity_of(Some(run_id), thread),
            // Done deletes the branch and its records. It is offered whenever
            // there is something to delete: the primary checkout is the
            // repository, so there is nothing to file away and everything to
            // lose. What the deletion would cost is `finish.warnings`, which
            // the client confirms through — never a refusal here.
            "can_finish": !primary,
            "finish": { "warnings": if primary {
                json!([])
            } else {
                sync.finish_warnings_json(&active.worktree.branch())
            } },
            "muted": self.is_muted(run_id),
            // Cleared out of the inbox until the work speaks again. The client
            // hides the row on it; nothing here changes because of it.
            "dismissed": self.is_dismissed(run_id),
            "worktree_path": active.worktree.path.display().to_string(),
            "worktree_id": crate::worktree::external_worktree_id(&Self::canonical_root(&active.worktree.path)),
            "run_id": run_id,
            "issue_id": issue_id,
            "primary": primary,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: crate::branch::WorkItemKey::Branch {
                project_id: self
                    .projects
                    .project_id_of(run_id)
                    .unwrap_or_default()
                    .to_string(),
                branch: active.worktree.branch(),
            },
            source: Some(crate::branch::BranchSource::Run),
            issue_id: active.run.plan_id.as_ref().map(|id| id.0.clone()),
            implementation_active: !active.run.state.is_terminal(),
            row,
        }
    }

    /// The branch row for a project's primary checkout — the `main` row.
    /// `None` when the checkout has no branch to name it by.
    pub(in crate::app) fn branch_candidate_from_primary(
        &self,
        entry: &Value,
    ) -> Option<crate::branch::WorkItemCandidate> {
        let project_id = entry["project_id"].as_str()?.to_string();
        let branch = entry["branch"].as_str()?.to_string();
        let project = self.project(&project_id)?;
        let repo_path = project.repo_path.display().to_string();
        let sync = WorkItemStat::from_primary_entry(entry);
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": project_id,
            "project": project.name,
            "branch": branch,
            "title": branch,
            "state": CHECKOUT_IDLE_STATE,
            "unread": false,
            "unread_count": 0,
            "unread_reason": Value::Null,
            "working": self.checkout_agent_working(&project.repo_path),
            "working_time": Value::Null,
            "agents": Vec::<Value>::new(),
            "stat": sync.to_json(),
            "resume_at": Value::Null,
            // A checkout with no run behind it has no record to anchor: it
            // dates itself by its own last commit, which is the only history it
            // has. Same for its last activity, plus whatever its agent painted.
            "anchor": sync.head_committed_at,
            "last_activity": self.board
                .attention()
                .attention(&crate::attention::primary_row_key(&project_id))
                .and_then(|attention| attention.first_observed_at.clone()),
            // The repository is not a worktree to file away.
            "can_finish": false,
            "finish": { "warnings": [] },
            "muted": false,
            // Cleared out of the inbox until the project has something new to
            // say. The repository holds no conversation to fall quiet, so the
            // line is drawn at the commit the checkout was cleared on.
            "dismissed": self.row_is_dismissed(
                &crate::attention::primary_row_key(&project_id),
                entry["head_sha"].as_str(),
            ),
            "worktree_path": repo_path,
            "worktree_id": Value::Null,
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "primary": true,
        });
        Some(crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: crate::branch::WorkItemKey::Branch { project_id, branch },
            source: Some(crate::branch::BranchSource::PrimaryCheckout),
            issue_id: None,
            implementation_active: false,
            row,
        })
    }

    /// The branch row for a worktree Build never cut: everything git can see
    /// about it, and nothing else — it has no run, so it has no agents and no
    /// conversation.
    pub(in crate::app) fn branch_candidate_from_external(
        &self,
        entry: &Value,
    ) -> crate::branch::WorkItemCandidate {
        let project_id = entry["project_id"].as_str().unwrap_or_default().to_string();
        let worktree_id = entry["worktree_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let branch = entry["branch"].as_str().map(str::to_string);
        let path = entry["path"].as_str().unwrap_or_default().to_string();
        let sync = WorkItemStat::from_external_entry(entry);
        let title = match (entry["head_subject"].as_str(), branch.as_deref()) {
            (Some(subject), _) if !subject.trim().is_empty() => subject.to_string(),
            (_, Some(branch)) => branch.to_string(),
            _ => path.clone(),
        };
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": project_id,
            "project": entry["project"],
            "branch": branch,
            "title": title,
            "state": CHECKOUT_IDLE_STATE,
            "unread": false,
            "unread_count": 0,
            "unread_reason": Value::Null,
            "working": entry["agent_working"].as_bool().unwrap_or(false),
            "working_time": Value::Null,
            "agents": Vec::<Value>::new(),
            "stat": sync.to_json(),
            "resume_at": entry["attention"]["resume_at"],
            // See the primary row: a bare checkout is dated by its own commits,
            // unless the user has acted on it here and given it an anchor.
            "anchor": self
                .board
                .attention()
                .attention(&worktree_id)
                .and_then(|attention| attention.anchor_at.clone())
                .or_else(|| sync.head_committed_at.clone()),
            "last_activity": self.board
                .attention()
                .attention(&match &branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.clone(),
                })
                .and_then(|attention| attention.first_observed_at.clone()),
            "can_finish": true,
            "finish": { "warnings": sync.finish_warnings_json(
                branch.as_deref().unwrap_or("this checkout"),
            ) },
            "muted": self.is_muted(&worktree_id),
            // A checkout has no conversation, so git changes cannot revive a
            // clear. Adoption turns it into a run that can speak for itself.
            "dismissed": self.row_is_dismissed(
                &match &branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.clone(),
                },
                entry["head_sha"].as_str(),
            ),
            "worktree_path": path,
            "worktree_id": worktree_id.clone(),
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "primary": false,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: match &branch {
                Some(branch) => crate::branch::WorkItemKey::Branch {
                    project_id,
                    branch: branch.clone(),
                },
                None => crate::branch::WorkItemKey::Checkout { worktree_id },
            },
            source: Some(crate::branch::BranchSource::ExternalWorktree),
            issue_id: None,
            implementation_active: false,
            row,
        }
    }

    pub(in crate::app) fn is_muted(&self, entity_id: &str) -> bool {
        self.board.attention().is_muted(entity_id)
    }

    /// Whether this row was cleared and no user or agent has spoken on any of
    /// its conversations since.
    ///
    /// Every agent has to still be cleared, which is the complement of the
    /// badge above it: `unread_for` unions unread across the roster, so a row
    /// is out of the list only while nothing anywhere on it has spoken past
    /// the line the human drew.
    ///
    pub(in crate::app) fn is_dismissed(&self, entity_id: &str) -> bool {
        let Some(attention) = self.board.attention().attention(entity_id) else {
            return false;
        };
        let lines = self.dismissal_lines(entity_id);
        // A row with no roster behind it holds no conversation to have been
        // cleared: it is dismissed at a commit instead, by `row_is_dismissed`.
        attention.has_message_dismissal()
            && (lines.is_empty()
                || lines.iter().enumerate().all(
                    |(position, (agent_id, latest_attention_sequence))| {
                        attention.is_dismissed_for(
                            agent_id,
                            position == 0,
                            *latest_attention_sequence,
                        ) || (*latest_attention_sequence == 0 && attention.has_message_dismissal())
                    },
                ))
    }

    /// When this work item's oldest turn still in flight started — how long the
    /// ITEM has been working, rather than how long its newest agent has.
    pub(in crate::app) fn working_since_for(
        &self,
        entity_id: &str,
        thread: Option<&crate::thread::Thread>,
    ) -> Option<String> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return thread
                .and_then(|thread| thread.working_since())
                .map(str::to_string);
        };
        roster
            .iter()
            .filter_map(|agent| agent.working_since.as_deref())
            .min()
            .map(str::to_string)
    }

    /// Whether a Build-owned agent is painting in this checkout right now. The
    /// tab registry is the only place an agent can be, and its key is the
    /// checkout root, so a checkout reports its own agent whatever entity (or
    /// none) currently owns it.
    pub(in crate::app) fn checkout_agent_working(&self, root: &std::path::Path) -> bool {
        let root = Self::canonical_root(root);
        self.session_registry.agent_is_working_at(&root)
    }

    pub(in crate::app) fn entity_agents_working(&self, entity_id: &str) -> bool {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster.iter().any(|agent| agent.working_since.is_some())
    }
}
