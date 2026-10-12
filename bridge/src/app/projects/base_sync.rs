//! Keeping each source's base branch in step with its remote (#267): the
//! service that does it while no client is connected, the sync a workspace
//! cut runs first, `project.sync_source`, and what each sync concluded.
//!
//! The service is always-on logic, so it stands apart from the RPC and event
//! layer: a loop on its own task that reads the sources under the app mutex,
//! syncs each with the mutex released ([`crate::source_sync`]), and writes
//! what it found under the mutex. What it writes reaches clients the way
//! every other change does: each source row carries it as `sync`, and the
//! project list is noted changed.

use crate::app::{off_the_workers, AppState};
use crate::changes::BoardLists;
use crate::source_sync::{
    sync_base, sync_base_for_a_cut, Failure, Fetch, SyncLock, SyncOutcome, SyncReport,
    CUT_FETCH_DEADLINE, SERVICE_FETCH_DEADLINE,
};
use crate::workspace::WorkspaceSource;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// A cut that finds a fetch this recent moves the base to what it fetched
/// rather than asking the remote again.
const FRESH_FETCH: Duration = Duration::from_secs(60);

/// How long the timer leaves a source whose fetches keep timing out. The
/// first timeout is retried on the next pass; from the second the wait
/// starts at ten minutes and doubles, up to an hour.
const TIMEOUT_BACKOFF_FIRST: Duration = Duration::from_secs(10 * 60);
const TIMEOUT_BACKOFF_MOST: Duration = Duration::from_secs(60 * 60);

/// How early a pass may come and still count as on time: passes are
/// `every` apart, give or take a few milliseconds.
const PASS_SLACK: Duration = Duration::from_secs(60);

/// Where a sync of a source left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::app) enum SyncState {
    /// Level with the remote, or fast-forwarded to it.
    Synced,
    /// Left as it was so no work could be lost; `reason` says why.
    Skipped,
    /// The remote could not be reached or read; `reason` says why.
    Failed,
    /// The checkout has no remote to follow.
    NoRemote,
}

/// What the last sync of one source concluded, as its row carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) struct SyncStatus {
    pub(in crate::app) state: SyncState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::app) reason: Option<String>,
    /// Commits the base has that the remote does not.
    pub(in crate::app) ahead: usize,
    /// Commits the remote has that the base does not.
    pub(in crate::app) behind: usize,
    /// How far the last sync moved the base forward.
    pub(in crate::app) commits: usize,
    /// The remote wanted a person, and said so (a password, a passphrase).
    /// The service leaves the source alone until someone syncs it by hand
    /// or a workspace cut syncs it.
    pub(in crate::app) needs_you: bool,
    pub(in crate::app) last_attempt_ms: i64,
    /// The last sync that left the base level with its remote.
    pub(in crate::app) last_synced_ms: Option<i64>,
    /// The last time the remote answered a fetch.
    pub(in crate::app) last_fetched_ms: Option<i64>,
    /// The service's fetches that timed out in a row, which the timer backs
    /// off by. Bookkeeping, not news: neither on the wire nor stored, so a
    /// restart tries again at once.
    #[serde(skip)]
    pub(in crate::app) timeouts: u32,
}

impl SyncStatus {
    fn after(previous: Option<&SyncStatus>, report: &SyncReport, now_ms: i64) -> Self {
        let concluded = Concluded::from(&report.outcome);
        let kept = |pick: fn(&SyncStatus) -> Option<i64>| previous.and_then(pick);
        Self {
            state: concluded.state,
            reason: concluded.reason,
            ahead: report.ahead,
            behind: report.behind,
            commits: concluded.commits,
            needs_you: concluded.needs_you,
            last_attempt_ms: now_ms,
            last_synced_ms: if concluded.state == SyncState::Synced {
                Some(now_ms)
            } else {
                kept(|status| status.last_synced_ms)
            },
            last_fetched_ms: if report.fetched {
                Some(now_ms)
            } else {
                kept(|status| status.last_fetched_ms)
            },
            timeouts: if concluded.timed_out {
                previous.map_or(0, |status| status.timeouts) + 1
            } else {
                0
            },
        }
    }

