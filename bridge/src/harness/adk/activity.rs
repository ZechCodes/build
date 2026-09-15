use super::protocol::ProtocolState;
#[cfg(test)]
use super::reader::ProtocolReader;
use crate::harness::shell_tail::ShellTail;
use crate::harness::surfaces::SurfaceRevision;
use crate::harness::ActivityReport;
#[cfg(test)]
use crate::harness::{AgentStatus, SessionStatusSnapshot};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::broadcast;
#[cfg(test)]
use tokio::sync::watch;

pub(super) type ActivitySlot = Arc<Mutex<Option<broadcast::Sender<ActivityReport>>>>;

const SHELL_TAIL_INTERVAL: Duration = Duration::from_secs(1);

pub(super) type ShellPollerSlot = Arc<Mutex<Option<JoinHandle<()>>>>;

#[cfg(test)]
pub(super) fn running_shell_outputs(state: &Mutex<ProtocolState>) -> Vec<(String, PathBuf)> {
    state.lock().unwrap().surfaces.running_shell_outputs()
}

pub(super) fn shells_left_to_tail(
    state: &Mutex<ProtocolState>,
    activity: &ActivitySlot,
    shell_poller: &ShellPollerSlot,
) -> Option<Vec<(String, PathBuf)>> {
    let session_ended = activity.lock().unwrap().is_none();
    let held = state.lock().unwrap();
    let running = held.surfaces.running_shell_outputs();
    match session_ended || running.is_empty() {
        true => {
            *shell_poller.lock().unwrap() = None;
            None
        }
        false => Some(running),
    }
}

pub(super) fn spawn_shell_tail_poller(
    state: Arc<Mutex<ProtocolState>>,
    activity: ActivitySlot,
    revision: SurfaceRevision,
    shell_poller: ShellPollerSlot,
) -> JoinHandle<()> {
    std::thread::spawn(move || loop {
        std::thread::sleep(SHELL_TAIL_INTERVAL);
        let Some(running) = shells_left_to_tail(&state, &activity, &shell_poller) else {
            return;
        };
        for (shell_id, output_path) in running {
            match ShellTail::read(&output_path) {
                Ok(tailed) => {
                    let moved = state
                        .lock()
                        .unwrap()
                        .surfaces
                        .read_shell_tail(&shell_id, tailed);
                    if moved {
                        revision.bump();
                    }
                }
                Err(why) => eprintln!("shell tail {shell_id}: {why}"),
            }
        }
    })
}

#[cfg(test)]
pub(crate) fn reports_minted_by(file_name: &str) -> Vec<ActivityReport> {
    let (sender, mut heard) = broadcast::channel(1024);
    let activity: ActivitySlot = Arc::new(Mutex::new(Some(sender)));
    let mut reader = reader_reporting_into(Arc::clone(&activity));
    for line in crate::harness::stream_fixtures::fixture_lines(file_name) {
        reader.read_line(&line);
    }
    activity.lock().unwrap().take();
    reports_already_sent(&mut heard)
}

#[cfg(test)]
pub(super) fn reports_already_sent(
    heard: &mut broadcast::Receiver<ActivityReport>,
) -> Vec<ActivityReport> {
    let mut reported = Vec::new();
    while let Ok(one) = heard.try_recv() {
        reported.push(one);
    }
    reported
}

#[cfg(test)]
pub(super) fn reader_reporting_into(activity: ActivitySlot) -> ProtocolReader {
    let (status_updates, _) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
    ProtocolReader::new(
        Arc::new(Mutex::new(ProtocolState::new(
            &crate::models::ModelChoice::default(),
        ))),
        activity,
        SurfaceRevision::default(),
        status_updates,
    )
}
