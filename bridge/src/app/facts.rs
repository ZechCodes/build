//! What the push bus is built over (wire spec Part 1, step 1.3): the board's
//! worktree entities, and the per-flush lookups that fill a `changes` item.
//!
//! [`ChangeBus`] knows nothing about [`AppState`]; it is handed two closures
//! at construction. Both run off the app mutex — the entity list is a map
//! snapshot the watchers hold, and the facts source takes the mutex for the
//! thread tails and the state rows only, then runs the git status walks with
//! it released.

use super::watchers::{WorktreeRoots, WorktreeWatchers};
use super::{AppState, ReadSubject};
use crate::changes::WORKING_TREE_DIFF_MAX_BYTES;
use crate::changes::{ChangeBus, EntityFacts, FactsRequest, ThreadTip};
use crate::gitgui::{counted_status_shape, log_page, unpushed_summary, GIT_STATUS_MAX_FILES};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// What a flush reads one checkout's git with: where it is, the branch its
/// history is marked against, and the diff verb's own subject — the SAME
/// [`ReadSubject`] `run.diff`, `worktree.diff` and `project.diff` render, so
/// a pushed diff and a pulled one are one shape and a client keeps them in
/// one cache slot.
pub(in crate::app) struct GitSubject {
    root: PathBuf,
    base_branch: Option<String>,
    diff: ReadSubject,
}

impl GitSubject {
    /// What this checkout's history marks, exactly as `git.log` marks it for
    /// the same scope.
    fn highlight(&self) -> Option<crate::gitgui::LogHighlight<'_>> {
        self.base_branch
            .as_deref()
            .map(crate::gitgui::LogHighlight::AheadOfBase)
    }
}

/// Everything a `git` item carries, read with the app mutex released: the
/// status walk, the latest commits, what is unpublished, and the working
/// tree's diff.
fn read_git(subject: &GitSubject, fact: &mut EntityFacts) {
    if let Ok((status, key)) = counted_status_shape(&subject.root, GIT_STATUS_MAX_FILES) {
        fact.head = status["head"].as_str().map(str::to_string);
        fact.status = Some(status);
        fact.status_key = Some(key);
    }
    fact.log = log_page(
        &subject.root,
        subject.highlight(),
        crate::api::v1::git::LATEST_COMMITS as usize,
        0,
        None,
    )
    .ok();
    fact.unpushed = unpushed_summary(&subject.root).ok();
    (fact.diff, fact.diff_bytes) = read_worktree_diff(subject);
}

/// The working tree's diff, when it is small enough to push — the body this
/// checkout's own diff verb answers with, less the `diff_key` a conditional
/// read stamps on (computing one is a second diff walk, and a push is
/// nobody's conditional read). The size rides the item either way; past the
/// cap the client reads the body itself when a reviewer opens the changes.
fn read_worktree_diff(subject: &GitSubject) -> (Option<Value>, Option<u64>) {
    let Ok(body) = subject.diff.render(crate::diff::DiffPaths::All) else {
        return (None, None);
    };
    let Some(bytes) = body["patch"].as_str().map(str::len) else {
        return (None, None);
    };
    let body = (bytes <= WORKING_TREE_DIFF_MAX_BYTES).then_some(body);
    (body, Some(bytes as u64))
}

/// Where the facts source finds the state once it is shared. Filled by
/// [`AppState::shared`]; before that, or after the state is gone, the source
/// answers nothing and every item reads as "refetch".
pub(in crate::app) type FactsHandle = Arc<OnceLock<Weak<Mutex<AppState>>>>;

/// The daemon's bus: `{"kind":"all"}` resolves against the watchers' roots
/// snapshot, and a flush's keys come from [`entity_facts`].
pub(in crate::app) fn bus_with_sources(
    window: Duration,
    watchers: &Arc<WorktreeWatchers>,
    handle: &FactsHandle,
) -> Arc<ChangeBus> {
    let entities = Arc::clone(watchers);
    let handle = Arc::clone(handle);
    ChangeBus::with_sources(
        window,
        Arc::new(move || entities.entity_ids()),
        Arc::new(move |requests| entity_facts(&handle, requests)),
    )
}

