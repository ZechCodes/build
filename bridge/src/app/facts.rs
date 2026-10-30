//! What the push bus is built over (wire spec Part 1, step 1.3): the board's
//! worktree entities, and the per-flush lookups that fill a `changes` item.
//!
//! [`ChangeBus`] reads a committed immutable projection. Filesystem and Git
//! observations run independently, and their cache updates belong to a separate
//! producer which cannot delay a flush on the application mutex.

mod observations;
mod snapshot;
pub(in crate::app) use snapshot::{entity_facts, FactsHandle, FactsSource, PushFacts};

use super::git::{namespace_payload_keys, namespaced_key, project_source_namespace};
use super::read_view::SharedJson;
use super::watchers::{WorktreeRoots, WorktreeWatchers};
use super::{AppState, ReadSubject};
use crate::changes::WORKING_TREE_DIFF_MAX_BYTES;
use crate::changes::{ChangeBus, EntityFacts, ThreadTip};
use crate::gitgui::{counted_status_shape, log_page, unpushed_summary, GIT_STATUS_MAX_FILES};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// What a flush reads one checkout with: where it is, the branch its
/// history is marked against, and the diff verb's own subject — the SAME
/// [`ReadSubject`] the run and worktree diff reads render, so
/// a pushed diff and a pulled one are one shape and a client keeps them in
/// one cache slot.
pub(in crate::app) struct CheckoutSubject {
    root: PathBuf,
    base_branch: Option<String>,
    /// `None` for a plain folder: files and terminals, and no Git to read.
    diff: Option<ReadSubject>,
    /// The cache namespace the scope's own `git.*` verbs key under, so a
    /// pushed key is the key a pull would answer.
    namespace: Option<String>,
    terminal_source: Option<crate::changes::ProjectSourceScope>,
}

impl CheckoutSubject {
    fn terminal_scope(&self) -> super::runtime::terminals::TerminalScope {
        super::runtime::terminals::TerminalScope {
            root: self.root.clone(),
            source: self.terminal_source.clone(),
        }
    }