    fn fetched_since(&self, since_ms: i64) -> bool {
        self.last_fetched_ms.is_some_and(|at| at >= since_ms)
    }

    /// Whether the timer syncs a source left like this at `now_ms`: not one
    /// that wanted a person, nor one still inside its timeout backoff.
    fn due_for_the_timer(&self, now_ms: i64) -> bool {
        let wait = i64::try_from(timeout_backoff(self.timeouts).as_millis()).unwrap_or(i64::MAX);
        !self.needs_you && now_ms >= self.last_attempt_ms.saturating_add(wait)
    }
}

/// How long after the last attempt the timer waits, given the timeouts in a
/// row that attempt ended.
fn timeout_backoff(timeouts: u32) -> Duration {
    if timeouts < 2 {
        return Duration::ZERO;
    }
    let doublings = (timeouts - 2).min(8);
    TIMEOUT_BACKOFF_FIRST
        .saturating_mul(1 << doublings)
        .min(TIMEOUT_BACKOFF_MOST)
        .saturating_sub(PASS_SLACK)
}

/// One outcome, as the parts a status is written from.
struct Concluded {
    state: SyncState,
    reason: Option<String>,
    commits: usize,
    needs_you: bool,
    timed_out: bool,
}

impl From<&SyncOutcome> for Concluded {
    fn from(outcome: &SyncOutcome) -> Self {
        let plain = |state, reason: Option<&String>| Self {
            state,
            reason: reason.cloned(),
            commits: 0,
            needs_you: false,
            timed_out: false,
        };
        match outcome {
            SyncOutcome::UpToDate => plain(SyncState::Synced, None),
            SyncOutcome::FastForwarded { commits } => Self {
                commits: *commits,
                ..plain(SyncState::Synced, None)
            },
            SyncOutcome::Skipped(reason) => plain(SyncState::Skipped, Some(reason)),
            SyncOutcome::Failed(failure) => Self {
                needs_you: failure.needs_you,
                timed_out: failure.timed_out,
                ..plain(SyncState::Failed, Some(&failure.reason))
            },
            SyncOutcome::NoRemote => plain(SyncState::NoRemote, None),
            SyncOutcome::CheckoutLeft { .. } => plain(SyncState::Skipped, None),
        }
    }
}

/// One source to sync, read under the mutex.
#[derive(Clone, Debug)]
pub(in crate::app) struct SyncSubject {
    project_id: String,
    source_id: String,
    label: String,
    path: PathBuf,
    base_branch: String,
    fetch: Fetch,
    origin: SyncOrigin,
    enabled: bool,
}

/// Who ran a sync: the service (the timer or Sync now), or a workspace cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SyncOrigin {
    Service,
    Cut,
}

/// One source's sync, to be written under the mutex.
pub(in crate::app) struct Synced {
    subject: SyncSubject,
    report: SyncReport,
    start: Option<String>,
}

impl Synced {
    /// Whether this sync says anything the source's status should keep. A
    /// cut that ran out of its ten seconds (waiting on the remote, or on
    /// another sync) says nothing the timer should act on, so the status is
    /// left as the last real answer made it; nor does one that left the
    /// checkout to the service, which is about to say how that went.
    fn tells_the_timer(&self) -> bool {
        let ran_out = matches!(
            &self.report.outcome,
            SyncOutcome::Failed(Failure {
                timed_out: true,
                ..
            })
        );
        self.subject.enabled
            && !(self.subject.origin == SyncOrigin::Cut && ran_out)
            && !self.left_the_checkout()
    }

    fn left_the_checkout(&self) -> bool {
        matches!(self.report.outcome, SyncOutcome::CheckoutLeft { .. })
    }
}

