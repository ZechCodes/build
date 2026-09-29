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
    sync_base, Failure, Fetch, SyncLock, SyncOutcome, SyncReport, CUT_FETCH_DEADLINE,
    SERVICE_FETCH_DEADLINE,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A cut that finds a fetch this recent moves the base to what it fetched
/// rather than asking the remote again.
const FRESH_FETCH: Duration = Duration::from_secs(60);

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
    /// The remote wanted a person (a password, a passphrase, a key touch) or
    /// never answered. The service leaves the source alone until someone
    /// syncs it by hand or a workspace cut syncs it.
    pub(in crate::app) needs_you: bool,
    pub(in crate::app) last_attempt_ms: i64,
    /// The last sync that left the base level with its remote.
    pub(in crate::app) last_synced_ms: Option<i64>,
    /// The last time the remote answered a fetch.
    pub(in crate::app) last_fetched_ms: Option<i64>,
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
        }
    }

    fn fetched_since(&self, since_ms: i64) -> bool {
        self.last_fetched_ms.is_some_and(|at| at >= since_ms)
    }
}

/// One outcome, as the parts a status is written from.
struct Concluded {
    state: SyncState,
    reason: Option<String>,
    commits: usize,
    needs_you: bool,
}

impl From<&SyncOutcome> for Concluded {
    fn from(outcome: &SyncOutcome) -> Self {
        let plain = |state, reason: Option<&String>| Self {
            state,
            reason: reason.cloned(),
            commits: 0,
            needs_you: false,
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
                ..plain(SyncState::Failed, Some(&failure.reason))
            },
            SyncOutcome::NoRemote => plain(SyncState::NoRemote, None),
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
}

/// One source's sync, to be written under the mutex.
pub(in crate::app) struct Synced {
    subject: SyncSubject,
    report: SyncReport,
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
fn sync_for_the_service(subject: SyncSubject) -> Option<Synced> {
    let (_held, _) = SyncLock::acquire(&subject.path, SERVICE_FETCH_DEADLINE)?;
    let report = sync_base(&subject.path, &subject.base_branch, subject.fetch);
    Some(Synced { subject, report })
}

/// Sync one source before a cut. A sync already running is waited for up to
/// the cut's deadline, and then what it fetched is used rather than fetching
/// again; one that outlasts the deadline leaves the base as it stands.
fn sync_for_a_cut(subject: SyncSubject) -> Synced {
    let report = match SyncLock::acquire(&subject.path, CUT_FETCH_DEADLINE) {
        Some((_held, waited)) => {
            let fetch = if waited { Fetch::Skip } else { subject.fetch };
            sync_base(&subject.path, &subject.base_branch, fetch)
        }
        None => SyncReport {
            outcome: SyncOutcome::Failed(Failure {
                reason: "Another sync of this source was still running.".to_string(),
                needs_you: false,
            }),
            ahead: 0,
            behind: 0,
            fetched: false,
        },
    };
    Synced { subject, report }
}

/// Every source a cut takes, synced side by side, so the cut waits for the
/// slowest one rather than for all of them in turn.
pub(in crate::app) fn sync_before_cut(subjects: &[SyncSubject]) -> Vec<Synced> {
    std::thread::scope(|scope| {
        let running: Vec<_> = subjects
            .iter()
            .cloned()
            .map(|subject| scope.spawn(move || sync_for_a_cut(subject)))
            .collect();
        running
            .into_iter()
            .filter_map(|sync| sync.join().ok())
            .collect()
    })
}

/// What the agent or person who asked for a workspace is told about each
/// source whose base may still be behind its remote.
pub(in crate::app) fn cut_warnings(synced: &[Synced]) -> Vec<String> {
    synced.iter().filter_map(cut_warning).collect()
}

fn cut_warning(synced: &Synced) -> Option<String> {
    let subject = &synced.subject;
    let behind = synced.report.behind;
    let why = match &synced.report.outcome {
        SyncOutcome::Failed(failure) => failure.reason.clone(),
        SyncOutcome::Skipped(reason) if behind > 0 => {
            let commits = if behind == 1 { "commit" } else { "commits" };
            format!("It is {behind} {commits} behind its remote. {reason}")
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
        let subjects = state.lock().unwrap().source_sync_subjects(pass);
        let synced: Vec<Synced> = subjects
            .into_iter()
            .filter_map(sync_for_the_service)
            .collect();
        state.lock().unwrap().settle_source_syncs(synced, now_ms);
    }

    fn source_sync_subjects(&mut self, pass: SyncPass) -> Vec<SyncSubject> {
        let requested = std::mem::take(&mut self.source_sync_requested);
        let fetch = Fetch::Within(SERVICE_FETCH_DEADLINE);
        self.projects
            .iter()
            .flat_map(|project| project.sources.iter().map(move |source| (project, source)))
            .filter(|(project, source)| {
                let asked = requested.contains(&(project.id.clone(), source.id.clone()));
                source.is_git && (asked || (pass == SyncPass::Due && waits_on_the_timer(source)))
            })
            .map(|(project, source)| subject(&project.id, source, fetch))
            .collect()
    }

    /// The sources a workspace cut in `project_id` syncs first, each fetching
    /// unless it fetched a moment ago.
    pub(in crate::app) fn cut_sync_subjects(&self, project_id: &str) -> Vec<SyncSubject> {
        let fresh_since = now_ms() - FRESH_FETCH.as_millis() as i64;
        self.sources_for(project_id)
            .unwrap_or_default()
            .iter()
            .filter(|source| source.syncs_base())
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
                subject(project_id, source, fetch)
            })
            .collect()
    }

    /// Write what each sync concluded onto its source, when the source is
    /// still the one that was synced: a source removed, moved or rebased
    /// meanwhile is not written.
    pub(in crate::app) fn settle_source_syncs(&mut self, synced: Vec<Synced>, now_ms: i64) {
        let mut wrote = false;
        for Synced { subject, report } in synced {
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
        self.request_source_sync(project_id, source_id);
        Ok(json!({ "pending": true }))
    }

    /// Ask the service to sync one source as soon as it can.
    pub(in crate::app) fn request_source_sync(&mut self, project_id: String, source_id: String) {
        self.source_sync_requested.insert((project_id, source_id));
        self.source_sync_nudge.notify_one();
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

/// Whether the timer syncs this source: it is on, and the last sync did not
/// need a person.
fn waits_on_the_timer(source: &super::ProjectSource) -> bool {
    source.syncs_base()
        && !source
            .sync_status
            .as_ref()
            .is_some_and(|status| status.needs_you)
}

fn subject(project_id: &str, source: &super::ProjectSource, fetch: Fetch) -> SyncSubject {
    SyncSubject {
        project_id: project_id.to_string(),
        source_id: source.id.clone(),
        label: source.name.clone(),
        path: source.path.clone(),
        base_branch: source.base_branch.clone(),
        fetch,
    }
}

fn status_key(project_id: &str, source_id: &str) -> String {
    format!("{project_id}/{source_id}")
}