/// What a board item's whole lists cost, split by what the app mutex is
/// needed for.
///
/// A project row RENDERS by opening its repository, reading its volume and
/// asking git for the origin remote — `std::process::Command`, once per
/// project and per source, with no timeout, against whatever the repository
/// sits on. `project.list` defers for exactly that reason, and a flush may
/// not hold the daemon's one lock for it either: a project on an
/// unresponsive mount would stall every RPC and every push for as long as
/// the subprocess took. So the rows are captured under the lock and
/// rendered with it released.
pub(in crate::app) struct BoardListFacts {
    projects: Option<Vec<super::ProjectListRow>>,
    workspaces: Option<Value>,
    usage_limits: Option<Value>,
}

impl BoardListFacts {
    /// The lists as the board item carries them, under the keys
    /// `project.list` and `workspace.list` answer under. MUST run with the
    /// app mutex released.
    pub(in crate::app) fn render(self) -> Value {
        let mut state = serde_json::Map::new();
        if let Some(rows) = self.projects {
            let projects: Vec<Value> = rows.iter().map(super::ProjectListRow::render).collect();
            state.insert("projects".into(), Value::Array(projects));
        }
        if let Some(workspaces) = self.workspaces {
            state.insert("workspaces".into(), workspaces);
        }
        if let Some(usage_limits) = self.usage_limits {
            state.insert("usage_limits".into(), usage_limits);
        }
        Value::Object(state)
    }
}

/// What one request needs read under the app mutex: the conversation tails,
/// the row, and the tab list. The board item's whole lists are taken beside
/// this, by [`AppState::board_lists`].
fn locked_facts(
    app: &mut AppState,
    request: &FactsRequest,
    subjects: &BTreeMap<String, GitSubject>,
) -> EntityFacts {
    EntityFacts {
        entity_id: request.entity_id.clone(),
        threads: if request.thread {
            app.thread_tips(&request.entity_id, &request.thread_after)
        } else {
            Vec::new()
        },
        state: request
            .state
            .then(|| app.entity_state_item(&request.entity_id))
            .flatten(),
        terminals: request
            .terminals
            .then(|| subjects.get(&request.entity_id))
            .flatten()
            .map(|subject| json!({ "tabs": app.shell_tabs_at(&subject.root) })),
        ..EntityFacts::default()
    }
}

