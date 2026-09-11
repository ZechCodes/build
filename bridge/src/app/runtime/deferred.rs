use crate::app::runtime::lifecycle::{LifecycleOutcome, WorktreeLifecycleJob};
use crate::app::{
    diff_file_edited_at, diff_file_rows, diff_json, entity_ids_of, sha256_hex, worktree_diff_json,
    AppState, DeferredGit,
};
use crate::isolation::{Isolation, IsolationAvailability};
use crate::lifecycle::{PendingRow, WorktreeChange};
use crate::worktree::{git_remote_origin, ExternalWorktree};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// One unit of work split the way the rule splits everything: `decide` runs
/// with the state lock released, `apply` under it. The caller has already
/// taken this job's single-flight claim; `claim` names it, so a decide phase
/// that never returns — a panic on the blocking pool — can still hand it back
/// through `abandon`, or that claim would never be taken again.
pub(in crate::app) trait OffLockJob: Send + 'static {
    type Claim: Send + 'static;
    type Decided: Send + 'static;
    fn claim(&mut self) -> Self::Claim;
    /// MUST run with the state lock released.
    fn decide(self) -> Self::Decided;
    fn apply(state: &mut AppState, claim: Self::Claim, decided: Self::Decided);
    fn abandon(state: &mut AppState, claim: Self::Claim);
}

#[cfg(test)]
pub use crate::test_support::off_lock::{OffLockGate, OffLockGateHandle};

/// Work a verb handed to the drain, to run with the app mutex released.
pub(in crate::app) enum DeferredWork {
    /// One lifecycle verb's git — `git worktree add`, a checkpoint, a scan —
    /// and the row reserved on the board until it returns.
    Lifecycle(Box<WorktreeLifecycleJob>),
    /// One `git.*` verb against one resolved checkout.
    Git(Box<DeferredGit>),
    /// One diff to render for a review surface.
    Read(Box<DeferredRead>),
}

impl DeferredWork {
    /// The lock-free phase. Consumes the work so nothing can run it twice.
    pub(in crate::app) fn run(self) -> DeferredOutcome {
        match self {
            Self::Lifecycle(job) => DeferredOutcome::Lifecycle(Box::new(job.run())),
            Self::Git(git) => {
                #[cfg(test)]
                if let Some(gate) = &git.gate {
                    gate.arrive();
                }
                let result = git.run();
                DeferredOutcome::Git { git, result }
            }
            Self::Read(read) => {
                #[cfg(test)]
                if let Some(gate) = &read.gate {
                    gate.arrive();
                }
                DeferredOutcome::Read(read.run())
            }
        }
    }
}

/// What the lock-free phase brought back, for the app mutex to write down.
pub(in crate::app) enum DeferredOutcome {
    Lifecycle(Box<LifecycleOutcome>),
    Git {
        git: Box<DeferredGit>,
        result: Result<Value, String>,
    },
    Read(Result<Value, String>),
}

/// A read whose git work needs nothing the app mutex holds: the lock resolves
/// its inputs — paths, refs, ids — and the drain renders the answer from them
/// alone.
///
/// Rendering a patch reads every changed blob, so a review surface asking for
/// one is seconds of libgit2 on a large checkout. Nothing is written back
/// afterwards, so there is no staleness to check: the answer describes the tree
/// as it was read, which is what was asked for.
pub(in crate::app) struct DeferredRead {
    pub(in crate::app) subject: ReadSubject,
    /// The issue that asked, when the read came in through an issue surface —
    /// stamped onto the answer, as the issue verbs did before the split.
    pub(in crate::app) issue_id: Option<String>,
    /// The complete aggregate held by the caller. We still recompute to avoid
    /// stale filesystem answers, then suppress the equal payload on the wire.
    pub(in crate::app) if_diff_key: Option<String>,
    #[cfg(test)]
    pub(in crate::app) gate: Option<OffLockGate>,
}

