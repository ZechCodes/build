//! The workspace reclaim service, and `workspace.reclaim` (#135).
//!
//! The service is always-on logic, so it stands apart from the RPC and event
//! layer. It is a loop on its own task. Each sweep reads every workspace under
//! the app mutex, measures each one with the mutex released, and writes the
//! verdict under it. What it writes reaches clients the way every other change
//! does: the workspace rows carry the verdict as `lifecycle`, and the list is
//! noted changed.
//!
//! The bridge never removes a workspace by itself. A quiet workspace is
//! reported to the project agent as one notice per project, plus an entry on
//! each linked task's timeline. The agent merges it, deletes it or surfaces
//! it. `workspace.reclaim` is the explicit removal the agent or the user calls
//! (`reclaim/explicit.rs`).
//!
//! Dropping a quiet workspace's build output (tier 1) is off unless the
//! device's `workspace_prune` setting or `BRIDGE_WORKSPACE_PRUNE` turns it on;
//! the idle threshold is `workspace_idle_secs` or `BRIDGE_WORKSPACE_IDLE_SECS`,
//! the variable winning where it is set (#167). Each sweep reads them afresh. When it runs, the workspace is reserved
//! first: under the mutex, with every hold read fresh. While it is reserved no
//! agent turn is delivered in it, no terminal opens in it and nothing else
//! removes it, and no Git verb, file write or directory change starts in it.
//! Then its Git state and activity are measured again and its build output is
//! inspected, with the mutex released. A killable child reads the final Git
//! state and one fresh index per repository, validating candidates in a batch.
//! Under the mutex once more, holds, index metadata and candidate paths are
//! checked cheaply before build output moves into the workspace's trash. The
//! same final deadline bounds the child, checks and moves. The reservation
//! ends before trash is emptied off the mutex.

mod explicit;
mod notice;

use crate::app::{off_the_workers, AppState, DeliveryRunner};
use crate::reclaim::artifacts::{self, Artifact};
use crate::reclaim::containment::WorkspaceBoundary;
use crate::reclaim::{
    Budget, IndexSnapshot, LifecycleRecord, LinkedTask, NoticeDue, ReclaimPolicy, Subject,
};
use crate::tracker::{Actor, Task, TaskEventKind, TaskState};
use crate::workspace::{Workspace, WorkspaceStatus};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a nudge waits before sweeping, so a burst of task moves is one
/// sweep.
const NUDGE_SETTLE: Duration = Duration::from_secs(5);

/// A reservation older than this is treated as released. Everything done
/// under one is bounded well inside it (the measurement's budget, two
/// minutes, with its Git readings killed at the deadline, plus the last
/// look's five seconds), so this only frees a workspace whose reserving
/// thread died.
const RESERVATION_LIMIT: Duration = Duration::from_secs(15 * 60);

/// Where the service's own deliveries are charged on the frame clock.
const SWEEP_METHOD: &str = "workspace.reclaim_sweep";

/// Every task that links a workspace, by workspace id, or why the tasks
/// could not be read.
type LinkedTasks = Result<HashMap<String, Vec<LinkedTask>>, String>;

/// One workspace, reserved between measuring and removing.
pub(in crate::app) struct ReclaimReservation {
    /// Canonical, like every turn's and terminal's root.
    root: PathBuf,
    since: Instant,
}

impl ReclaimReservation {
    fn live(&self) -> bool {
        self.since.elapsed() < RESERVATION_LIMIT
    }
}

/// Whether `root` is inside one of these reserved workspaces. A free function
/// so the delivery queue can ask it while the queue itself is borrowed.
pub(in crate::app) fn reserved_holds(
    reserved: &HashMap<String, ReclaimReservation>,
    root: &Path,
) -> bool {
    reserved
        .values()
        .any(|reservation| reservation.live() && root.starts_with(&reservation.root))
}

/// Where a prune stands, for a test racing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PrunePhase {
    /// Reserved, before Git and activity are measured again.
    Reserved,
    /// Measured again and the build output inspected, before the last look
    /// and the move.
    Inspected,
    /// The final child has validated candidates, before the locked move.
    Validated,
}

/// A workspace the project agent is about to hear about.
struct Quiet {
    subject: Subject,
    record: LifecycleRecord,
    /// It has just gone quiet, rather than stayed quiet since the last notice.
    first: bool,
}

fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX)
}

