use crate::api::ApiError;
use crate::app::runtime::lifecycle::{LifecycleOutcome, WorktreeLifecycleJob};
use crate::app::{
    diff_file_edited_at, diff_file_rows, diff_json, entity_ids_of, off_the_workers, sha256_hex,
    worktree_diff_json, AppState, DeferredGit,
};
use crate::isolation::{Isolation, IsolationAvailability};
use crate::lifecycle::{PendingRow, WorktreeChange};
use crate::worktree::ExternalWorktree;
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
    /// A subscribe or unsubscribe: reconcile the worktree watchers against
    /// the new coverage, then answer with the watch state that produced.
    Watch(Box<DeferredWatch>),
    /// A read of something outside the state entirely — another program's
    /// answer, such as `gh`'s — whose answer is the whole result.
    External(Box<dyn FnOnce() -> Result<Value, String> + Send>),
}

/// How the drain holds a deferred reply to the result type `api/v1` declares
/// for the verb that deferred it. Built from `serde_json::from_value::<R>` at
/// dispatch time, when `R` is still known.
pub(crate) type DeferredResultCheck = fn(&Value) -> Result<(), String>;

/// What a drain does after writing one stage back.
pub(in crate::app) enum DeferredNext {
    /// The verb's answer.
    Answered(Result<Value, String>),
    /// Another stage to run off the lock.
    Again(DeferredJob),
}

/// One verb's deferred work, with the type check its answer owes.
///
/// The check rides ALONG with the work rather than being looked up when the
/// reply lands: the app mutex is released while the work runs, and by then
/// the state carries whatever the next frame put on it.
pub(in crate::app) struct DeferredJob {
    work: DeferredWork,
    check: Option<DeferredResultCheck>,
}

impl DeferredJob {
    /// The lock-free phase, keeping the check for the write-back.
    pub(in crate::app) fn run(self) -> DeferredDone {
        DeferredDone {
            outcome: self.work.run(),
            check: self.check,
        }
    }
}

/// What [`DeferredJob::run`] brought back: the outcome to write down, and the
/// check the published value must pass.
pub(in crate::app) struct DeferredDone {
    outcome: DeferredOutcome,
    check: Option<DeferredResultCheck>,
}

#[cfg(test)]
impl DeferredDone {
    /// Answer something else than the implementation did — how a test stands
    /// in for an implementation whose shape has drifted from the type
    /// `api/v1` declares for it.
    pub(in crate::app) fn answer_instead(&mut self, value: Value) {
        match &mut self.outcome {
            DeferredOutcome::Git { result, .. } | DeferredOutcome::Read(result) => {
                *result = Ok(value);
            }
            DeferredOutcome::Watch(reply) => *reply = value,
            DeferredOutcome::Lifecycle(_) => {
                panic!("only a git verb, a read or a watch answers a value of its own")
            }
        }
    }
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
            Self::Watch(watch) => DeferredOutcome::Watch(watch.run()),
            Self::External(read) => DeferredOutcome::Read(read()),
        }
    }
}

/// The lock-free half of `changes.subscribe` / `changes.unsubscribe`: the bus
/// already holds the new subscription set; what is left is starting or
/// dropping watchers, which walks trees and so runs here, and the reply,
/// which can only say `live` or `polled` once that has happened.
pub(in crate::app) struct DeferredWatch {
    pub(in crate::app) watchers: Arc<crate::app::watchers::WorktreeWatchers>,
    pub(in crate::app) bus: Arc<crate::changes::ChangeBus>,
    pub(in crate::app) answer: WatchAnswer,
}

/// What the verb answers once the watchers are reconciled.
pub(crate) enum WatchAnswer {
    /// `changes.subscribe`: the subscription as stored, whose `watch` is read
    /// off the bus after the reconcile.
    Subscribed(crate::changes::SubscriptionSpec),
    /// `changes.unsubscribe`: `{"ok": true}`.
    Unsubscribed,
}