/// Answer a flush's lookups. The mutex is held for the thread tails and the
/// roots only; every status walk runs with it released.
fn entity_facts(handle: &FactsHandle, requests: &[FactsRequest]) -> Vec<EntityFacts> {
    let Some(state) = handle.get().and_then(Weak::upgrade) else {
        return Vec::new();
    };
    let (subjects, summary_subjects, mut facts, board_lists) = {
        let mut app = state.lock().unwrap();
        let subjects = app.git_subjects();
        let summary_subjects = requests
            .iter()
            .filter(|request| request.git)
            .filter_map(|request| app.workspace_summary_subject_of_run(&request.entity_id))
            .collect::<Vec<_>>();
        let facts = requests
            .iter()
            .map(|request| locked_facts(&mut app, request, &subjects))
            .collect::<Vec<_>>();
        let board_lists = requests
            .iter()
            .find(|request| !request.lists.is_empty())
            .map(|request| app.board_lists(request.lists));
        (subjects, summary_subjects, facts, board_lists)
    };
    if let Some(lists) = board_lists {
        let board = facts
            .iter_mut()
            .find(|fact| fact.entity_id == crate::changes::BOARD_ITEM_ID);
        if let Some(fact) = board {
            fact.state = Some(lists.render());
        }
    }
    let mut fresh_stats = Vec::new();
    for (request, fact) in requests.iter().zip(facts.iter_mut()) {
        let Some(subject) = subjects.get(&request.entity_id) else {
            continue;
        };
        if request.git {
            read_git(subject, fact);
            if let Some(stat) = run_stat_of(subject) {
                restat_row(fact, &stat);
                fresh_stats.push((fact.entity_id.clone(), stat));
            }
        }
        if request.files {
            fact.root_listing = super::fs::directory_listing(&subject.root, "").ok();
        }
    }
    // A run whose git moved has a row whose stat moved with it, and a
    // workspace conversation's workspace has a work summary that moved too.
    // Both walks ran with the mutex released; what they found is stored here
    // so the `state` item the bus notes after this flush — and the workspace
    // list the board item carries — say what the checkout is, not what the
    // board's last TTL read left it as.
    let summaries = summary_subjects
        .into_iter()
        .map(|(workspace_id, repositories)| {
            let summary = super::board::cache::workspace_work_summary(&repositories);
            (workspace_id, repositories, summary)
        })
        .collect::<Vec<_>>();
    if !fresh_stats.is_empty() || !summaries.is_empty() {
        let mut app = state.lock().unwrap();
        let now = std::time::Instant::now();
        for (run_id, stat) in fresh_stats {
            app.store_run_stat(run_id, stat, now);
        }
        let lists_moved = !summaries.is_empty();
        if lists_moved {
            // The cache keeps a summary only under a membership it knows, and
            // a workspace made since the last board read has none yet.
            let memberships = app.workspace_summary_memberships();
            app.sync_workspace_summaries(&memberships);
        }
        for (workspace_id, repositories, summary) in summaries {
            app.store_diff_entry(super::board::cache::DiffCacheEntry::WorkspaceSummary {
                workspace_id,
                repositories,
                summary,
            });
        }
        if lists_moved {
            app.changes
                .note_board_lists(crate::changes::BoardLists::WORKSPACES);
        }
    }
    facts
}

/// A run's diffstat, walked off the app lock; `None` for a subject that is
/// no run, whose row carries no stat of this shape.
fn run_stat_of(subject: &GitSubject) -> Option<Value> {
    let ReadSubject::Run { .. } = subject.diff else {
        return None;
    };
    let base_branch = subject.base_branch.as_deref()?;
    Some(super::board::cache::run_diffstat(
        &subject.root,
        base_branch,
    ))
}

/// The row this flush carries, with the stat it just walked in place of the
/// one the board read under its TTL.
fn restat_row(fact: &mut EntityFacts, stat: &Value) {
    let Some(row) = fact.state.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if row.contains_key("stat") {
        row.insert("stat".into(), stat.clone());
    }
}

impl AppState {
    /// Every board entity with a checkout: a run's worktree, a project's
    /// primary checkout, and each external worktree the last scan found.
    pub(in crate::app) fn worktree_roots(&self) -> WorktreeRoots {
        self.git_subjects()
            .into_iter()
            .map(|(id, subject)| (id, subject.root))
            .collect()
    }

    /// The same entities, with what each one's git is read against — the map
    /// a flush works from, and the one [`worktree_roots`](Self::worktree_roots)
    /// is the paths of.
    pub(in crate::app) fn git_subjects(&self) -> BTreeMap<String, GitSubject> {
        let mut subjects = BTreeMap::new();
        for (id, active) in &self.runs {
            if active.run.state == crate::run::RunState::Archived {
                continue;
            }
            let root = self.run_git_root(id, &active.worktree.path);
            let base_branch = self.run_base_branch(id, &active.worktree.base_branch);
            subjects.insert(
                id.clone(),
                GitSubject {
                    root: root.clone(),
                    base_branch: Some(base_branch.clone()),
                    diff: ReadSubject::Run {
                        worktree_path: root,
                        base_sha: active.base_sha.clone(),
                        base_branch,
                    },
                },
            );
        }
        for project in self.projects.iter().filter(|project| project.is_git) {
            subjects.insert(
                project.id.clone(),
                GitSubject {
                    root: project.repo_path.clone(),
                    base_branch: None,
                    diff: ReadSubject::Project {
                        project_id: project.id.clone(),
                        repo_path: project.repo_path.clone(),
                    },
                },
            );
            let Some(scan) = self.board.diff().external_scan_cache(&project.id) else {
                continue;
            };
            for checkout in &scan.worktrees {
                subjects.insert(
                    checkout.id.clone(),
                    GitSubject {
                        root: checkout.path.clone(),
                        base_branch: Some(project.base_branch.clone()),
                        diff: ReadSubject::Worktree {
                            external: Box::new(checkout.clone()),
                            base_branch: project.base_branch.clone(),
                        },
                    },
                );
            }
        }
        subjects
    }