#[cfg(test)]
struct StopCheckProbe {
    before: tokio::sync::oneshot::Sender<()>,
    proceed: tokio::sync::oneshot::Receiver<()>,
    after: tokio::sync::oneshot::Sender<()>,
}

#[cfg(test)]
fn stop_check_probes() -> &'static Mutex<HashMap<tokio::runtime::Id, StopCheckProbe>> {
    static PROBES: std::sync::OnceLock<Mutex<HashMap<tokio::runtime::Id, StopCheckProbe>>> =
        std::sync::OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pause the first sweep immediately before the loop checks its stop flag.
#[cfg(test)]
pub(in crate::app) fn probe_reclaim_stop_for_test() -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (before, reached) = tokio::sync::oneshot::channel();
    let (proceed, allowed) = tokio::sync::oneshot::channel();
    let (after, checked) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current().id();
    assert!(
        stop_check_probes()
            .lock()
            .unwrap()
            .insert(
                runtime,
                StopCheckProbe {
                    before,
                    proceed: allowed,
                    after
                }
            )
            .is_none(),
        "only one reclaim stop probe per test runtime"
    );
    (reached, proceed, checked)
}

impl AppState {
    /// Run the service for the life of the daemon: the first sweep a little
    /// after startup, then one every `sweep_every`, and one soon after a nudge.
    ///
    /// No runtime worker waits on the app mutex here (#131): the policy is
    /// set and the handles read on the blocking pool, each sweep runs there,
    /// and the loop checks the stop handle it already holds.
    pub async fn spawn_workspace_reclaim(
        state: Arc<Mutex<AppState>>,
        policy: ReclaimPolicy,
    ) -> Arc<AtomicBool> {
        #[cfg(test)]
        let mut stop_probe = stop_check_probes()
            .lock()
            .unwrap()
            .remove(&tokio::runtime::Handle::current().id());
        let (nudge, stop) = {
            let state = Arc::clone(&state);
            let policy = policy.clone();
            off_the_workers(move || {
                let mut app = state.lock().unwrap();
                app.reclaim_policy = policy;
                (app.reclaim_nudge.clone(), app.reclaim_stop.clone())
            })
            .await
        };
        let stopped = Arc::clone(&stop);
        tokio::spawn(async move {
            tokio::time::sleep(policy.first_sweep_after).await;
            loop {
                let swept = Arc::clone(&state);
                let sweep = tokio::task::spawn_blocking(move || {
                    // The device's settings as they stand now: a change in
                    // Settings takes effect on the next sweep.
                    let policy_now = swept.lock().unwrap().reclaim_policy_now();
                    AppState::sweep_workspaces(&swept, &policy_now, now_ms());
                    // The notices the sweep queued.
                    AppState::deliver_after_sweep(&swept);
                });
                if let Err(joined) = sweep.await {
                    eprintln!("workspace reclaim: sweep failed: {joined}");
                }
                #[cfg(test)]
                let after_check = if let Some(probe) = stop_probe.take() {
                    let _ = probe.before.send(());
                    let _ = probe.proceed.await;
                    Some(probe.after)
                } else {
                    None
                };
                let should_stop = stopped.load(Ordering::Relaxed);
                #[cfg(test)]
                if let Some(after) = after_check {
                    let _ = after.send(());
                }
                if should_stop {
                    return;
                }
                tokio::select! {
                    _ = tokio::time::sleep(policy.sweep_every) => {}
                    _ = nudge.notified() => tokio::time::sleep(NUDGE_SETTLE).await,
                }
            }
        });
        stop
    }

    /// Stop the service: a sweep still walking stops at its next entry, and
    /// no other starts.
    pub fn stop_workspace_reclaim(state: &Arc<Mutex<AppState>>) {
        let (stop, sizes) = {
            let app = state.lock().unwrap();
            (app.reclaim_stop.clone(), Arc::clone(&app.size_requests))
        };
        stop.store(true, Ordering::Relaxed);
        sizes.wake();
    }

    /// One sweep: read every workspace, measure each with the mutex released,
    /// then write what was found.
    pub fn sweep_workspaces(state: &Arc<Mutex<AppState>>, policy: &ReclaimPolicy, now_ms: i64) {
        Self::sweep_workspaces_racing(state, policy, now_ms, &|_| {});
    }