impl DeferredWatch {
    pub(in crate::app) fn run(&self) -> Value {
        self.watchers.reconcile(&self.bus);
        match &self.answer {
            WatchAnswer::Subscribed(spec) => json!({
                "subscription_id": spec.id,
                "watch": self.bus.watch_state(spec),
            }),
            WatchAnswer::Unsubscribed => json!({ "ok": true }),
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
    /// The subscribe/unsubscribe reply, watchers reconciled.
    Watch(Value),
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
    /// The task that asked, when the read came in through a task surface —
    /// stamped onto the answer, as the task verbs did before the split.
    pub(in crate::app) task_id: Option<String>,
    /// The complete aggregate held by the caller. We still recompute to avoid
    /// stale filesystem answers, then suppress the equal payload on the wire.
    pub(in crate::app) if_diff_key: Option<String>,
    /// Whether the rendered answer keeps its patch text. A caller filling a
    /// cache asks for the shape without it: the stat, the per-file rows and
    /// the key that names the body it can ask for later.
    pub(in crate::app) with_patch: bool,
    /// The paths this read covers, or `None` for the whole changeset. A
    /// reader with one file open asks for that file: same changeset, same
    /// key, none of the hunks behind the files nobody opened.
    pub(in crate::app) paths: Option<Vec<String>>,
    /// One page of a narrowed read's patch (#95), or `None` for all of it.
    pub(in crate::app) range: Option<crate::body_page::BodyRange>,
    #[cfg(test)]
    pub(in crate::app) gate: Option<OffLockGate>,
}

/// What a narrowed read answers, whichever kind of checkout it came off.
///
/// A whole-changeset read carries the checkout's own identity beside the diff
/// — which worktree, which branch it is anchored on, where it is on disk —
/// because the surface opening it is about that checkout. A read of one
/// opened file is about the file: the rows, the hunks, and the key that says
/// whether they still stand. So the subject's own fields are dropped rather
/// than answered inconsistently across scopes.
const CHANGESET_FIELDS: [&str; 5] = ["stat", "files", "patch", "file_edited_at", "diff_key"];

/// Whether this read's caller wants the patch text. Absent means yes: a client
/// that has not heard of the flag is answered exactly as it always was.
pub(in crate::app) fn wants_patch(params: &Value) -> bool {
    params.get("patch").and_then(Value::as_bool).unwrap_or(true)
}

impl DeferredRead {
    pub(in crate::app) fn run(&self) -> Result<Value, String> {
        let conditional_key = self.subject.conditional_key()?;
        if let Some(diff_key) = conditional_key.as_deref() {
            if self.if_diff_key.as_deref() == Some(diff_key) {
                return Ok(json!({ "unchanged": true, "diff_key": diff_key }));
            }
        }
        let paths = match &self.paths {
            Some(paths) => crate::diff::DiffPaths::Only(paths),
            None => crate::diff::DiffPaths::All,
        };
        let mut rendered = self.subject.render(paths)?;
        if let (Some(task_id), Some(object)) = (&self.task_id, rendered.as_object_mut()) {
            object.insert("task_id".to_string(), json!(task_id));
        }
        if let Some(diff_key) = conditional_key {
            if let Some(object) = rendered.as_object_mut() {
                object.insert("diff_key".to_string(), json!(diff_key));
            }
        }
        if !self.with_patch {
            if let Some(object) = rendered.as_object_mut() {
                object.remove("patch");
            }
        }
        if self.paths.is_some() {
            if let Some(object) = rendered.as_object_mut() {
                object.retain(|field, _| CHANGESET_FIELDS.contains(&field.as_str()));
            }
        }
        if let Some(range) = self.range {
            page_the_patch(&mut rendered, range)?;
        }
        Ok(rendered)
    }
}

/// Cut a rendered answer's patch down to one page of it, and say where the
/// page sits.
fn page_the_patch(rendered: &mut Value, range: crate::body_page::BodyRange) -> Result<(), String> {
    let patch = rendered.get("patch").and_then(Value::as_str).unwrap_or("");
    let (page, span) = crate::body_page::text_page(patch, range);
    rendered["patch"] = json!(page);
    rendered["range"] = json!(span);
    Ok(())
}

/// Which diff a deferred read renders.
pub(in crate::app) enum ReadSubject {
    /// `project.list` — immutable row inputs captured at request time, with
    /// repository and volume metadata read while the app mutex is released.
    ProjectList { projects: Vec<ProjectListRow> },
    /// The project repository's uncommitted work.
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
    /// The unpublished changeset of one checkout — everything its push
    /// destination does not have. What a workspace source's "All changes" is,
    /// and the only changeset whose base is a publication rather than a branch.
    Unpublished { repo_path: std::path::PathBuf },
    /// One immutable stage boundary, sha to sha.
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
            // Already a key of the whole delta, and the one `git.unpushed`
            // answers with — a narrowed read has to agree with the list read
            // that sent the reader here.
            Self::Unpublished { repo_path } => {
                return crate::gitgui::unpushed_key(repo_path).map(Some)
            }
            _ => return Ok(None),
        };
        Ok(Some(sha256_hex(material.as_bytes())))
    }