    /// Where this entity's row stands — the `state` item, which carries the
    /// WHOLE row `board.list` paints for it, so a client writes the item into
    /// its cache and repaints the feed without asking anything back.
    ///
    /// `None` for an id the board paints no row for: a project's primary
    /// checkout, a run that has been archived, a legacy task. Those items
    /// stay the bare "refetch" they have always been.
    pub(in crate::app) fn entity_state_item(&self, entity_id: &str) -> Option<Value> {
        self.board_row(entity_id)
            .or_else(|| self.legacy_task_digest(entity_id))
    }

    /// The row `board.list` carries for this entity: a run's folded feed row,
    /// or an external checkout's ride-along row.
    ///
    /// Read off the caches the board reads and never refreshed here. A flush
    /// answers with what the board already knows — the diffstat walk behind a
    /// row belongs to the board's own TTL, and a push must not start one per
    /// entity that moved. `board_list` also reconciles conversationless rows
    /// and crossed dismissal lines before it builds its rows; this is a
    /// `&self` read and does neither, so a pushed row's attention can lag the
    /// board's by one `board.list`.
    ///
    /// Two rows can contest one branch key — an adopted worktree and a
    /// Build-cut run on the same branch. The board folds those to one
    /// ([`crate::branch::fold_work_items`]) and this does not, so both push a
    /// row and a client painting every pushed row shows one the board would
    /// have folded away. Deliberately left: the fold's tie-break between two
    /// runs is their order in a `HashMap`, so there is no stable winner for a
    /// push to agree with, and dropping both rows would leave the one that IS
    /// painted with no live updates at all. The board's own tie-break has to
    /// become stable first.
    fn board_row(&self, entity_id: &str) -> Option<Value> {
        let Some(active) = self.runs.get(entity_id) else {
            return self.external_worktree_row_of(entity_id);
        };
        if active.run.state == crate::run::RunState::Archived {
            return None;
        }
        let stat = self
            .board
            .diff()
            .run_stat(entity_id)
            .map(|cached| cached.value.clone())
            .unwrap_or(Value::Null);
        Some(crate::branch::named_project_row(
            self.branch_candidate_from_run(entity_id, &stat).row,
        ))
    }

    /// The three-field digest a legacy task still answers with. Tasks left
    /// the board, so nothing paints a row for one; a client watching an old
    /// record hears the same lifecycle, agent count and attention it always
    /// heard.
    fn legacy_task_digest(&self, entity_id: &str) -> Option<Value> {
        let lifecycle = super::plan_state_str(&self.plans.get(entity_id)?.plan.state);
        let agents = self
            .entity_agents(entity_id)
            .map(|roster| roster.iter().count())
            .unwrap_or(0);
        Some(json!({
            "run": lifecycle,
            "agents": agents,
            "attention": self.unread_for(entity_id, None).reason.unwrap_or("none"),
        }))
    }