    /// [`Self::sweep_workspaces`], with `racer` run at each [`PrunePhase`] of
    /// a prune: where a test starts an agent, reopens a task or edits a
    /// file.
    pub(in crate::app) fn sweep_workspaces_racing(
        state: &Arc<Mutex<AppState>>,
        policy: &ReclaimPolicy,
        now_ms: i64,
        racer: &dyn Fn(PrunePhase),
    ) {
        let (subjects, stop) = {
            let app = state.lock().unwrap();
            (app.reclaim_subjects(), Arc::clone(&app.reclaim_stop))
        };
        let mut measured = Vec::new();
        for subject in subjects {
            let budget = policy.budget(Arc::clone(&stop));
            if budget.stopped() {
                break;
            }
            // Whatever an interrupted sweep left in the trash. Only ever what
            // Build moved there.
            if let Some(guard) = subject
                .boundary
                .as_ref()
                .and_then(|boundary| boundary.validate().ok())
            {
                guard.empty_trash(&budget);
            }
            let mut record = subject.measure(now_ms, policy, &budget);
            if policy.prune && record.idle && record.reclaimable {
                record = Self::prune_workspace(state, &subject, record, now_ms, policy, racer);
            }
            measured.push((subject, record));
        }
        state
            .lock()
            .unwrap()
            .settle_workspace_sweep(measured, now_ms, policy);
    }