impl DeferredRead {
    pub(in crate::app) fn run(&self) -> Result<Value, String> {
        let conditional_key = self.subject.conditional_key()?;
        if let Some(diff_key) = conditional_key.as_deref() {
            if self.if_diff_key.as_deref() == Some(diff_key) {
                return Ok(json!({ "unchanged": true, "diff_key": diff_key }));
            }
        }
        let mut rendered = self.subject.render()?;
        if let (Some(issue_id), Some(object)) = (&self.issue_id, rendered.as_object_mut()) {
            object.insert("issue_id".to_string(), json!(issue_id));
        }
        if let Some(diff_key) = conditional_key {
            if let Some(object) = rendered.as_object_mut() {
                object.insert("diff_key".to_string(), json!(diff_key));
            }
        }
        Ok(rendered)
    }
}

/// Which diff a deferred read renders.
pub(in crate::app) enum ReadSubject {
    /// `project.list` — immutable row inputs captured at request time, with
    /// repository and volume metadata read while the app mutex is released.
    ProjectList { projects: Vec<ProjectListRow> },
    /// `project.diff` — a primary checkout's uncommitted work.
    Project {
        project_id: String,
        repo_path: std::path::PathBuf,
    },
    /// `worktree.diff` — one external checkout against its base branch.
    Worktree {
        external: Box<ExternalWorktree>,
        base_branch: String,
    },
    /// `run.diff` — one run against its baseline: the sha it started from, or
    /// its merge base with the base branch when it has none.
    Run {
        worktree_path: std::path::PathBuf,
        base_sha: Option<String>,
        base_branch: String,
    },
    /// `run.stage_diff` — one immutable stage boundary, sha to sha.
    Stage {
        run_id: String,
        stage_id: String,
        /// Where the two commits can still be read: the checkout while it
        /// exists, the project's repository once it does not.
        object_database: std::path::PathBuf,
        start_sha: String,
        completion_sha: String,
    },
}

impl ReadSubject {
    pub(in crate::app) fn conditional_key(&self) -> Result<Option<String>, String> {
        let material = match self {
            Self::Worktree {
                external,
                base_branch,
            } => format!(
                "worktree\0{}\0{}\0{:?}\0{}\0{}\0{}",
                external.id,
                base_branch,
                external.branch,
                external.head_subject,
                external.dirty_files,
                crate::diff::key_against_merge_base(&external.path, base_branch)
                    .map_err(|error| error.to_string())?
            ),
            Self::Run {
                worktree_path,
                base_sha,
                base_branch,
            } => {
                let (base, delta_key) = match base_sha {
                    Some(sha) => (
                        sha.as_str(),
                        crate::diff::key_against_base(worktree_path, sha)
                            .map_err(|error| error.to_string())?,
                    ),
                    None => (
                        base_branch.as_str(),
                        crate::diff::key_against_merge_base(worktree_path, base_branch)
                            .map_err(|error| error.to_string())?,
                    ),
                };
                format!("run\0{}\0{}", base, delta_key)
            }
            _ => return Ok(None),
        };
        Ok(Some(sha256_hex(material.as_bytes())))
    }