    /// The whole lists a board item carries when the change that noted it
    /// moved one — the half of them that needs the app mutex.
    ///
    /// The workspace list is read here whole: it is what `workspace.list`
    /// answers, and that verb runs under this lock for every client read
    /// too. The project rows are only captured; see [`BoardListFacts`].
    /// The workspace rows with the board's verdict on each: its work summary,
    /// whether Done is on it, and what blocks Done. `workspace.list` answers
    /// the rows alone; a board item carries them the way `board.list` does, so
    /// a client re-stamping the list has the summary this flush re-read rather
    /// than the one it was holding.
    fn with_work_summaries(&mut self, workspaces: Value) -> Value {
        let summaries = self.workspace_summaries_json();
        let Some(rows) = workspaces.as_array() else {
            return workspaces;
        };
        Value::Array(
            rows.iter()
                .map(|row| {
                    let mut row = row.clone();
                    let verdict = summaries
                        .iter()
                        .find(|summary| summary["workspace_id"] == row["workspace_id"]);
                    if let Some(verdict) = verdict {
                        for key in ["work_summary", "can_finish", "finish_blockers"] {
                            row[key] = verdict[key].clone();
                        }
                    }
                    row
                })
                .collect(),
        )
    }

    pub(in crate::app) fn board_lists(
        &mut self,
        lists: crate::changes::BoardLists,
    ) -> BoardListFacts {
        BoardListFacts {
            projects: lists.projects.then(|| self.project_list_rows()),
            workspaces: lists
                .workspaces
                .then(|| self.workspace_list(&json!({})).ok())
                .flatten()
                .map(|listed| self.with_work_summaries(listed["workspaces"].clone()))
                // A list this could not read is left UNSAID rather than said
                // to be null: the board item's lists are arrays by contract,
                // and the bus keeps the subscription's latch up for a list
                // nothing answered.
                .filter(Value::is_array),
            usage_limits: lists.usage_limits.then(|| self.usage_limits_json()),
        }
    }

    /// Where each of an entity's conversations stands, and what was said to
    /// get there — the `thread` item.
    ///
    /// `after` is what the subscription being flushed has already been sent,
    /// per agent. An agent it names gets the items since that sequence; one
    /// it does not — the first flush a subscription makes for a conversation
    /// its client has just read for itself — gets its tip alone.
    ///
    /// Every read here is off the tail this process holds. A cursor from
    /// under that tail is answered with the tip alone rather than with a
    /// store read: this runs under the app mutex, and the client can page
    /// forward for the gap without holding the daemon up.
    pub(in crate::app) fn thread_tips(
        &self,
        entity_id: &str,
        after: &[(String, u64)],
    ) -> Vec<ThreadTip> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let carried =
                    after
                        .iter()
                        .find(|(id, _)| *id == agent.id)
                        .and_then(|(_, since)| {
                            thread
                                .push_items_after(*since, crate::changes::THREAD_PUSH_MAX_ITEMS)
                                .map(|items| (*since, items))
                        });
                let (since_sequence, items) = match carried {
                    Some((since, items)) => (Some(since), items),
                    None => (None, Vec::new()),
                };
                let project_id = self.projects.project_id_of(entity_id).map(str::to_owned);
                let workspace_id = if self.is_project_conversation_owner(entity_id) {
                    None
                } else {
                    self.workspaces
                        .list(project_id.as_deref())
                        .into_iter()
                        .find(|workspace| {
                            self.workspace_conversation_owner(workspace).as_deref()
                                == Some(entity_id)
                        })
                        .map(|workspace| workspace.id.clone())
                };
                ThreadTip {
                    agent_id: agent.id.clone(),
                    conversation_id: Some(agent.conversation_id().to_string()),
                    last_sequence: thread.last_sequence(),
                    workspace_session: workspace_id
                        .as_deref()
                        .map(|_| self.session_summary(entity_id)),
                    project_session: project_id.as_deref().map(|id| self.session_summary(id)),
                    workspace_id,
                    project_id,
                    items,
                    since_sequence,
                }
            })
            .collect()
    }
}