/// The commit a cut branches `source` from in place of its base branch: the
/// one its base is moving to, when the sync left the source's checkout to
/// the service with nothing in the way (#271). `None` cuts from the base as
/// it stands.
pub(in crate::app) fn cut_start(synced: &[Synced], source: &WorkspaceSource) -> Option<String> {
    synced.iter().find_map(|synced| {
        let subject = &synced.subject;
        let same = subject.source_id == source.id
            && subject.path == source.path
            && subject.base_branch == source.base_branch;
        same.then(|| synced.start.clone()).flatten()
    })
}

/// Which sources a pass of the service syncs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPass {
    /// The timer: every source that syncs and has not asked for a person,
    /// and any somebody asked for.
    Due,
    /// Only the sources somebody asked for with Sync now.
    Requested,
}

/// When the service runs.
#[derive(Clone, Debug)]
pub struct SourceSyncPolicy {
    pub first_after: Duration,
    pub every: Duration,
}

impl Default for SourceSyncPolicy {
    fn default() -> Self {
        Self {
            first_after: Duration::from_secs(30),
            every: Duration::from_secs(5 * 60),
        }
    }
}

pub(in crate::app) fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX)
}

/// Sync one source for the service, once no other sync holds it.
fn sync_for_the_service(subject: SyncSubject, state: &Arc<Mutex<AppState>>) -> Option<Synced> {
    let (_held, _) = SyncLock::acquire(&subject.path, SERVICE_FETCH_DEADLINE)?;
    if !subject.is_current(&state.lock().unwrap()) {
        return None;
    }
    let report = sync_base(&subject.path, &subject.base_branch, subject.fetch);
    Some(Synced {
        subject,
        report,
        start: None,
    })
}

/// Sync one source before a cut. A sync already running is waited for up to
/// the cut's deadline, and then what it fetched is used rather than fetching
/// again; one that outlasts the lock deadline refuses the cut so no mutable ref is
/// resolved without protection. The
/// cut never fast-forwards the source's own checkout, which can take
/// minutes: that is left to the service, and the cut branches from the
/// commit the checkout is moving to ([`SyncOutcome::CheckoutLeft`]).
fn sync_for_a_cut(
    subject: SyncSubject,
    waited: bool,
    state: Option<&std::sync::Weak<Mutex<AppState>>>,
) -> Result<Synced, String> {
    if let Some(state) = state.and_then(std::sync::Weak::upgrade) {
        if !subject.is_current(&state.lock().unwrap()) {
            return Err(super::MOVED_GIT_SOURCE.to_string());
        }
    }
    let mut report = if subject.enabled {
        let fetch = if waited { Fetch::Skip } else { subject.fetch };
        sync_base_for_a_cut(&subject.path, &subject.base_branch, fetch)
    } else {
        SyncReport {
            outcome: SyncOutcome::NoRemote,
            ahead: local_base_commits(&subject),
            behind: 0,
            fetched: false,
        }
    };
    report.ahead = report.ahead.max(local_base_commits(&subject));
    // Pin while the repository is protected. Materialization must never resolve
    // the mutable base ref again after this lock drops (#271, #530).
    let start = match &report.outcome {
        SyncOutcome::CheckoutLeft { cut_from } => cut_from.clone(),
        _ => git2::Repository::open(&subject.path)
            .and_then(|repo| {
                repo.revparse_single(&subject.base_branch)?
                    .peel_to_commit()
                    .map(|commit| commit.id().to_string())
            })
            .map_err(|error| format!("cannot pin workspace base: {error}"))?,
    };
    Ok(Synced {
        subject,
        report,
        start: Some(start),
    })
}

impl SyncSubject {
    fn is_current(&self, app: &AppState) -> bool {
        app.projects
            .get(&self.project_id)
            .and_then(|project| {
                project
                    .sources
                    .iter()
                    .find(|source| source.id == self.source_id)
            })
            .is_some_and(|source| {
                source.path == self.path && source.base_branch == self.base_branch && source.is_git
            })
    }
}