    pub(in crate::app) fn render(&self) -> Result<Value, String> {
        match self {
            Self::ProjectList { projects } => {
                let projects = projects
                    .iter()
                    .map(ProjectListRow::render)
                    .collect::<Vec<_>>();
                Ok(json!({ "projects": projects }))
            }
            Self::Project {
                project_id,
                repo_path,
            } => {
                let branch = git2::Repository::open(repo_path)
                    .ok()
                    .and_then(|repo| {
                        repo.head()
                            .ok()
                            .and_then(|head| head.shorthand().map(str::to_string))
                    })
                    .unwrap_or_else(|| "HEAD".to_string());
                let diff =
                    crate::diff::diff_against_head(repo_path).map_err(|error| error.to_string())?;
                Ok(json!({
                    "project_id": project_id,
                    "branch": branch,
                    "path": repo_path.display().to_string(),
                    "stat": diff.stat().to_json(),
                    "files": diff_file_rows(&diff),
                    "patch": diff.patch(),
                }))
            }
            Self::Worktree {
                external,
                base_branch,
            } => {
                let diff = crate::diff::diff_against_merge_base(&external.path, base_branch)
                    .map_err(|error| error.to_string())?;
                let adoptable = external
                    .branch
                    .as_deref()
                    .is_some_and(|branch| branch != base_branch);
                Ok(json!({
                    "worktree_id": external.id,
                    "branch": external.branch,
                    // The branch this diff is anchored on, so the surface can
                    // name it instead of saying "the base branch".
                    "base_branch": base_branch,
                    "head_subject": external.head_subject,
                    "dirty_files": external.dirty_files,
                    "path": external.path.display().to_string(),
                    "adoptable": adoptable,
                    "stat": diff.stat().to_json(),
                    "files": diff_file_rows(&diff),
                    "file_edited_at": diff_file_edited_at(&external.path, &diff),
                    "patch": diff.patch(),
                }))
            }
            Self::Run {
                worktree_path,
                base_sha,
                base_branch,
            } => {
                let diff = match base_sha {
                    Some(sha) => crate::diff::diff_against_base(worktree_path, sha),
                    None => crate::diff::diff_against_merge_base(worktree_path, base_branch),
                }
                .map_err(|error| error.to_string())?;
                Ok(worktree_diff_json(worktree_path, &diff))
            }
            Self::Stage {
                run_id,
                stage_id,
                object_database,
                start_sha,
                completion_sha,
            } => {
                let diff =
                    crate::diff::diff_between_commits(object_database, start_sha, completion_sha)
                        .map_err(|error| format!("stage diff unavailable: {error}"))?;
                let mut value = diff_json(&diff);
                let object = value.as_object_mut().expect("diff_json returns an object");
                object.insert("run_id".to_string(), json!(run_id));
                object.insert("stage_id".to_string(), json!(stage_id));
                object.insert("status".to_string(), json!("available"));
                object.insert("start_sha".to_string(), json!(start_sha));
                object.insert("completion_sha".to_string(), json!(completion_sha));
                Ok(value)
            }
        }
    }
}

pub(in crate::app) struct ProjectListRow {
    pub(in crate::app) project_id: String,
    pub(in crate::app) name: String,
    pub(in crate::app) repo_path: std::path::PathBuf,
    pub(in crate::app) worktrees_root: std::path::PathBuf,
    pub(in crate::app) base_branch: String,
    pub(in crate::app) is_git: bool,
    pub(in crate::app) sources: Vec<crate::app::projects::ProjectSource>,
    pub(in crate::app) isolation: Option<Isolation>,
    pub(in crate::app) isolation_default: Isolation,
}

impl ProjectListRow {
    pub(in crate::app) fn render(&self) -> Value {
        let available = IsolationAvailability::of(&self.repo_path, &self.worktrees_root);
        let requested = self.isolation.unwrap_or(self.isolation_default);
        let effective = if available.lock_reason(requested).is_none() {
            requested
        } else {
            Isolation::default()
        };
        json!({
            "project_id": self.project_id,
            "name": self.name,
            "path": self.repo_path.display().to_string(),
            "base_branch": self.base_branch,
            "is_git": self.is_git,
            "remote": self.is_git.then(|| git_remote_origin(&self.repo_path)).flatten(),
            "sources": self.sources.iter().enumerate().map(|(index, source)| json!({
                "id": source.id,
                "name": source.name,
                "mount": source.mount,
                "path": source.path.display().to_string(),
                "is_git": source.is_git,
                "base_branch": source.base_branch,
                "remote": source.remote.clone().or_else(|| (index == 0 && source.is_git).then(|| git_remote_origin(&source.path)).flatten()),
            })).collect::<Vec<_>>(),
            "isolation": self.isolation,
            "isolation_default": self.isolation_default,
            "isolation_effective": effective,
            "isolation_available": available,
        })
    }
}

