//! Real gate holders and origin observations for actual-handler controls.
use super::*;
use crate::timing::write_wait::{IntervalCategory, WaitKind, WriteSnapshot, WriteWaitRegistry};
use crate::update::{Release, UpdateBackend, UpdateConfig, UpdateService};
use async_trait::async_trait;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) const SAFETY: Duration = Duration::from_secs(15);

pub(crate) struct HeldGate {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HeldGate {
    pub(crate) fn take(gate: Arc<AdmissionGate>) -> Self {
        let (held, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = gate.state.lock().unwrap();
            let _ = held.send(());
            let _ = released.recv_timeout(SAFETY);
        });
        let mut holder = Self {
            release: Some(release),
            thread: Some(thread),
        };
        let started = ready.recv_timeout(SAFETY);
        if started.is_err() {
            holder.release();
        }
        assert!(started.is_ok(), "owned gate holder did not start");
        holder
    }

    pub(crate) fn release(&mut self) -> bool {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.thread
            .take()
            .is_none_or(|thread| thread.join().is_ok())
    }
}

impl Drop for HeldGate {
    fn drop(&mut self) {
        self.release();
    }
}

pub(crate) fn observe(gate: &AdmissionGate) -> tokio::sync::mpsc::UnboundedReceiver<()> {
    let (entered, received) = tokio::sync::mpsc::unbounded_channel();
    let installed = gate.state_observer.set(Arc::new(move || {
        let _ = entered.send(());
    }));
    assert!(installed.is_ok(), "one observer per isolated gate");
    received
}

pub(crate) async fn wait_for_admission(
    registry: &WriteWaitRegistry,
    method: &str,
) -> Option<WriteSnapshot> {
    let deadline = Instant::now() + Duration::from_millis(750);
    let mut last = None;
    loop {
        if let Some(row) = registry
            .active_snapshot()
            .into_iter()
            .find(|row| row.method == method)
        {
            let observed = has_admission(&row);
            last = Some(row);
            if observed {
                return last;
            }
        }
        if Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn has_admission(row: &WriteSnapshot) -> bool {
    row.active
        .iter()
        .any(|interval| interval.category == IntervalCategory::KnownWait(WaitKind::Admission))
}

pub(crate) fn assert_observed(row: Option<WriteSnapshot>, original: &str) {
    let row = row.expect("actual handler origin exists while its gate state is held");
    assert!(row.original_id.contains(original), "{row:?}");
    assert!(!row.root_finished && !row.outstanding.is_empty(), "{row:?}");
    assert!(
        has_admission(&row),
        "real handler gate-state wait lost its origin: {row:?}"
    );
}

pub(crate) fn idle(gate: &AdmissionGate) -> bool {
    gate.state.lock().unwrap().active == 0
}

struct UnusedBackend;

#[async_trait]
impl UpdateBackend for UnusedBackend {
    async fn latest(&self) -> Result<Release, String> {
        Err("this admission-only fixture must not query a release".into())
    }

    async fn install(&self, _: &Release, _: &str) -> Result<(), String> {
        Err("this admission-only fixture must not launch a helper".into())
    }
}

pub(crate) fn service(root: &Path) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("store/update.json"),
                result_path: root.join("updates/result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(86400),
            },
            Arc::new(UnusedBackend),
        )
        .unwrap(),
    )
}
