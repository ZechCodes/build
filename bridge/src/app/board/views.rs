use crate::app::{run_state_str, DigestScope, ExternalWorktreeRows, WORKSPACE_SUMMARY_TTL};
use crate::run::RunState;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use std::collections::HashMap;

use serde_json::{json, Value};

use super::super::AppState;

/// One inbox row that no entity stands behind: a branch checked out somewhere
/// Build never cut.
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

/// What the feed needs about one workspace to summarize it, read off the
/// registry before the loop that walks repositories borrows the cache.
struct WorkspaceSummarySubject {
    workspace_id: String,
    root: std::path::PathBuf,
    /// Provisioned and usable. A workspace that is anything else is not a
    /// workspace Done is offered on.
    ready: bool,
    /// The Git directories in it, which are the only ones Done measures.
    repositories: Vec<std::path::PathBuf>,
    /// Whether it also holds an ordinary directory. Nothing publishes one, so
    /// Done — which removes the workspace — is not offered over it.
    plain_directories: bool,
}

/// The Git half of why Done is unavailable, read off the summary the feed
/// already carries rather than by walking the repositories again. A workspace
/// holding no repository has no Git work to lose; one whose walk has not
/// landed yet is unknown rather than ready.
fn git_finish_blockers(summary: &Value, repositories: &[std::path::PathBuf]) -> Vec<&'static str> {
    if repositories.is_empty() {
        return Vec::new();
    }
    let (Some(pushes), Some(dirty)) = (summary["pushes"].as_u64(), summary["dirty"].as_bool())
    else {
        return vec![crate::workspace::FINISH_BLOCKER_UNKNOWN];
    };
    crate::workspace::summary_finish_blockers(&crate::gitgui::WorkSummary {
        pushes,
        behind: 0,
        additions: 0,
        deletions: 0,
        dirty,
        clean: pushes == 0 && !dirty,
    })
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

/// Which project a checkout's row is read against.
pub(in crate::app) struct ExternalRowScope<'a> {
    pub(in crate::app) project_id: &'a str,
    pub(in crate::app) project_name: &'a str,
    pub(in crate::app) base_branch: &'a str,
}