/// Run a claimed job on the runtime, off every lock, and apply it under the
/// lock when it has decided.
///
/// `spawn_blocking` on purpose: the decide phase is libgit2 walking a worktree
/// or a bounded fetch, and it must not sit on a runtime worker the relay's read
/// loop needs. Returns the job back when there is no runtime to spawn onto
/// (the synchronous unit tests), so the caller can decide what to do with it.
pub(in crate::app) fn spawn_off_lock<J: OffLockJob>(
    state: Arc<Mutex<AppState>>,
    mut job: J,
) -> Result<(), J> {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(job);
    };
    runtime.spawn(async move {
        let claim = job.claim();
        let decided = tokio::task::spawn_blocking(move || job.decide()).await;
        let mut app = state.lock().unwrap();
        match decided {
            Ok(decided) => J::apply(&mut app, claim, decided),
            Err(_) => J::abandon(&mut app, claim),
        }
    });
    Ok(())
}

impl AppState {
    /// Run a job whose claim the caller has just taken: on the runtime with
    /// the lock released, or — with no runtime and no shared handle to apply
    /// through — as [`decide_without_a_runtime`](Self::decide_without_a_runtime)
    /// has it. The one place "decide off the lock, apply under it" is written.
    pub(in crate::app) fn run_off_lock<J: OffLockJob>(&mut self, job: J) {
        let spawned = match self.self_handle.as_ref().and_then(std::sync::Weak::upgrade) {
            Some(shared) => spawn_off_lock(shared, job),
            // No shared handle: nothing could apply what a thread decided.
            None => Err(job),
        };
        if let Err(unspawned) = spawned {
            self.decide_without_a_runtime(unspawned);
        }
    }

    /// What a claimed job does when there is no runtime to carry it.
    ///
    /// In production that means the daemon is shutting down or was built
    /// unrooted: the claim goes straight back and the next caller tries again.
    /// The synchronous tests have no runtime and no mutex — nobody is waiting
    /// on this thread — so there the job decides here, and the read that
    /// claimed it is answered from what it found.
    pub(in crate::app) fn decide_without_a_runtime<J: OffLockJob>(&mut self, mut job: J) {
        let claim = job.claim();
        #[cfg(test)]
        J::apply(self, claim, job.decide());
        #[cfg(not(test))]
        {
            let _ = job;
            J::abandon(self, claim);
        }
    }

    /// Dispatch without draining: a verb that handed its git work to
    /// [`AppState::deferred_work`] hands it back out HERE, to a caller that
    /// can release the app mutex before running it. The `Ok` returned
    /// alongside a deferral is the placeholder that field documents.
    pub(in crate::app) fn dispatch_deferring(
        &mut self,
        method: &str,
        params: &Value,
    ) -> (Result<Value, String>, Option<DeferredWork>) {
        let queued_before = self.delivery_queue.checkpoint();
        let outcome = self.route(method, params);
        if outcome.is_err() {
            self.drop_turns_queued_since(queued_before);
        }
        match self.deferred_work.take() {
            // Nothing is settled until the git work returns, so the stamp waits
            // for `apply_deferred` too.
            Some(deferred) => (outcome, Some(deferred)),
            None => {
                if let Ok(result) = &outcome {
                    // Only a verb that SUCCEEDED counts: a rejected action never happened.
                    self.stamp_interaction_for(method, params, result);
                }
                (outcome, None)
            }
        }
    }