    /// Drop a quiet workspace's build output, under a reservation, and answer
    /// the verdict as it stands afterwards.
    fn prune_workspace(
        state: &Arc<Mutex<AppState>>,
        subject: &Subject,
        record: LifecycleRecord,
        now_ms: i64,
        policy: &ReclaimPolicy,
        racer: &dyn Fn(PrunePhase),
    ) -> LifecycleRecord {
        if subject.canonical_root().is_none() {
            let mut record = record;
            record.unmeasured();
            return record;
        };
        let (stop, tasks) = {
            let mut app = state.lock().unwrap();
            match app.prune_holds(&subject.workspace_id, false) {
                Some((holds, tasks)) if holds.is_empty() => {
                    app.reserve_workspace(&subject.workspace_id);
                    (Arc::clone(&app.reclaim_stop), tasks)
                }
                _ => return record,
            }
        };
        // The holds and tasks as the reservation read them.
        let subject = Subject {
            holds: Vec::new(),
            tasks,
            ..subject.clone()
        };
        racer(PrunePhase::Reserved);
        // Measured again now that nothing Build starts can touch it: an edit,
        // a commit or a push since the first measurement counts.
        let budget = policy.budget(Arc::clone(&stop));
        let mut fresh = subject.measure(now_ms, policy, &budget);
        let artifacts = if fresh.idle && fresh.reclaimable {
            match subject.build_output(&budget) {
                Ok(artifacts) => artifacts,
                Err(_) => {
                    fresh.unmeasured();
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        racer(PrunePhase::Inspected);
        // The reservation stays live while the killable child validates the
        // final Git state and all candidates; RPCs can still take the mutex.
        let last_look = policy.final_check_budget(Arc::clone(&stop));
        let snapshots = if artifacts.is_empty() {
            if subject.canonical_root().is_none() {
                fresh.unmeasured();
            }
            None
        } else {
            subject.confirm_build_output(&mut fresh, &artifacts, policy, &last_look)
        };
        if snapshots.is_some() {
            racer(PrunePhase::Validated);
        }
        let moved = {
            let mut app = state.lock().unwrap();
            let moved = if app.spoken_to_since(&subject, &mut fresh) {
                Vec::new()
            } else if let Some(snapshots) = &snapshots {
                app.move_build_output(&subject, &artifacts, &mut fresh, snapshots, &last_look)
            } else {
                Vec::new()
            };
            app.release_reservation(&subject.workspace_id);
            moved
        };
        Self::deliver_after_sweep(state);
        if moved.is_empty() {
            return fresh;
        }
        if let Some(guard) = subject
            .boundary
            .as_ref()
            .and_then(|boundary| boundary.validate().ok())
        {
            guard.empty_trash(&policy.budget(Arc::clone(&stop)));
        }
        fresh.pruned_bytes += moved.iter().map(|artifact| artifact.bytes).sum::<u64>();
        fresh.pruned_at_ms = Some(now_ms);
        if !subject.resize(&mut fresh, &policy.budget(stop), now_ms) {
            fresh.unmeasured();
        }
        fresh
    }

    /// Only cheap checks remain under the mutex. The same deadline that
    /// bounded the child is checked inside each loop and each rename.
    fn move_build_output(
        &self,
        subject: &Subject,
        artifacts: &[Artifact],
        record: &mut LifecycleRecord,
        snapshots: &[IndexSnapshot],
        last_look: &Budget,
    ) -> Vec<Artifact> {
        if last_look.check().is_err() {
            record.unmeasured();
            return Vec::new();
        }
        match self.prune_holds(&subject.workspace_id, true) {
            Some((holds, _)) if holds.is_empty() => {}
            Some((holds, _)) => {
                record.hold(&holds);
                return Vec::new();
            }
            None => return Vec::new(),
        }
        for snapshot in snapshots {
            if last_look.check().is_err() || !snapshot.unchanged() {
                record.unmeasured();
                return Vec::new();
            }
        }
        for artifact in artifacts {
            if last_look.check().is_err() || !artifacts::still_candidate(artifact) {
                record.unmeasured();
                return Vec::new();
            }
        }
        let Some(guard) = subject
            .boundary
            .as_ref()
            .and_then(|boundary| boundary.validate().ok())
        else {
            record.unmeasured();
            return Vec::new();
        };
        let moved = guard.move_to_trash(artifacts, last_look);
        if last_look.check().is_err() {
            record.unmeasured();
        }
        moved
    }

    /// Whether somebody wrote to the workspace's conversation since `subject`
    /// was read. The turn it queued waits for the reservation, but the
    /// workspace is not quiet any more, and `record` says so.
    fn spoken_to_since(&self, subject: &Subject, record: &mut LifecycleRecord) -> bool {
        let Some(workspace) = self.workspaces.get(&subject.workspace_id) else {
            return true;
        };
        let latest = self.conversation_activity_of(workspace);
        if latest <= subject.conversation_activity_ms {
            return false;
        }
        record.last_activity_ms = record.last_activity_ms.max(latest);
        record.idle = false;
        true
    }

    /// Hand out the turns a reservation held and the notices a sweep queued.
    /// Only with a runtime to deliver on: the synchronous tests read the queue
    /// themselves.
    fn deliver_after_sweep(state: &Arc<Mutex<AppState>>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        DeliveryRunner::drain(state, &clock.frame(SWEEP_METHOD));
    }

    /// What a sweep runs under now: the service's policy, with the device's
    /// idle threshold and prune switch over it wherever the environment did
    /// not set them (#167).
    pub(in crate::app) fn reclaim_policy_now(&self) -> ReclaimPolicy {
        self.reclaim_policy.with_settings(&self.reclaim_settings)
    }

    /// Ask for a sweep soon: a task linked to a workspace just finished, so
    /// that workspace may have become reclaimable.
    pub(in crate::app) fn nudge_workspace_reclaim(&self) {
        self.reclaim_nudge.notify_one();
    }

    /// The last verdict on one workspace, as its row carries it. `null` before
    /// the first sweep has measured it or a size walk has sized it (#273).
    pub(in crate::app) fn workspace_lifecycle_json(&self, workspace_id: &str) -> Value {
        self.workspace_lifecycle
            .get(workspace_id)
            .map(|record| serde_json::to_value(record).unwrap_or(Value::Null))
            .unwrap_or(Value::Null)
    }

    /// Bring back what the service last concluded, so a restart does not
    /// announce every quiet workspace again.
    pub(in crate::app) fn restore_workspace_lifecycle(&mut self) -> Result<(), String> {
        if let Some(store) = self.store.as_ref() {
            self.workspace_lifecycle = store
                .load_workspace_lifecycle()
                .map_err(|error| format!("workspace lifecycle restore: {error}"))?;
        }
        Ok(())
    }

    // ------------------------------------------------------- reservation ---

    /// Whether `root` is inside a reserved workspace: a turn for an agent
    /// there waits, and a terminal does not open there.
    pub(in crate::app) fn reclaim_reserved_at(&self, root: &Path) -> bool {
        reserved_holds(&self.reclaim_reserved, root)
    }

    /// Whether this workspace is reserved.
    pub(in crate::app) fn workspace_reserved(&self, workspace_id: &str) -> bool {
        self.reclaim_reserved
            .get(workspace_id)
            .is_some_and(ReclaimReservation::live)
    }

    /// Refuse to start something that writes inside a reserved workspace: a
    /// terminal, a Git verb that changes the tree or its refs, a file write.
    /// `path` is resolved through its links first, like the reserved root,
    /// including a folder the write would make.
    pub(in crate::app) fn refuse_writers_while_reserved(&self, path: &Path) -> Result<(), String> {
        if self.reclaim_reserved_at(&crate::worktree::canonical_planned_path(path)) {
            return Err(crate::reclaim::RESERVED.to_string());
        }
        Ok(())
    }

    /// Reserve a workspace that is still here. Callers have read its holds
    /// under the same guard.
    fn reserve_workspace(&mut self, workspace_id: &str) {
        let Some(workspace) = self.workspaces.get(workspace_id) else {
            return;
        };
        let root = Self::canonical_root(&workspace.root);
        self.reclaim_reserved.insert(
            workspace_id.to_string(),
            ReclaimReservation {
                root,
                since: Instant::now(),
            },
        );
    }

    fn release_reservation(&mut self, workspace_id: &str) {
        self.reclaim_reserved.remove(workspace_id);
    }

    /// What keeps the service from dropping this workspace's build output right
    /// now, every hold read under the mutex and the linked tasks read again.
    /// `None` when it is not the service's to touch at all: gone, not Build's,
    /// reserved by somebody else, or other files are being moved. `reserved`
    /// says the caller holds the reservation.
    fn prune_holds(
        &self,
        workspace_id: &str,
        reserved: bool,
    ) -> Option<(Vec<&'static str>, Vec<LinkedTask>)> {
        let workspace = self.workspaces.get(workspace_id)?;
        let busy = self.project_deletion_in_progress
            || self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || (!reserved && self.workspace_reserved(workspace_id));
        if busy || self.refuse_removing_what_is_not_builds(workspace).is_err() {
            return None;
        }
        let tasks = self
            .tasks_linking_workspaces()
            .map(|mut linked| linked.remove(workspace_id).unwrap_or_default());
        let holds = self.live_holds(workspace, &tasks);
        Some((holds, tasks.unwrap_or_default()))
    }

    // ------------------------------------------------------------- sweep ---

    /// Every workspace Build made, with what can only be read under the mutex.
    /// Adopted checkouts are somebody else's and are not measured.
    fn reclaim_subjects(&self) -> Vec<Subject> {
        let linked = self.tasks_linking_workspaces();
        self.measured_workspaces(None)
            .map(|workspace| {
                let tasks = tasks_of(&linked, &workspace.id);
                self.reclaim_subject(workspace, tasks)
            })
            .collect()
    }

    /// The workspaces the service measures, of one project or of all:
    /// Build's own, not yet finished.
    pub(in crate::app) fn measured_workspaces(
        &self,
        project_id: Option<&str>,
    ) -> impl Iterator<Item = &Workspace> {
        self.workspaces
            .list(project_id)
            .into_iter()
            .filter(|workspace| workspace.managed && workspace.status != WorkspaceStatus::Finished)
    }

    fn reclaim_subject(
        &self,
        workspace: &Workspace,
        tasks: Result<Vec<LinkedTask>, String>,
    ) -> Subject {
        Subject {
            workspace_id: workspace.id.clone(),
            project_id: workspace.project_id.clone(),
            name: workspace.name.clone(),
            root: workspace.root.clone(),
            boundary: self.workspace_boundary(workspace),
            repositories: workspace
                .directories
                .iter()
                .filter(|directory| directory.is_git)
                .map(|directory| (directory.path.clone(), directory.branch.clone()))
                .collect(),
            holds: self.live_holds(workspace, &tasks),
            conversation_activity_ms: self.conversation_activity_of(workspace),
            previous: self.workspace_lifecycle.get(&workspace.id).cloned(),
            tasks: tasks.unwrap_or_default(),
        }
    }

    /// The managed-storage boundary a walk of this workspace holds to.
    pub(in crate::app) fn workspace_boundary(
        &self,
        workspace: &Workspace,
    ) -> Option<WorkspaceBoundary> {
        WorkspaceBoundary::new(
            self.workspaces.storage_anchor(),
            &workspace.root,
            workspace
                .directories
                .iter()
                .map(|directory| directory.path.clone())
                .collect(),
        )
    }

    /// The newest message in the workspace's conversation.
    fn conversation_activity_of(&self, workspace: &Workspace) -> Option<i64> {
        self.workspace_conversation_owner(workspace)
            .and_then(|owner| self.session_summary(&owner).last_activity_ms)
    }

    /// What holds a workspace that only the app state knows: it is not ready,
    /// an agent is working or a terminal is open anywhere inside it, it holds
    /// a plain directory, or its tasks are still being worked toward or
    /// could not be read.
    fn live_holds(
        &self,
        workspace: &Workspace,
        tasks: &Result<Vec<LinkedTask>, String>,
    ) -> Vec<&'static str> {
        let root = Self::canonical_root(&workspace.root);
        let checks = [
            (
                workspace.status != WorkspaceStatus::Ready,
                crate::reclaim::HOLD_NOT_READY,
            ),
            (
                self.agent_working_within(&root),
                crate::workspace::FINISH_BLOCKER_AGENT_WORKING,
            ),
            (
                self.terminal_open_within(&root),
                crate::reclaim::HOLD_TERMINAL_OPEN,
            ),
            (
                workspace
                    .directories
                    .iter()
                    .any(|directory| !directory.is_git),
                crate::workspace::FINISH_BLOCKER_PLAIN_DIRECTORY,
            ),
            (
                tasks
                    .as_ref()
                    .is_ok_and(|tasks| tasks.iter().any(|task| !task.finished())),
                crate::reclaim::HOLD_TASK_OPEN,
            ),
            (tasks.is_err(), crate::reclaim::HOLD_TASKS_UNREAD),
        ];
        checks
            .into_iter()
            .filter_map(|(holds, hold)| holds.then_some(hold))
            .collect()
    }

    /// An agent working anywhere in the workspace: at its root, or in one of
    /// its checkouts.
    fn agent_working_within(&self, root: &Path) -> bool {
        self.delivery_queue.has_in_flight_within(root)
            || self.session_registry.agent_working_roots().into_iter().any(
                |(candidate, working)| {
                    working && Self::canonical_root(&candidate).starts_with(root)
                },
            )
    }

    /// One of the user's terminals open anywhere in the workspace. A shell's
    /// tab lasts exactly as long as its process.
    fn terminal_open_within(&self, root: &Path) -> bool {
        self.session_registry
            .tab_keys()
            .into_iter()
            .any(|key| !key.is_agent() && key.root.starts_with(root))
    }

    /// Every task of every project, filed under each workspace it links. One
    /// store read per project. One project's tasks failing to read fails the
    /// whole answer: nobody can say which workspaces those tasks link.
    fn tasks_linking_workspaces(&self) -> LinkedTasks {
        let mut linked: HashMap<String, Vec<LinkedTask>> = HashMap::new();
        for project in self.projects.iter() {
            for task in self.every_task_of(&project.id)? {
                for workspace_id in &task.links.workspace_ids {
                    linked
                        .entry(workspace_id.clone())
                        .or_default()
                        .push(linked_task(&task));
                }
            }
        }
        Ok(linked)
    }

    fn every_task_of(&self, project_id: &str) -> Result<Vec<Task>, String> {
        // A bridge with no store has no tasks at all.
        let Some(store) = self.store.as_ref() else {
            return Ok(Vec::new());
        };
        let project_path = self.tracker_project_path(project_id)?;
        store
            .list_tracker_tasks(&project_path, crate::store::TaskFilter::default())
            .map_err(|error| format!("read the tasks of {project_id}: {error}"))
    }

    /// Keep what was measured, record on the linked tasks what they should
    /// hear, and tell each project agent about its quiet workspaces. A
    /// workspace a stopped sweep never reached keeps its last verdict.
    fn settle_workspace_sweep(
        &mut self,
        measured: Vec<(Subject, LifecycleRecord)>,
        now_ms: i64,
        policy: &ReclaimPolicy,
    ) {
        let workspaces = &self.workspaces;
        self.workspace_lifecycle
            .retain(|workspace_id, _| workspaces.get(workspace_id).is_some());
        let mut quiet: BTreeMap<String, Vec<Quiet>> = BTreeMap::new();
        for (subject, mut record) in measured {
            // Removed while it was being measured: nothing left to say.
            if self.workspaces.get(&subject.workspace_id).is_none() {
                continue;
            }
            record.keep_newer_size(self.workspace_lifecycle.get(&subject.workspace_id));
            if record.pruned_at_ms == Some(now_ms) {
                self.note_on_linked_tasks(
                    &subject,
                    TaskEventKind::WorkspacePruned,
                    &record,
                    policy,
                );
            }
            let due = record.notice_due(now_ms, policy);
            if due != NoticeDue::No {
                quiet
                    .entry(subject.project_id.clone())
                    .or_default()
                    .push(Quiet {
                        subject: subject.clone(),
                        record: record.clone(),
                        first: due == NoticeDue::First,
                    });
            }
            self.workspace_lifecycle
                .insert(subject.workspace_id, record);
        }
        for (project_id, workspaces) in quiet {
            self.announce_quiet_workspaces(&project_id, &workspaces, now_ms, policy);
        }
        self.persist_workspace_lifecycle();
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
    }

    /// One notice for the project agent naming every quiet workspace in its
    /// project. Only a notice that landed counts as told: one that failed is
    /// tried again next sweep.
    fn announce_quiet_workspaces(
        &mut self,
        project_id: &str,
        workspaces: &[Quiet],
        now_ms: i64,
        policy: &ReclaimPolicy,
    ) {
        let body = notice::quiet_workspaces(workspaces, policy);
        if let Err(error) = self.tell_project_agent(project_id, body) {
            eprintln!(
                "workspace reclaim: tell project {project_id} about quiet workspaces: {error}"
            );
            return;
        }
        for quiet in workspaces {
            // A workspace that has just gone quiet is recorded on its tasks
            // once, when the project agent has actually been told.
            if quiet.first {
                self.note_on_linked_tasks(
                    &quiet.subject,
                    TaskEventKind::WorkspaceIdle,
                    &quiet.record,
                    policy,
                );
            }
            if let Some(record) = self
                .workspace_lifecycle
                .get_mut(&quiet.subject.workspace_id)
            {
                record.noticed_at_ms = Some(now_ms);
            }
        }
    }

    fn tell_project_agent(&mut self, project_id: &str, body: String) -> Result<(), String> {
        let conversation =
            self.project_ensure_conversation(&json!({ "project_id": project_id }))?;
        let entity_id = conversation["run_id"]
            .as_str()
            .ok_or("the project conversation has no owner")?
            .to_string();
        let agent_id = self.ensure_primary_agent(&entity_id)?;
        self.post_from_build_and_wake(&entity_id, &agent_id, notice::PHASE, |thread, now| {
            thread.post_user_from_build(body, now);
        })
    }

    /// A timeline entry by Build on every task linking the workspace. Written
    /// without waking the tasks' watchers: the project agent gets one notice
    /// for the whole sweep instead.
    fn note_on_linked_tasks(
        &mut self,
        subject: &Subject,
        kind: TaskEventKind,
        record: &LifecycleRecord,
        policy: &ReclaimPolicy,
    ) {
        let payload = json!({
            "workspace_id": subject.workspace_id,
            // What "quiet" meant when this was written: Settings can move it.
            "idle_after_secs": policy.idle_after.as_secs(),
            "workspace_name": subject.name,
            "last_activity_ms": record.last_activity_ms,
            "reclaimable": record.reclaimable,
            "holds": record.holds,
            "size_bytes": record.size_bytes,
            "pruned_bytes": record.pruned_bytes,
        });
        for task in &subject.tasks {
            if let Err(error) =
                self.record_quiet_event(&task.task_id, &Actor::Build, kind, payload.clone())
            {
                eprintln!(
                    "workspace reclaim: note {} on #{}: {error}",
                    kind.as_str(),
                    task.number
                );
            }
        }
    }

    pub(in crate::app) fn persist_workspace_lifecycle(&self) {
        if let Some(store) = self.store.as_ref() {
            if let Err(error) = store.save_workspace_lifecycle(&self.workspace_lifecycle) {
                eprintln!("workspace reclaim: persist: {error}");
            }
        }
    }
}

/// One workspace's tasks out of every project's, or the reason none could be
/// read.
fn tasks_of(linked: &LinkedTasks, workspace_id: &str) -> Result<Vec<LinkedTask>, String> {
    match linked {
        Ok(linked) => Ok(linked.get(workspace_id).cloned().unwrap_or_default()),
        Err(error) => Err(error.clone()),
    }
}

fn linked_task(task: &Task) -> LinkedTask {
    LinkedTask {
        task_id: task.id.clone(),
        number: task.number,
        title: task.title.clone(),
        status: task.status.clone(),
        state: match task.state {
            TaskState::Open => "open",
            TaskState::Closed => "closed",
        }
        .to_string(),
    }
}