/// One external checkout's ride-along row, as `board.list` carries it.
pub(in crate::app) fn external_worktree_row(
    w: &crate::worktree::ExternalWorktree,
    scope: &ExternalRowScope<'_>,
    agent: (bool, bool),
    attention: Option<Value>,
) -> Value {
    let (agent_working, can_finish) = agent;
    json!({
        "worktree_id": w.id,
        "project_id": scope.project_id,
        "project": scope.project_name,
        "path": w.path.display().to_string(),
        "isolation": w.isolation.wire(),
        "branch": w.branch,
        "head_sha": w.head_sha,
        "head_subject": w.head_subject,
        "head_age_seconds": w.head_age_seconds,
        "head_committed_at": w.head_committed_at,
        "dirty_files": w.dirty_files,
        // Ahead and behind always share one comparison ref. The working-tree
        // delta is reported separately below.
        "comparison_ref": w.comparison_ref,
        "ahead": w.ahead,
        "behind": w.behind,
        "base_branch": scope.base_branch,
        "unpushed": w.unpushed,
        "upstream": w.upstream,
        "diffstat": w.diffstat.to_json(),
        // What is sitting in the tree unsaved — the rail's +/−.
        "uncommitted": w.uncommitted.to_json(),
        "adoptable": w.branch.as_deref().is_some_and(|b| b != scope.base_branch),
        "agent_working": agent_working,
        "can_finish": can_finish,
        // A worktree Build cut carries attention from birth, so it surfaces in
        // the rail as something waiting for you. One made outside Build has
        // none until you act on it here, and stays in the Worktrees row until
        // then.
        "attention": attention.unwrap_or_else(|| json!({
            "resume_at": Value::Null,
            "interacted": false,
            "seen": false,
        })),
    })
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

    /// Every project's external worktrees, carried by `board.list`
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
                rows.rows.push(external_worktree_row(
                    &w,
                    &ExternalRowScope {
                        project_id: &project_id,
                        project_name: &project_name,
                        base_branch: &base_branch,
                    },
                    agent_signals.get(&w.id).copied().unwrap_or((false, false)),
                    attention_of.get(&w.id).cloned(),
                ));
            }
        }
        rows
    }

    /// The same row for ONE checkout, off the caches the board itself reads —
    /// what a `state` push carries for an external worktree. Nothing is
    /// scanned here: a flush answers with what the board knows, and the walk
    /// that would make it newer is the board's own TTL to run.
    pub(in crate::app) fn external_worktree_row_of(&self, worktree_id: &str) -> Option<Value> {
        let projects = self.projects.iter().filter(|project| project.is_git);
        for project in projects {
            // A project whose walk has not landed is not an answer about the
            // board: keep looking in the ones that have.
            let Some(scan) = self.board.diff().external_scan_cache(&project.id) else {
                continue;
            };
            let Some(w) = scan.worktrees.iter().find(|w| w.id == worktree_id) else {
                continue;
            };
            let agent = self
                .session_registry
                .agent_working_roots()
                .into_iter()
                .find(|(root, _)| crate::worktree::external_worktree_id(root) == w.id)
                .map(|(_, working)| (working, !working));
            let attention = self
                .board
                .attention()
                .attention_ids()
                .any(|id| id == w.id)
                .then(|| self.attention_json(&w.id));
            return Some(external_worktree_row(
                w,
                &ExternalRowScope {
                    project_id: &project.id,
                    project_name: &project.name,
                    base_branch: &project.base_branch,
                },
                agent.unwrap_or((false, false)),
                attention,
            ));
        }
        None
    }

    /// Workspace-wide publication-aware summaries, served stale while every
    /// repository walk runs through the existing off-lock cache worker.
    ///
    /// Each row also carries Done: whether it is offered, and what is standing
    /// in the way while it is not. Done removes the workspace, so it appears
    /// only once every Git directory has put its work somewhere else.
    pub(in crate::app) fn workspace_summaries_json(&mut self) -> Vec<Value> {
        let workspaces = self
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| WorkspaceSummarySubject {
                workspace_id: workspace.id.clone(),
                root: Self::canonical_root(&workspace.root),
                ready: workspace.status == crate::workspace::WorkspaceStatus::Ready,
                repositories: workspace
                    .directories
                    .iter()
                    .filter(|directory| directory.is_git)
                    .map(|directory| directory.path.clone())
                    .collect::<Vec<_>>(),
                plain_directories: workspace
                    .directories
                    .iter()
                    .any(|directory| !directory.is_git),
            })
            .collect::<Vec<_>>();
        self.sync_workspace_summaries(
            &workspaces
                .iter()
                .map(|subject| (subject.workspace_id.clone(), subject.repositories.clone()))
                .collect::<Vec<_>>(),
        );
        workspaces
            .into_iter()
            .map(|subject| {
                let summary = self.workspace_summary_json(&subject);
                let mut blockers = Vec::new();
                if self.agent_working_at_root(&subject.root) {
                    blockers.push(crate::workspace::FINISH_BLOCKER_AGENT_WORKING);
                }
                if subject.plain_directories {
                    blockers.push(crate::workspace::FINISH_BLOCKER_PLAIN_DIRECTORY);
                }
                blockers.extend(git_finish_blockers(&summary, &subject.repositories));
                json!({
                    "workspace_id": subject.workspace_id,
                    "work_summary": summary,
                    // A workspace still being built, or one that failed, says
                    // what it is in its own status; Done is not on it either
                    // way.
                    "can_finish": subject.ready && blockers.is_empty(),
                    "finish_blockers": blockers,
                })
            })
            .collect()
    }

    /// One workspace's summary, refreshed behind the answer when the last walk
    /// has aged out. `null` while nothing has landed, and for a workspace
    /// holding no repository at all.
    fn workspace_summary_json(&mut self, subject: &WorkspaceSummarySubject) -> Value {
        let computed_at = self
            .workspace_summary_of(&subject.workspace_id, &subject.repositories)
            .map(|(at, _)| at);
        let refresh =
            self.workspace_summary_refresh(&subject.workspace_id, subject.repositories.clone());
        self.refresh_if_stale(computed_at, WORKSPACE_SUMMARY_TTL, refresh);
        self.workspace_summary_of(&subject.workspace_id, &subject.repositories)
            .map(|(_, summary)| summary.clone())
            .unwrap_or(Value::Null)
    }

    // ---- Board + views --------------------------------------------------------

    /// The board: workspace branches and runs (each run carries a live
    /// diffstat), plus the ride-along external-worktree summaries. Legacy
    /// tasks remain available through their direct read
    /// APIs, but no longer participate in this active-work surface.
    ///
    /// `pub(crate)`, not `pub(in crate::app)`: `api::v1::board` serves
    /// `board.list` from here.
    pub(crate) fn board_list(&mut self) -> Value {
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
        let workspace_summaries = self.workspace_summaries_json();
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
        // a branch Build never cut or adopted — no run behind it — earns no row
        // here, ever — not while an agent happens to be active in it, not on a
        // fresh commit.
        // Adopting it (or sending it a first message, which adopts on the way)
        // is what brings it in; from there its row leaves the same way every
        // other row does — Done, deleted, or dismissed — never on its own.
        // This filter is the feed list's alone.
        let mut items: Vec<Value> = self
            .work_items(&checkouts.rows)
            .into_iter()
            .filter(|row| {
                row["kind"] != crate::branch::WorkItemKind::Branch.as_str()
                    || !row["run_id"].is_null()
            })
            .collect();
        // The tasks the user is watching, in the same list as the
        // conversations so the two interleave by `anchor` for free (spec:
        // Tasks → Watching). A row's absence is how unwatching shows.
        items.extend(self.watched_task_rows());
        crate::branch::sort_by_anchor(&mut items);
        json!({
            // The active feed contains workspace-backed work only. Legacy
            // task records are intentionally absent from both the folded
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
            "workspace_summaries": workspace_summaries,
            // The harnesses out of usage on this device (task #58). The board
            // item carries the same list whole whenever it moves.
            "usage_limits": self.usage_limits_json(),
        })
    }

    /// The feed's work items: one row per branch or task.
    ///
    /// Branch rows fold the three ways a branch can be stored — a run, an
    /// adopted worktree, a worktree Build never cut — into one shape keyed
    /// `(project_id, branch)`. A task whose implementation is still in flight is
    /// spoken for by that implementation's branch row and emits none of its
    /// own. Both rules live in [`crate::branch`].
    ///
    /// Every branch a worktree names resolves here, whether or not it belongs
    /// on the INBOX list — `board_list`'s `items` read this, and additionally
    /// filter out a worktree Build never adopted with no agent presently in
    /// it (see `board_list`). This function stays the unfiltered source of truth for
    /// "what branch is this," not "what does the inbox show."
    ///
    /// The scans are passed in rather than taken again: `board_list` already
    /// paid for them, and re-running them here would double every poll's git
    /// work.
    pub(in crate::app) fn work_items(&mut self, external_worktrees: &[Value]) -> Vec<Value> {
        self.observe_conversationless_rows(external_worktrees);
        self.reconcile_crossed_dismissal_lines();
        let run_ids: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, active)| active.run.state != RunState::Archived)
            // A conversation nobody is watching is not in the inbox (spec:
            // Tasks → Watching). An agent an agent made is its own business
            // until it asks for the user, and a run whose every agent has been
            // unwatched is a row the user has already put down.
            //
            // A run with NO agents is not unwatched — it is a checkout nobody
            // has started yet, and it has always had a row. "Every agent is
            // unwatched" is only an answer when there are agents to ask.
            .filter(|(_, active)| {
                active.agents.is_empty() || active.agents.iter().any(|agent| agent.watched)
            })
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

    pub(in crate::app) fn observe_conversationless_rows(&mut self, external_worktrees: &[Value]) {
        let now = now_rfc3339();
        let external_keys = external_worktrees.iter().filter_map(|entry| {
            let project_id = entry["project_id"].as_str()?;
            Some(match entry["branch"].as_str() {
                Some(branch) => crate::attention::branch_row_key(project_id, branch),
                None => entry["worktree_id"].as_str()?.to_string(),
            })
        });
        let mut changed = false;
        for key in external_keys {
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
        let task_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let sync = WorkItemStat::from_run_stat(stat);
        let thread = self.conversation_thread_for_run(active);
        let unread = self.unread_for(run_id, thread);
        let working = self.entity_agents_working(run_id);
        let working_since = working
            .then(|| self.working_since_for(run_id, thread))
            .flatten();
        let session = self.session_summary(run_id);
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
            // `Detail`, on a row: a cache-first client reads its agents off
            // this row and asks no detail verb after it, so the surface
            // snapshot has to ride here or the reader never sees a goal, a
            // checklist or a workflow again. The scope is what carries it.
            "agents": self.agent_digests(run_id, DigestScope::Detail),
            "stat": sync.to_json(),
            "resume_at": self.attention_json(run_id)["resume_at"],
            // Where this row sits in the inbox, and how long it has been quiet.
            // Every row carries both, whatever it was read off.
            "anchor": self.anchor_of(run_id),
            "last_activity": self.last_activity_of(Some(run_id), thread),
            // This conversation's own message session (#98's rule). The inbox
            // orders the project agent's row by it (#103); a workspace's row
            // reads the same numbers off its workspace.
            "session_started_ms": session.session_started_ms,
            "last_activity_ms": session.last_activity_ms,
            // Done deletes the branch and its records. What the deletion
            // would cost is `finish.warnings`, which the client confirms
            // through — never a refusal here.
            "can_finish": true,
            "finish": { "warnings": sync.finish_warnings_json(&active.worktree.branch()) },
            "muted": self.is_muted(run_id),
            // Cleared out of the inbox until the work speaks again. The client
            // hides the row on it; nothing here changes because of it.
            "dismissed": self.is_dismissed(run_id),
            "worktree_path": active.worktree.path.display().to_string(),
            "worktree_id": crate::worktree::external_worktree_id(&Self::canonical_root(&active.worktree.path)),
            "run_id": run_id,
            "task_id": task_id,
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
            task_id: active.run.plan_id.as_ref().map(|id| id.0.clone()),
            implementation_active: !active.run.state.is_terminal(),
            row,
        }
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
            // A bare checkout is dated by its own commits, unless the user has
            // acted on it here and given it an anchor.
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
            "task_id": Value::Null,
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
            task_id: None,
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

    pub(in crate::app) fn entity_agents_working(&self, entity_id: &str) -> bool {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster.iter().any(|agent| agent.working_since.is_some())
    }
}