    /// Write back what the lock-free git work found, and stamp the verb that
    /// deferred it — the second half of [`AppState::dispatch_deferring`].
    pub(in crate::app) fn apply_deferred(
        &mut self,
        method: &str,
        params: &Value,
        done: DeferredOutcome,
    ) -> Result<Value, String> {
        // Whether the git that just ran off-lock CHANGED anything. A read
        // deferred its work to keep the mutex free and writes nothing back, so
        // nothing about it is worth telling a browser; a mutating git verb
        // moved the tree every diff surface is showing.
        let mutating = match &done {
            DeferredOutcome::Lifecycle(_) => true,
            DeferredOutcome::Git { git, .. } => git.invalidates,
            DeferredOutcome::Read(_) => false,
        };
        let queued_before = self.delivery_queue.checkpoint();
        let applied = match done {
            DeferredOutcome::Lifecycle(outcome) => self.apply_lifecycle(*outcome),
            DeferredOutcome::Git { git, result } => self.apply_git(&git, result),
            // A read writes nothing back: its answer is the whole result.
            DeferredOutcome::Read(result) => result,
        };
        match &applied {
            Ok(result) => {
                self.stamp_interaction_for(method, params, result);
                // HERE, not before the drain: the decide half only claimed the
                // checkout, and a browser told to refetch then would have read
                // the state this write-back is about to replace.
                if mutating {
                    for entity_id in entity_ids_of(params, result) {
                        self.note_entity_changed(&entity_id);
                    }
                    self.note_board_changed();
                }
            }
            Err(_) => self.drop_turns_queued_since(queued_before),
        }
        applied
    }

    /// Forget what a failed request queued for an agent. A turn is not
    /// deliverable until the mutation that queued it is durable, and only this
    /// request's turns are dropped: a later harmless verb's drain would
    /// otherwise deliver work that nothing was ever written down for. What was
    /// written down before the refusal stays — see
    /// [`PendingAgentTurn::survives_refusal`].
    ///
    /// The two halves of a request both end here — [`dispatch_deferring`] for
    /// what refused before the git ran, [`apply_deferred`] for what failed
    /// writing the git down — so every drain in the daemon, frame, MCP control
    /// socket and test twin alike, inherits the rule.
    ///
    /// The queue can be SHORTER than it was measured: a request that retires
    /// an agent drops that agent's turns however early they were queued, and
    /// a refusal after that must find nothing of its own left, not a panic.
    ///
    /// [`dispatch_deferring`]: AppState::dispatch_deferring
    /// [`apply_deferred`]: AppState::apply_deferred
    pub(in crate::app) fn drop_turns_queued_since(
        &mut self,
        queued_before: crate::app::runtime::delivery::queue::DeliveryCheckpoint,
    ) {
        self.delivery_queue.refuse_since(queued_before);
    }

    /// Hand a resolved diff to the drain, which renders it with the mutex
    /// released. The `Value` returned is the placeholder
    /// [`AppState::deferred_work`] documents.
    pub(in crate::app) fn defer_read(
        &mut self,
        subject: ReadSubject,
        issue_id: Option<String>,
    ) -> Value {
        self.defer_conditional_read(subject, issue_id, None)
    }

    pub(in crate::app) fn defer_conditional_read(
        &mut self,
        subject: ReadSubject,
        issue_id: Option<String>,
        if_diff_key: Option<&str>,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject,
            issue_id,
            if_diff_key: if_diff_key.map(str::to_string),
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Value::Null
    }

    /// Put a placeholder on the board for a verb that is about to run git, and
    /// refuse a second verb claiming the same thing while it stands. The row is
    /// visible to every reader from this acquisition until the epilogue
    /// replaces it with the real record.
    ///
    /// Reached only through [`AppState::defer_lifecycle`], which is what makes
    /// the row's release certain: only a job can release one, so a row is never
    /// reserved without one.
    pub(in crate::app) fn reserve_row(
        &mut self,
        row: PendingRow,
    ) -> Result<Arc<PendingRow>, String> {
        if let Some(held) = self.row_claiming(&row) {
            return Err(format!(
                "{:?} is already {} — wait for that to finish",
                held.title,
                held.state.as_str()
            ));
        }
        let row = Arc::new(row);
        self.pending_rows.push(Arc::clone(&row));
        self.note_board_changed();
        Ok(row)
    }

    /// The standing row that already claims what `row` would: the same record,
    /// branch, checkout or issue in the same project. One rule for what two
    /// lifecycle verbs collide on.
    pub(in crate::app) fn row_claiming(&self, row: &PendingRow) -> Option<&Arc<PendingRow>> {
        self.pending_rows.iter().find(|held| {
            held.project_id == row.project_id
                && (held.entity_id == row.entity_id
                    || (held.branch.is_some() && held.branch == row.branch)
                    || (held.checkout_id.is_some() && held.checkout_id == row.checkout_id)
                    || (held.implements.is_some() && held.implements == row.implements))
        })
    }