/// Count unpublished base commits without syncing when automatic sync is off.
fn local_base_commits(subject: &SyncSubject) -> usize {
    let counts = (|| {
        let repo = git2::Repository::open(&subject.path).ok()?;
        let base = repo
            .find_branch(&subject.base_branch, git2::BranchType::Local)
            .ok()?;
        let upstream = base
            .upstream()
            .or_else(|_| {
                repo.find_branch(
                    &format!("origin/{}", subject.base_branch),
                    git2::BranchType::Remote,
                )
            })
            .ok()?;
        repo.graph_ahead_behind(base.get().target()?, upstream.get().target()?)
            .ok()
    })();
    counts.map_or(0, |(ahead, _)| ahead)
}

/// Sync independent repositories side by side. Aliases sharing refs are
/// prepared together under one acquisition of their common Git directory.
pub(in crate::app) fn sync_before_cut(
    subjects: &[SyncSubject],
    state: Option<&std::sync::Weak<Mutex<AppState>>>,
) -> Result<Vec<Synced>, String> {
    let mut groups: std::collections::BTreeMap<PathBuf, Vec<SyncSubject>> = Default::default();
    for subject in subjects {
        groups
            .entry(crate::source_sync::sync_lock_identity(&subject.path))
            .or_default()
            .push(subject.clone());
    }
    std::thread::scope(|scope| {
        let running: Vec<_> = groups.into_values().map(|group| scope.spawn(move || {
            let (_held, waited) = SyncLock::acquire(&group[0].path, CUT_FETCH_DEADLINE)
                .ok_or_else(|| "Another source operation is still running; try creating the workspace again.".to_string())?;
            group.into_iter().map(|subject| sync_for_a_cut(subject, waited, state)).collect::<Result<Vec<_>, String>>()
        })).collect();
        let batches: Result<Vec<_>, String> = running
            .into_iter()
            .map(|sync| {
                sync.join()
                    .map_err(|_| "workspace base preparation failed".to_string())?
            })
            .collect();
        Ok(batches?.into_iter().flatten().collect())
    })
}

/// What the agent or person who asked for a workspace is told about each
/// source whose base may still be behind its remote.
pub(in crate::app) fn cut_warnings(synced: &[Synced]) -> Vec<String> {
    synced.iter().filter_map(cut_warning).collect()
}

fn commits(count: usize) -> String {
    let noun = if count == 1 { "commit" } else { "commits" };
    format!("{count} {noun}")
}

fn cut_warning(synced: &Synced) -> Option<String> {
    let subject = &synced.subject;
    let behind = synced.report.behind;
    if synced.report.ahead > 0 {
        let mut included = format!(
            "{}: {} local {} on {} {} included in this workspace.",
            subject.label,
            synced.report.ahead,
            if synced.report.ahead == 1 {
                "commit"
            } else {
                "commits"
            },
            subject.base_branch,
            if synced.report.ahead == 1 {
                "is"
            } else {
                "are"
            }
        );
        if behind > 0 {
            included.push_str(&format!(
                " The configured base is {} behind its remote; it was preserved as it stood.",
                commits(behind)
            ));
        }
        if let SyncOutcome::Failed(failure) = &synced.report.outcome {
            included.push_str(&format!(
                " The base could not be synced: {}",
                failure.reason
            ));
        }
        return Some(included);
    }
    let why = match &synced.report.outcome {
        SyncOutcome::Failed(failure) => failure.reason.clone(),
        SyncOutcome::Skipped(reason) if behind > 0 => {
            format!("It is {} behind its remote. {reason}", commits(behind))
        }
        _ => return None,
    };
    Some(format!(
        "{}: this workspace was cut from {} as it stood, which may be behind its remote. {why}",
        subject.label, subject.base_branch
    ))
}

