use super::*;
use crate::harness::AgentActivity;
use crate::provider_recovery::{episodes, RecoveryEpisode, RecoveryScope, RecoveryStage};
use crate::resume::disk::{testing::RecordingDisk, Disk, RealDisk};
use std::io;

struct TerminalWriteGate {
    reached: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

impl Drop for TerminalWriteGate {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

struct GatedTerminalDisk {
    reached: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    refuse: bool,
}

impl GatedTerminalDisk {
    fn new(refuse: bool) -> (Arc<Self>, TerminalWriteGate) {
        let (reached_sender, reached) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        (
            Arc::new(Self {
                reached: reached_sender,
                release: Mutex::new(release_receiver),
                refuse,
            }),
            TerminalWriteGate { reached, release },
        )
    }
}

impl Disk for GatedTerminalDisk {
    fn create_synced(&self, path: &Path, body: &[u8]) -> io::Result<()> {
        let record: RecoveryEpisode = serde_json::from_slice(body)?;
        if record.stage == RecoveryStage::Terminal {
            self.reached.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            if self.refuse {
                return Err(io::Error::other(
                    "terminal write refused: injected I/O failure",
                ));
            }
        }
        RealDisk.create_synced(path, body)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        RealDisk.rename(from, to)
    }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        RealDisk.unlink(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        RealDisk.sync_dir(dir)
    }

    fn read(&self, path: &Path) -> io::Result<String> {
        RealDisk.read(path)
    }
}

fn checkpoint_session(
    root: &Path,
    disk: Arc<dyn Disk>,
) -> (
    CodexAppServerSession,
    broadcast::Receiver<ActivityReport>,
    RecoveryScope,
) {
    let started = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
    let script = opened_thread_script(
        root,
        &format!("read turn; printf '%s\\n' '{started}'; exit 23"),
    );
    let (session, activity) = scripted_session(root, &script);
    wait_until("became ready", || session.status() == AgentStatus::Waiting);
    let scope = RecoveryScope {
        entity_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "agent".into(),
        thread_id: "build-generation".into(),
    };
    *session.core.recovery.lock().unwrap() = Some(
        RecoveryJournal::on(
            RecoveryContext {
                root: root.into(),
                scope: scope.clone(),
            },
            disk,
        )
        .unwrap(),
    );
    (session, activity, scope)
}

fn failures_at_ended(activity: &mut broadcast::Receiver<ActivityReport>) -> Vec<ActivityReport> {
    let mut failures = Vec::new();
    loop {
        match activity.try_recv() {
            Ok(report) if matches!(report.activity, AgentActivity::TurnFailed { .. }) => {
                failures.push(report);
            }
            Ok(_) => {}
            Err(broadcast::error::TryRecvError::Closed) => return failures,
            error => panic!("Ended must follow failure delivery and activity closure: {error:?}"),
        }
    }
}

#[test]
fn revision_622_ended_waits_for_terminal_checkpoint_and_failure_delivery() {
    let root = tempfile::tempdir().unwrap();
    let (disk, gate) = GatedTerminalDisk::new(false);
    let (session, mut activity, scope) = checkpoint_session(root.path(), disk);
    let status = session.status_changed().unwrap();
    session
        .send_turn(&Turn::new("go").with_operation_id("failed-operation"))
        .unwrap();
    gate.reached.recv_timeout(Duration::from_secs(5)).unwrap();
    let direct_status = session.status();
    let watched_status = status.borrow().status.clone();
    let pending = episodes(root.path(), &scope).unwrap();
    gate.release.send(()).unwrap();
    wait_until("ended", || {
        matches!(session.status(), AgentStatus::Ended { .. })
    });

    assert!(!matches!(direct_status, AgentStatus::Ended { .. }));
    assert!(!matches!(watched_status, AgentStatus::Ended { .. }));
    assert_eq!(pending[0].stage, RecoveryStage::Pending);
    let records = episodes(root.path(), &scope).unwrap();
    assert_eq!(records[0].stage, RecoveryStage::Terminal);
    assert!(records[0].needs_notice);
    let failures = failures_at_ended(&mut activity);
    assert!(matches!(failures.as_slice(), [ActivityReport {
        activity: AgentActivity::TurnFailed { operation_id: Some(operation), episode_id: Some(id), .. }, ..
    }] if operation == "failed-operation" && id == &records[0].id));
    assert_eq!(
        status.borrow().status,
        AgentStatus::Ended { code: Some(23) }
    );
}

#[test]
fn revision_622_failed_terminal_checkpoint_delivers_direct_failure_before_ended() {
    let root = tempfile::tempdir().unwrap();
    let (disk, gate) = GatedTerminalDisk::new(true);
    let (session, mut activity, _) = checkpoint_session(root.path(), disk);
    session
        .send_turn(&Turn::new("go").with_operation_id("failed-operation"))
        .unwrap();
    gate.reached.recv_timeout(Duration::from_secs(5)).unwrap();
    let before_checkpoint = session.status();
    gate.release.send(()).unwrap();
    wait_until("ended", || {
        matches!(session.status(), AgentStatus::Ended { .. })
    });

    assert!(!matches!(before_checkpoint, AgentStatus::Ended { .. }));
    let failures = failures_at_ended(&mut activity);
    assert!(matches!(failures.as_slice(), [ActivityReport {
        activity: AgentActivity::TurnFailed { operation_id: Some(operation), episode_id: None, summary }, ..
    }] if operation == "failed-operation" && summary.contains("23")));
}

#[test]
fn revision_622_checkpoint_diagnostics_preserve_internal_io_reason() {
    let root = tempfile::tempdir().unwrap();
    let disk = Arc::new(RecordingDisk::default());
    let (session, _, _) = checkpoint_session(root.path(), disk.clone());
    disk.fail_next_creates(1);
    session
        .send_turn(&Turn::new("go").with_operation_id("failed-operation"))
        .unwrap();
    wait_for_terminal_sources(&session, "finished terminal publication", |snapshot| {
        snapshot.publication_claimed
    });
    let diagnostics = session.core.diagnostics.captured();
    let failed = diagnostics
        .iter()
        .find(|event| event["event"] == "recovery_checkpoint_failed")
        .unwrap();
    let reason = failed["reason"].as_str().unwrap();
    assert!(reason.starts_with(&format!(
        "write {}/provider-recovery/",
        root.path().display()
    )));
    assert!(reason.ends_with(": the test disk refused"), "{reason}");
    assert!(!reason.contains("[REDACTED]"));
}