    pub(in crate::app) fn render(
        &self,
        paths: crate::diff::DiffPaths<'_>,
    ) -> Result<Value, String> {
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
                let diff = crate::diff::diff_against_head_for(repo_path, paths)
                    .map_err(|error| error.to_string())?;
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
                let diff =
                    crate::diff::diff_against_merge_base_for(&external.path, base_branch, paths)
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
                    Some(sha) => crate::diff::diff_against_base_for(worktree_path, sha, paths),
                    None => {
                        crate::diff::diff_against_merge_base_for(worktree_path, base_branch, paths)
                    }
                }
                .map_err(|error| error.to_string())?;
                Ok(worktree_diff_json(worktree_path, &diff))
            }
            Self::Unpublished { repo_path } => {
                let crate::diff::DiffPaths::Only(paths) = paths else {
                    // The whole unpublished changeset is `git.unpushed`'s to
                    // answer: it carries the base and the commit list too, and
                    // this subject exists for the narrowed read alone.
                    return Err("git.changeset_diff requires paths".to_string());
                };
                crate::gitgui::unpushed_file_diff(repo_path, paths)
            }
            Self::Stage {
                run_id,
                stage_id,
                object_database,
                start_sha,
                completion_sha,
            } => {
                let diff = crate::diff::diff_between_commits(
                    object_database,
                    start_sha,
                    completion_sha,
                    paths,
                )
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
    /// The conversation owner this project has, or `None` for one nobody has
    /// talked to yet. Read under the lock with the rest of the row: a list is a
    /// read, and a read never mints an owner.
    pub(in crate::app) conversation: Option<String>,
    pub(in crate::app) conversations: Vec<Value>,
    pub(in crate::app) session: crate::session_summary::SessionSummary,
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
        let sources = self
            .sources
            .iter()
            .map(crate::app::projects::ProjectSource::wire)
            .collect::<Vec<_>>();
        json!({
            "project_id": self.project_id,
            "name": self.name,
            "path": self.repo_path.display().to_string(),
            "base_branch": self.base_branch,
            "is_git": self.is_git,
            "remote": crate::app::projects::primary_remote(&sources),
            "sources": sources,
            "isolation": self.isolation,
            "isolation_default": self.isolation_default,
            "isolation_effective": effective,
            "isolation_available": available,
            "entity_id": self.conversation,
            "run_id": self.conversation,
            "conversations": self.conversations,
            "session_started_ms": self.session.session_started_ms,
            "last_activity_ms": self.session.last_activity_ms,
        })
    }
}