impl AppState {
    /// Run the service for the life of the daemon: a pass a little after
    /// startup, then one every `policy.every`, and one for the sources
    /// somebody asked for as soon as they ask.
    ///
    /// No runtime worker waits on the app mutex here (#131): the handles are
    /// read and each pass runs on the blocking pool.
    pub async fn spawn_source_sync(
        state: Arc<Mutex<AppState>>,
        policy: SourceSyncPolicy,
    ) -> Arc<AtomicBool> {
        let (nudge, stop) = {
            let state = Arc::clone(&state);
            off_the_workers(move || {
                let app = state.lock().unwrap();
                (app.source_sync_nudge.clone(), app.source_sync_stop.clone())
            })
            .await
        };
        let stopped = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut due = tokio::time::Instant::now() + policy.first_after;
            loop {
                let pass = tokio::select! {
                    _ = tokio::time::sleep_until(due) => {
                        due = tokio::time::Instant::now() + policy.every;
                        SyncPass::Due
                    }
                    _ = nudge.notified() => SyncPass::Requested,
                };
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let syncing = Arc::clone(&state);
                let ran = tokio::task::spawn_blocking(move || {
                    AppState::sync_sources(&syncing, pass, now_ms());
                });
                if let Err(joined) = ran.await {
                    eprintln!("source sync: pass failed: {joined}");
                }
            }
        });
        stop
    }

    /// Stop the service: no pass starts after the one running.
    pub fn stop_source_sync(state: &Arc<Mutex<AppState>>) {
        let stop = state.lock().unwrap().source_sync_stop.clone();
        stop.store(true, Ordering::Relaxed);
    }

    /// One pass: read the sources under the mutex, sync each with it
    /// released, one after another, and write what was found.
    pub fn sync_sources(state: &Arc<Mutex<AppState>>, pass: SyncPass, now_ms: i64) {
        let subjects = state.lock().unwrap().source_sync_subjects(pass, now_ms);
        let synced: Vec<Synced> = subjects
            .into_iter()
            .filter_map(|subject| sync_for_the_service(subject, state))
            .collect();
        state.lock().unwrap().settle_source_syncs(synced, now_ms);
    }

    fn source_sync_subjects(&mut self, pass: SyncPass, now_ms: i64) -> Vec<SyncSubject> {
        let requested = std::mem::take(&mut self.source_sync_requested);
        let fetch = Fetch::Within(SERVICE_FETCH_DEADLINE);
        let mut subjects: Vec<_> = requested.into_values().collect();
        if pass == SyncPass::Due {
            for project in self.projects.iter() {
                for source in &project.sources {
                    if due_for_the_timer(source, now_ms)
                        && !subjects.iter().any(|subject| {
                            subject.project_id == project.id && subject.source_id == source.id
                        })
                    {
                        subjects.push(subject(&project.id, source, fetch, SyncOrigin::Service));
                    }
                }
            }
        }
        subjects
    }

    /// The sources a workspace cut in `project_id` syncs first, each fetching
    /// unless it fetched a moment ago.
    pub(in crate::app) fn cut_sync_subjects(&self, project_id: &str) -> Vec<SyncSubject> {
        let fresh_since = now_ms() - FRESH_FETCH.as_millis() as i64;
        self.sources_for(project_id)
            .unwrap_or_default()
            .iter()
            .filter(|source| source.is_git)
            .map(|source| {
                let fresh = source
                    .sync_status
                    .as_ref()
                    .is_some_and(|status| status.fetched_since(fresh_since));
                let fetch = if fresh {
                    Fetch::Skip
                } else {
                    Fetch::Within(CUT_FETCH_DEADLINE)
                };
                subject(project_id, source, fetch, SyncOrigin::Cut)
            })
            .collect()
    }

    /// Write what each sync concluded onto its source, when the source is
    /// still the one that was synced: a source removed, moved or rebased
    /// meanwhile is not written. A checkout a cut left is handed to the
    /// service.
    pub(in crate::app) fn settle_source_syncs(&mut self, synced: Vec<Synced>, now_ms: i64) {
        for left in synced.iter().filter(|synced| synced.left_the_checkout()) {
            self.request_source_sync(
                left.subject.project_id.clone(),
                left.subject.source_id.clone(),
            );
        }
        let mut wrote = false;
        for Synced {
            subject, report, ..
        } in synced.into_iter().filter(Synced::tells_the_timer)
        {
            let Some(source) = self
                .projects
                .source_mut(&subject.project_id, &subject.source_id)
            else {
                continue;
            };
            if source.path != subject.path || source.base_branch != subject.base_branch {
                continue;
            }
            source.sync_status = Some(SyncStatus::after(
                source.sync_status.as_ref(),
                &report,
                now_ms,
            ));
            wrote = true;
        }
        if wrote {
            self.persist_source_syncs();
            self.note_board_lists_changed(BoardLists::PROJECTS);
        }
    }

    /// `project.sync_source` — sync one source now, whatever the setting
    /// says: somebody asked. The service does it; the answer says it is on
    /// its way, and the source's row says how it went.
    pub(crate) fn project_sync_source(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = crate::app::require_str(params, "project_id")?;
        let source_id = crate::app::require_str(params, "source_id")?;
        let project = self.project_for(&project_id)?;
        let source = project
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        if !source.is_git {
            return Err(
                "Build cannot sync a folder that is not a Git repository: it has no base branch."
                    .to_string(),
            );
        }
        self.resolve_project_source(
            &project_id,
            &source_id,
            params.get("source_path").and_then(Value::as_str),
            super::MOVED_GIT_SOURCE,
        )?;
        self.request_source_sync(project_id, source_id);
        Ok(json!({ "pending": true }))
    }

    /// Ask the service to sync one source as soon as it can.
    pub(in crate::app) fn request_source_sync(&mut self, project_id: String, source_id: String) {
        if let Some(source) = self
            .projects
            .get(&project_id)
            .and_then(|project| project.sources.iter().find(|source| source.id == source_id))
        {
            let mut captured = subject(
                &project_id,
                source,
                Fetch::Within(SERVICE_FETCH_DEADLINE),
                SyncOrigin::Service,
            );
            captured.enabled = true; // Manual sync works even when automatic sync is off.
            self.source_sync_requested
                .insert((project_id, source_id), captured);
            self.source_sync_nudge.notify_one();
        }
    }

    /// Bring back what the last syncs concluded, so the settings view has
    /// something true to show before the first pass after a restart.
    pub(in crate::app) fn restore_source_syncs(&mut self) -> Result<(), String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let mut stored = store
            .load_source_syncs::<SyncStatus>()
            .map_err(|error| format!("source sync restore: {error}"))?;
        for (project_id, source) in self.projects.sources_mut() {
            source.sync_status = stored.remove(&status_key(project_id, &source.id));
        }
        Ok(())
    }

    fn persist_source_syncs(&self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let statuses: HashMap<String, &SyncStatus> = self
            .projects
            .iter()
            .flat_map(|project| {
                project.sources.iter().filter_map(move |source| {
                    let status = source.sync_status.as_ref()?;
                    Some((status_key(&project.id, &source.id), status))
                })
            })
            .collect();
        if let Err(error) = store.save_source_syncs(&statuses) {
            eprintln!("source sync: persist: {error}");
        }
    }
}

/// Whether the timer syncs this source at `now_ms`: it is on, the last sync
/// did not need a person, and it is not backing off from timeouts.
fn due_for_the_timer(source: &super::ProjectSource, now_ms: i64) -> bool {
    source.syncs_base()
        && source
            .sync_status
            .as_ref()
            .is_none_or(|status| status.due_for_the_timer(now_ms))
}

fn subject(
    project_id: &str,
    source: &super::ProjectSource,
    fetch: Fetch,
    origin: SyncOrigin,
) -> SyncSubject {
    SyncSubject {
        project_id: project_id.to_string(),
        source_id: source.id.clone(),
        label: source.name.clone(),
        path: source.path.clone(),
        base_branch: source.base_branch.clone(),
        fetch,
        origin,
        enabled: origin == SyncOrigin::Service || source.syncs_base(),
    }
}

fn status_key(project_id: &str, source_id: &str) -> String {
    format!("{project_id}/{source_id}")
}