    /// One original project source: its uncommitted work read as the
    /// primary checkout's is, keys in the namespace its `git.*` verbs use,
    /// and no Git at all for a plain folder.
    fn of_project_source(source: super::projects::ResolvedProjectSource) -> Self {
        let namespace = project_source_namespace(&source);
        let terminal_source = crate::changes::ProjectSourceScope::new(
            source.project_id.clone(),
            source.source_id.clone(),
            source.source_path.clone(),
        );
        let diff = source.is_git.then(|| ReadSubject::Project {
            project_id: source.project_id,
            repo_path: source.root.clone(),
            key_namespace: Some(namespace.clone()),
        });
        Self {
            root: source.root,
            base_branch: None,
            diff,
            namespace: Some(namespace),
            terminal_source: Some(terminal_source),
        }
    }

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
fn read_git(subject: &CheckoutSubject, diff: &ReadSubject, fact: &mut EntityFacts) {
    if let Ok((status, key)) = counted_status_shape(&subject.root, GIT_STATUS_MAX_FILES) {
        fact.head = status["head"].as_str().map(str::to_string);
        let (status, key) = match &subject.namespace {
            Some(namespace) => (
                namespace_payload_keys(namespace, status),
                namespaced_key(namespace, &key),
            ),
            None => (status, key),
        };
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
    fact.unpushed = unpushed_summary(&subject.root).ok().map(|mut summary| {
        if let (Some(namespace), Some(key)) = (&subject.namespace, summary["diff_key"].as_str()) {
            summary["diff_key"] = json!(namespaced_key(namespace, key));
        }
        summary
    });
    (fact.diff, fact.diff_bytes) = read_worktree_diff(diff);
}

/// The working tree's diff, when it is small enough to push — the body this
/// checkout's own diff verb answers with, less the `diff_key` a conditional
/// read stamps on (computing one is a second diff walk, and a push is
/// nobody's conditional read). The size rides the item either way; past the
/// cap the client reads the body itself when a reviewer opens the changes.
fn read_worktree_diff(diff: &ReadSubject) -> (Option<Value>, Option<u64>) {
    let Ok(body) = diff.render(crate::diff::DiffPaths::All) else {
        return (None, None);
    };
    let Some(bytes) = body["patch"].as_str().map(str::len) else {
        return (None, None);
    };
    let body = (bytes <= WORKING_TREE_DIFF_MAX_BYTES).then_some(body);
    (body, Some(bytes as u64))
}

/// The daemon's bus resolves entities and facts from the same committed view.
pub(in crate::app) fn bus_with_sources(
    window: Duration,
    _watchers: &Arc<WorktreeWatchers>,
    handle: &FactsHandle,
) -> Arc<ChangeBus> {
    let entities = Arc::clone(handle);
    let handle = Arc::clone(handle);
    ChangeBus::with_sources(
        window,
        Arc::new(move || {
            entities
                .get()
                .map(|source| source.entity_ids())
                .unwrap_or_default()
        }),
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
#[cfg(test)]
pub(in crate::app) struct BoardListFacts {
    projects: Option<Vec<super::ProjectListRow>>,
    workspaces: Option<Value>,
    usage_limits: Option<Value>,
}

#[cfg(test)]
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

/// A run's diffstat, walked off the app lock; `None` for a subject that is
/// no run, whose row carries no stat of this shape.
fn run_stat_of(subject: &CheckoutSubject) -> Option<Value> {
    let Some(ReadSubject::Run { .. }) = subject.diff else {
        return None;
    };
    let base_branch = subject.base_branch.as_deref()?;
    Some(super::board::cache::run_diffstat(
        &subject.root,
        base_branch,
    ))
}

impl AppState {
    /// Fence completed Build mutations against probes already running or
    /// queued. A workspace aggregate depends on every directory it measured,
    /// and aliases of the same checkout share the invalidation.
    pub(in crate::app) fn invalidate_snapshot_checkout_observations(&self, root: &std::path::Path) {
        let root = self.snapshot_root(root);
        self.read_view.publish_current(|current, version| {
            let entities =
                current
                    .facts
                    .subjects
                    .iter()
                    .filter(|(entity, subject)| {
                        self.snapshot_root(&subject.root) == root
                            || current.facts.summaries.get(*entity).is_some_and(
                                |(_, repositories)| {
                                    repositories
                                        .iter()
                                        .any(|repository| self.snapshot_root(repository) == root)
                                },
                            )
                    })
                    .map(|(entity, _)| entity.clone())
                    .collect::<Vec<_>>();
            if entities.is_empty() {
                return None;
            }
            let mut facts = (*current.facts).clone();
            facts.invalidate_observations(&entities);
            Some(super::read_view::SnapshotDelta::new(version).set_facts(Arc::new(facts)))
        });
    }

    /// Every entity with a checkout: a run's worktree, a project's primary
    /// checkout, each external worktree the last scan found, and each of a
    /// project's original sources, Git or plain.
    pub(in crate::app) fn worktree_roots(&self) -> WorktreeRoots {
        #[cfg(test)]
        let _invocation = crate::workspace::warm_work_counts::roots_invocation();
        let ownership = super::workspaces::RootsOwnership::capture(self);
        self.checkout_subjects_using(Some(&ownership))
            .into_iter()
            .map(|(id, subject)| (id, subject.root))
            .collect()
    }

    /// The same entities, with what each one's git is read against — the map
    /// a flush works from, and the one [`worktree_roots`](Self::worktree_roots)
    /// is the paths of.
    pub(in crate::app) fn checkout_subjects(&self) -> BTreeMap<String, CheckoutSubject> {
        self.checkout_subjects_using(None)
    }

    fn checkout_subjects_using(
        &self,
        ownership: Option<&super::workspaces::RootsOwnership>,
    ) -> BTreeMap<String, CheckoutSubject> {
        let mut subjects = self.git_subjects(ownership);
        subjects.extend(self.project_source_subjects());
        subjects
    }

    /// Each original source of each project, under the entity its
    /// `project_source` subscriptions name (#529). The stored path is the
    /// canonical folder the source was added at, read without touching the
    /// disk: this runs under the app mutex.
    fn project_source_subjects(&self) -> Vec<(String, CheckoutSubject)> {
        let sources = self.projects.iter().flat_map(|project| {
            project
                .sources
                .iter()
                .map(move |source| (project.id.as_str(), source))
        });
        sources
            .map(|(project_id, source)| {
                let resolved = super::projects::ResolvedProjectSource {
                    project_id: project_id.to_string(),
                    source_id: source.id.clone(),
                    source_path: source.path.display().to_string(),
                    root: source.path.clone(),
                    is_git: source.is_git,
                };
                let entity_id = crate::changes::project_source_entity_id(
                    project_id,
                    &source.id,
                    &resolved.source_path,
                );
                (entity_id, CheckoutSubject::of_project_source(resolved))
            })
            .collect()
    }

    /// The producer's cached path observations, with the same workspace
    /// ownership rule as run_git_root and no filesystem probe after warm-up.
    fn snapshot_run_git_root(
        &self,
        run_id: &str,
        checkout: &std::path::Path,
        ownership: Option<&super::workspaces::RootsOwnership>,
    ) -> PathBuf {
        if self.snapshot_path_exists(&checkout.join(".git")) {
            return checkout.to_path_buf();
        }
        let workspace = match ownership {
            Some(ownership) => ownership
                .workspace_for_owner(run_id)
                .and_then(|id| self.workspaces.get(id)),
            None => self.workspace_snapshot_for_conversation(run_id, None),
        };
        workspace
            .and_then(|workspace| {
                workspace
                    .directories
                    .iter()
                    .find(|directory| directory.is_git)
            })
            .map(|directory| directory.path.clone())
            .unwrap_or_else(|| checkout.to_path_buf())
    }

    /// The board's own entities with a Git checkout.
    fn git_subjects(
        &self,
        ownership: Option<&super::workspaces::RootsOwnership>,
    ) -> BTreeMap<String, CheckoutSubject> {
        let mut subjects = BTreeMap::new();
        for (id, active) in &self.runs {
            if active.run.state == crate::run::RunState::Archived {
                continue;
            }
            let root = self.snapshot_run_git_root(id, &active.worktree.path, ownership);
            let base_branch = self.run_base_branch(id, &active.worktree.base_branch);
            subjects.insert(
                id.clone(),
                CheckoutSubject {
                    root: root.clone(),
                    base_branch: Some(base_branch.clone()),
                    diff: Some(ReadSubject::Run {
                        worktree_path: root,
                        base_sha: active.base_sha.clone(),
                        base_branch,
                    }),
                    namespace: None,
                    terminal_source: None,
                },
            );
        }
        for project in self.projects.iter().filter(|project| project.is_git) {
            subjects.insert(
                project.id.clone(),
                CheckoutSubject {
                    root: project.repo_path.clone(),
                    base_branch: None,
                    diff: Some(ReadSubject::Project {
                        project_id: project.id.clone(),
                        repo_path: project.repo_path.clone(),
                        key_namespace: None,
                    }),
                    namespace: None,
                    terminal_source: None,
                },
            );
            let Some(scan) = self.board.diff().external_scan_cache(&project.id) else {
                continue;
            };
            for checkout in &scan.worktrees {
                subjects.insert(
                    checkout.id.clone(),
                    CheckoutSubject {
                        root: checkout.path.clone(),
                        base_branch: Some(project.base_branch.clone()),
                        diff: Some(ReadSubject::Worktree {
                            external: Box::new(checkout.clone()),
                            base_branch: project.base_branch.clone(),
                        }),
                        namespace: None,
                        terminal_source: None,
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

    /// Push capture can retain the exact named node built by this guard's
    /// incremental winner producer. Legacy and external state keep their
    /// existing Value renderers, including archived-run fallback behavior.
    pub(in crate::app) fn entity_state_shared_item(&self, entity_id: &str) -> Option<SharedJson> {
        if self
            .runs
            .get(entity_id)
            .is_some_and(|active| active.run.state != crate::run::RunState::Archived)
        {
            let stat = self
                .board
                .diff()
                .run_stat(entity_id)
                .map(|cached| cached.value.clone())
                .unwrap_or(Value::Null);
            Some(self.snapshot_named_branch_row(entity_id, &stat))
        } else {
            self.entity_state_item(entity_id).map(SharedJson::value)
        }
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
    #[cfg(test)]
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

    #[cfg(test)]
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
        if roster.is_empty() {
            return Vec::new();
        }
        let project_id = self.projects.project_id_of(entity_id).map(str::to_owned);
        let workspace_id = self
            .workspace_snapshot_for_conversation(entity_id, project_id.as_deref())
            .map(|workspace| workspace.id.clone());
        let workspace_session = workspace_id
            .as_deref()
            .map(|_| self.session_summary(entity_id));
        let project_session = project_id.as_deref().map(|id| self.session_summary(id));
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
                ThreadTip {
                    agent_id: agent.id.clone(),
                    conversation_id: Some(agent.conversation_id().to_string()),
                    thread_id: Some(thread.id.clone()),
                    thread_generation_revision: Some(thread.generation_revision),
                    choice_revision: Some(agent.choice_revision),
                    last_sequence: self.snapshot_thread_sequence(thread),
                    history_start_sequence: thread.history_start_sequence,
                    workspace_session,
                    project_session,
                    workspace_id: workspace_id.clone(),
                    project_id: project_id.clone(),
                    items,
                    since_sequence,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::changes::FactsRequest;

    pub(super) fn test_app(root: &std::path::Path) -> AppState {
        let context =
            crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.into()).unwrap();
        AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
    }

    #[test]
    fn git_and_file_facts_return_published_observations_while_the_producer_waits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = crate::git_fixture::init_repo_named(dir.path(), "repo");
        let mut app = test_app(dir.path());
        let project = app.add_project(repo.clone(), "main".into());
        let state = app.shared();
        let held = state.lock().unwrap();
        let handle = Arc::clone(&held.facts_handle);
        let request = [FactsRequest {
            entity_id: project,
            git: true,
            files: true,
            ..FactsRequest::default()
        }];
        let first = entity_facts(&handle, &request);
        let missing = first[0].status_key.is_none() && first[0].root_listing.is_none();
        drop(held);
        assert!(
            missing,
            "a facts reader probed Git/files instead of using the published cache"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let facts = entity_facts(&handle, &request);
            if facts[0].status_key.is_some() && facts[0].root_listing.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the producer did not publish its observation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let held = state.lock().unwrap();
        let observed = entity_facts(&handle, &request);
        std::fs::write(repo.join("later.txt"), "changed").unwrap();
        let cached = entity_facts(&handle, &request);
        drop(held);
        assert_eq!(
            cached[0].root_listing, observed[0].root_listing,
            "a reader refreshed filesystem observations while the producer was blocked"
        );
    }

    #[test]
    fn nonempty_push_facts_finish_with_application_and_store_writer_held() {
        let dir = tempfile::tempdir().unwrap();
        let repo = crate::git_fixture::init_repo_named(dir.path(), "repo");
        let store = crate::store::Store::new(dir.path().join("records")).unwrap();
        let mut app = test_app(dir.path());
        app.store = Some(store.clone());
        let project = app.add_project(repo, "main".into());
        let ensured = app
            .project_ensure_conversation(&json!({"project_id": project}))
            .unwrap();
        let owner = ensured["entity_id"].as_str().unwrap().to_owned();
        let mut active = app.take_run(&owner).unwrap();
        active
            .agents
            .sole_thread_mut()
            .post_user("committed words", None, "2026-01-01T00:00:00Z");
        app.finish_run_mutation(owner.clone(), active).unwrap();
        let state = app.shared();
        let mut writer = store.writer_for_test();
        let transaction = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let held = state.lock().unwrap();
        let handle = Arc::clone(&held.facts_handle);
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            sent.send(entity_facts(
                &handle,
                &[
                    FactsRequest {
                        entity_id: owner,
                        state: true,
                        thread: true,
                        terminals: true,
                        ..FactsRequest::default()
                    },
                    FactsRequest {
                        entity_id: crate::changes::BOARD_ITEM_ID.into(),
                        lists: crate::changes::BoardLists::PROJECTS,
                        ..FactsRequest::default()
                    },
                ],
            ))
            .unwrap();
        });
        let answer = received.recv_timeout(Duration::from_millis(100));
        drop(held);
        drop(transaction);
        drop(writer);
        reader.join().unwrap();
        let facts = answer.expect("push facts waited for the app or store writer mutex");
        assert!(!facts[0].threads.is_empty());
        assert!(facts[0].threads[0].last_sequence > 0);
        assert!(facts[0].state.is_some());
        assert!(facts[0].terminals.is_some());
        assert_eq!(
            facts[1].state.as_ref().unwrap()["projects"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn push_facts_finish_while_application_state_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_app(dir.path()).shared();
        let held = state.lock().unwrap();
        let handle = held.facts_handle.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let facts = entity_facts(
                &handle,
                &[FactsRequest {
                    entity_id: crate::changes::BOARD_ITEM_ID.into(),
                    state: true,
                    ..FactsRequest::default()
                }],
            );
            sent.send(facts).unwrap();
        });
        let answer = received.recv_timeout(Duration::from_millis(100));
        drop(held);
        reader.join().unwrap();
        assert!(answer.is_ok(), "push facts waited for the app mutex");
    }
}