/// Run a claimed job on the runtime, off every lock, and apply it under the
/// lock when it has decided — waiting for that lock on the blocking pool too.
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
        off_the_workers(move || {
            let mut app = state.lock().unwrap();
            match decided {
                Ok(decided) => J::apply(&mut app, claim, decided),
                Err(_) => J::abandon(&mut app, claim),
            }
        })
        .await;
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
    ) -> (Result<Value, ApiError>, Option<DeferredJob>) {
        let queued_before = self.delivery_queue.checkpoint();
        let outcome = self.route(method, params);
        if outcome.is_err() {
            self.drop_turns_queued_since(queued_before);
        }
        match self.take_deferred() {
            // Nothing is settled until the git work returns, so the stamp waits
            // for `apply_deferred` too.
            Some(deferred) => (outcome, Some(deferred)),
            None => {
                if outcome.is_ok() {
                    // Only a verb that SUCCEEDED counts: a rejected action never happened.
                    self.stamp_interaction_for(method, params);
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
        done: DeferredDone,
    ) -> Result<Value, String> {
        self.active_deferred_filesystem_jobs -= 1;
        let DeferredDone {
            outcome: done,
            check,
        } = done;
        // Whether the git that just ran off-lock CHANGED anything. A read
        // deferred its work to keep the mutex free and writes nothing back, so
        // nothing about it is worth telling a browser; a mutating git verb
        // moved the tree every diff surface is showing.
        let mutating = match &done {
            DeferredOutcome::Lifecycle(_) => true,
            DeferredOutcome::Git { git, .. } => git.invalidates,
            DeferredOutcome::Read(_) => false,
            DeferredOutcome::Watch(_) => false,
        };
        let queued_before = self.delivery_queue.checkpoint();
        let applied = match done {
            DeferredOutcome::Lifecycle(outcome) => self.apply_lifecycle(*outcome),
            DeferredOutcome::Git { git, result } => self.apply_git(&git, result),
            // A read writes nothing back: its answer is the whole result.
            DeferredOutcome::Read(result) => result,
            DeferredOutcome::Watch(reply) => Ok(reply),
        };
        match &applied {
            Ok(result) => {
                self.stamp_interaction_for(method, params);
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
        // LAST, and deliberately after the write-back: the git ran and the
        // state it moved is written down whatever shape the value took, so
        // only the reply is refused. A mismatch is this bridge's own bug —
        // an implementation that drifted from the type `api/v1` declares for
        // its verb — and reads as `internal` to the client.
        Self::checked_reply(method, check, applied)
    }

    /// [`AppState::apply_deferred`], and whatever the write-back handed on.
    ///
    /// A verb can take more than one trip off the lock: `workspace.reclaim`
    /// measures Git off it, decides under it, and only then hands the removal
    /// to the drain. Every drain runs the next stage the same way it ran the
    /// first, until one answers.
    pub(in crate::app) fn apply_deferred_stage(
        &mut self,
        method: &str,
        params: &Value,
        done: DeferredDone,
    ) -> DeferredNext {
        let applied = self.apply_deferred(method, params, done);
        if applied.is_err() {
            // A refused write-back hands nothing on.
            self.deferred_work = None;
            self.deferred_result_check = None;
            return DeferredNext::Answered(applied);
        }
        match self.take_deferred() {
            Some(next) => DeferredNext::Again(next),
            None => DeferredNext::Answered(applied),
        }
    }

    /// Hold a deferred reply to the result type its verb declares. Runs in
    /// release builds too: it is one deserialise per deferred reply, and the
    /// verbs that answer this way — every `git.*` and every diff read — are
    /// exactly the ones the facade's own check never sees.
    fn checked_reply(
        method: &str,
        check: Option<DeferredResultCheck>,
        applied: Result<Value, String>,
    ) -> Result<Value, String> {
        let (Some(check), Ok(result)) = (check, &applied) else {
            return applied;
        };
        match check(result) {
            Ok(()) => applied,
            Err(error) => Err(format!(
                "{method}: the deferred reply does not match the type api/v1 declares for it: {error}"
            )),
        }
    }

    /// Take the work a verb deferred, with the type check `api/v1` attached
    /// to it. Any check left behind by a verb that did NOT defer goes with
    /// it, so nothing can be checked against the wrong verb's type.
    pub(in crate::app) fn take_deferred(&mut self) -> Option<DeferredJob> {
        let check = self.deferred_result_check.take();
        let work = self.deferred_work.take()?;
        // Claim before either the relay or MCP drain releases the mutex.
        // All variants can observe filesystem state that deletion would remove.
        self.active_deferred_filesystem_jobs += 1;
        Some(DeferredJob { work, check })
    }

    /// Name the result type the verb just dispatched declares, for the work
    /// it deferred. Called by [`crate::api::v1::dispatch`] the moment the
    /// handler returns, while `R` is still known; a verb that deferred
    /// nothing has nothing to check.
    pub(crate) fn expect_deferred_result(&mut self, check: DeferredResultCheck) {
        if self.deferred_work.is_some() {
            self.deferred_result_check = Some(check);
        }
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
        task_id: Option<String>,
    ) -> Value {
        self.defer_conditional_read(subject, task_id, None, true)
    }

    /// Hand the drain one changeset narrowed to the paths a reader has open.
    /// The key is the whole changeset's, because that is what says whether the
    /// body still stands; only the hunks are narrowed.
    pub(in crate::app) fn defer_narrowed_read(
        &mut self,
        subject: ReadSubject,
        if_diff_key: Option<&str>,
        paths: Vec<String>,
        range: Option<crate::body_page::BodyRange>,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject,
            task_id: None,
            if_diff_key: if_diff_key.map(str::to_string),
            with_patch: true,
            paths: Some(paths),
            range,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Value::Null
    }

    /// Hand a changed subscription set to the drain, which reconciles the
    /// worktree watchers with the mutex released and answers from the result.
    /// The roots snapshot is refreshed here so the reconcile sees the board
    /// as this verb saw it. The `Value` returned is the placeholder
    /// [`AppState::deferred_work`] documents.
    pub(crate) fn defer_watch(&mut self, answer: WatchAnswer) -> Value {
        self.watchers.set_roots(self.worktree_roots());
        self.deferred_work = Some(DeferredWork::Watch(Box::new(DeferredWatch {
            watchers: Arc::clone(&self.watchers),
            bus: Arc::clone(&self.changes),
            answer,
        })));
        Value::Null
    }

    pub(in crate::app) fn defer_conditional_read(
        &mut self,
        subject: ReadSubject,
        task_id: Option<String>,
        if_diff_key: Option<&str>,
        with_patch: bool,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject,
            task_id,
            if_diff_key: if_diff_key.map(str::to_string),
            with_patch,
            paths: None,
            range: None,
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
    /// branch, checkout or task in the same project. One rule for what two
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
        self.note_board_lists_changed(crate::changes::BoardLists::default());
    }

    /// The feed moved, and so did one of the lists a client caches whole: the
    /// next board item carries that list in full. Every other board note
    /// moves rows, which ride their own items.
    pub(in crate::app) fn note_board_lists_changed(&self, lists: crate::changes::BoardLists) {
        self.changes.note_board_lists(lists);
        // An entity may have arrived with a checkout or left with one: the
        // watchers follow the board, off this mutex.
        self.watchers.board_moved(self.worktree_roots());
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

    /// A tab opened or closed in this checkout.
    ///
    /// Every board entity rooted there hears it: one directory can be both a
    /// run's worktree and the external checkout it was adopted from, and a
    /// client watching either is looking at the same tab row.
    pub(in crate::app) fn note_terminals_at(&self, root: &std::path::Path) {
        let root = Self::canonical_root(root);
        for (entity_id, path) in self.worktree_roots() {
            if Self::canonical_root(&path) == root {
                self.changes.note_terminals(&entity_id);
            }
        }
    }
}