    /// Retire a placeholder, whichever way its verb went. The real record — or
    /// nothing at all, on a failure — stands where it was.
    pub(in crate::app) fn release_row(&mut self, entity_id: &str) {
        self.pending_rows.retain(|row| row.entity_id != entity_id);
        self.note_board_changed();
    }

    /// The lifecycle verbs in flight, as rows the board shows beside the
    /// checkouts that already exist.
    ///
    /// Only the rows that stand for a card: a project verb reserves the folder
    /// it is reaching for, and a folder is not something the board lists, so
    /// nothing about it belongs in a list of cards.
    pub(in crate::app) fn pending_rows_json(&self) -> Vec<Value> {
        self.pending_rows
            .iter()
            .filter_map(|row| {
                let project_id = row.project_id.as_ref()?;
                Some(json!({
                    "entity_id": row.entity_id,
                    "project_id": project_id,
                    "project": self.project_name_by_id(project_id),
                    "title": row.title,
                    "branch": row.branch,
                    "state": row.state.as_str(),
                    "checkout_id": row.checkout_id,
                    // The project's own checkout is listed under no id of its
                    // own, so a row standing on it is matched by this instead.
                    "primary": row.primary,
                    "implements": row.implements,
                    // How the checkout being made is isolated, said the way a
                    // settled card says it. A verb that makes none says
                    // nothing: what is already on disk describes itself.
                    "isolation": row.isolation.map(crate::isolation::Isolation::wire),
                    // How long this row has stood. A row older than a scan
                    // interval reads as stuck rather than as work in flight.
                    "pending_seconds": row.since.elapsed().as_secs(),
                }))
            })
            .collect()
    }

    /// Hand one reserved job to the drain, which runs it with the app mutex
    /// released. The `Value` is the placeholder [`AppState::deferred_work`]
    /// documents: whichever drain runs the job replaces it with what
    /// [`AppState::apply_lifecycle`] answers.
    pub(in crate::app) fn defer_job(&mut self, job: WorktreeLifecycleJob) -> Value {
        self.deferred_work = Some(DeferredWork::Lifecycle(Box::new(job)));
        Value::Null
    }

    /// Move what one mutation did to the checkouts on disk into the list the
    /// board reads: what appeared, what went, and — when the mutation touched a
    /// checkout it could not describe — the rescan that finds it.
    ///
    /// A project verb has no project to amend and moves no checkout, so there
    /// is nothing here for it to do.
    pub(in crate::app) fn amend_checkouts(
        &mut self,
        project_id: Option<&str>,
        change: &WorktreeChange,
    ) {
        let Some(project_id) = project_id else {
            return;
        };
        for worktree in &change.appeared {
            self.note_worktree_appeared(project_id, worktree.clone());
        }
        for path in &change.gone {
            self.note_worktree_gone(project_id, path);
        }
        if change.rescan {
            self.rescan_external_worktrees(project_id);
        }
    }

    /// The feed moved: task lifecycle, inbox/attention, capture, agent
    /// liveness. Queues only — the send happens with this mutex released.
    pub(in crate::app) fn note_board_changed(&self) {
        self.changes.note_board();
    }

    /// One entity's detail moved: its thread, stages, git state or diff. The
    /// feed shows a row for it, so this stales that too.
    pub(in crate::app) fn note_entity_changed(&self, entity_id: &str) {
        self.changes.note_entity(entity_id);
    }

    /// The same, from an origin that fires on every file an agent writes: the
    /// entity's event is paced at [`crate::changes::ENTITY_SETTLE_WINDOW`].
    pub(in crate::app) fn note_entity_settled(&self, entity_id: &str) {
        self.changes.note_entity_settled(entity_id);
    }
}
