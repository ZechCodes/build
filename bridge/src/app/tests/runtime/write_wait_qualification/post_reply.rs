//! OFFSOURCE: required delivery remains owed after the original final reply.
use super::rpc::RpcHarness;
use crate::app::tests::runtime::run_agents::insert_run_without_agent;
use crate::app::tests::{
    derived_agent_key, init_repo, qa_state, wait_for_agent_tab, wait_for_deliveries,
};
use crate::app::{AppState, TabKey};
use crate::timing::write_wait::{WriteSnapshot, WriteWaitRegistry, WAIT_BUDGET};
use serde_json::json;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SAFETY: Duration = Duration::from_secs(20);
const OWNER: &str = "run-logical-required-delivery";

struct HeldApp {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HeldApp {
    fn take(state: Arc<Mutex<AppState>>) -> Self {
        let (held, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            let _ = held.send(());
            let _ = released.recv_timeout(SAFETY);
        });
        let holder = Self {
            release: Some(release),
            thread: Some(thread),
        };
        ready.recv_timeout(SAFETY).expect("owned holder starts");
        holder
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("holder exits without poisoning app");
        }
    }
}
impl Drop for HeldApp {
    fn drop(&mut self) {
        self.release();
    }
}

pub(in crate::app::tests::runtime::write_wait_qualification) struct DeliveryFixture {
    pub(in crate::app::tests::runtime::write_wait_qualification) _directory: tempfile::TempDir,
    pub(in crate::app::tests::runtime::write_wait_qualification) state: Arc<Mutex<AppState>>,
    pub(in crate::app::tests::runtime::write_wait_qualification) registry: Arc<WriteWaitRegistry>,
    pub(in crate::app::tests::runtime::write_wait_qualification) rpc: Arc<RpcHarness>,
    pub(in crate::app::tests::runtime::write_wait_qualification) key: TabKey,
    pub(in crate::app::tests::runtime::write_wait_qualification) spawned: mpsc::Receiver<()>,
    pub(in crate::app::tests::runtime::write_wait_qualification) spawn_release: mpsc::Sender<()>,
    pub(in crate::app::tests::runtime::write_wait_qualification) waits:
        tokio::sync::mpsc::UnboundedReceiver<String>,
}
impl DeliveryFixture {
    pub(in crate::app::tests::runtime::write_wait_qualification) fn new() -> Self {
        let (directory, repo) = init_repo();
        let state = qa_state(&repo, directory.path()).shared();
        let root = insert_run_without_agent(&state, &repo, directory.path().join("side"), OWNER);
        let key = derived_agent_key(&root, OWNER);
        let (arrived, spawned) = mpsc::channel();
        let (spawn_release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));
        state.lock().unwrap().session_locator_factory = Arc::new(move |_, _| {
            let _ = arrived.send(());
            // Actual production locator callback, outside the app lock. An
            // infrastructure timeout releases it without poisoning any lock.
            let _ = released.lock().unwrap().recv_timeout(SAFETY);
            None
        });
        let (waiting, waits) = tokio::sync::mpsc::unbounded_channel();
        state
            .lock()
            .unwrap()
            .frame_clock
            .observe_lock_waits(Arc::new(move |method, _| {
                let _ = waiting.send(method.to_owned());
            }));
        let registry = WriteWaitRegistry::new();
        let rpc = Arc::new(RpcHarness::new(Arc::clone(&state), Arc::clone(&registry)));
        Self {
            _directory: directory,
            state,
            registry,
            rpc,
            key,
            spawned,
            spawn_release,
            waits,
        }
    }

    async fn wait_for_lock(&mut self, method: &str) -> bool {
        tokio::time::timeout(SAFETY, async {
            while let Some(next) = self.waits.recv().await {
                if next == method {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
}

struct FaultProof {
    active: Option<WriteSnapshot>,
    completed: Option<WriteSnapshot>,
    watchdog_fired: bool,
    reached_wait: bool,
    pre_reached_wait: bool,
    reply_ok: bool,
    reserved_tab_matches: bool,
    lineage_recorded: bool,
    retained_root_unchanged: bool,
}

async fn delivery_fault(
    pre_wait: Duration,
    post_wait: Duration,
    original: &'static str,
) -> FaultProof {
    let mut fixture = DeliveryFixture::new();
    let retained_root = fixture.state.lock().unwrap().read_view.load();
    let retained_board = retained_root.get_owned("board.list", "");
    let fired = Arc::new(Mutex::new(Vec::new()));
    let reported = Arc::clone(&fired);
    let watchdog = fixture.registry.watchdog(
        Duration::from_millis(20),
        Arc::new(move |row| {
            reported.lock().unwrap().push(row);
        }),
    );
    let mut before_reply = (!pre_wait.is_zero()).then(|| HeldApp::take(Arc::clone(&fixture.state)));
    let rpc = Arc::clone(&fixture.rpc);
    let mut call = tokio::spawn(async move {
        rpc.request(0, original, "agent.start", json!({"id":OWNER}))
            .await
    });
    let pre_reached_wait = if before_reply.is_some() {
        let reached = fixture.wait_for_lock("agent.start").await;
        tokio::time::sleep(pre_wait).await;
        before_reply.as_mut().unwrap().release();
        reached
    } else {
        true
    };
    let reply = match tokio::time::timeout(SAFETY, &mut call).await {
        Ok(reply) => reply.ok(),
        Err(_) => {
            call.abort();
            let _ = call.await;
            None
        }
    };
    let reply_ok = reply
        .as_ref()
        .is_some_and(|proof| proof.reply["ok"] == true);
    let reserved_tab_matches = reply
        .as_ref()
        .is_some_and(|proof| proof.reply["result"]["term_id"] == fixture.key.tab_id);
    let locator_arrived = fixture.spawned.recv_timeout(SAFETY).is_ok();
    // The locator is parked, so old delivery lock pulses cannot be mistaken
    // for the attempt caused by releasing it while our holder is active.
    while fixture.waits.try_recv().is_ok() {}
    let mut holder = HeldApp::take(Arc::clone(&fixture.state));
    let _ = fixture.spawn_release.send(());
    let reached_wait = locator_arrived && fixture.wait_for_lock("agent.deliver").await;
    tokio::time::sleep(post_wait).await;
    let active = fixture
        .registry
        .active_snapshot()
        .iter()
        .find(|row| row.original_id == original)
        .cloned();
    let watchdog_fired = fired
        .lock()
        .unwrap()
        .iter()
        .any(|row| row.original_id == original);
    holder.release();
    wait_for_agent_tab(&fixture.state, &fixture.key).await;
    wait_for_deliveries(&fixture.state).await;
    let (lineage_recorded, session) = {
        let app = fixture.state.lock().unwrap();
        let tab = app.session_registry.test_tab(&fixture.key).unwrap();
        (tab.session_instance.is_some(), Arc::clone(&tab.session))
    };
    session.end();
    let retained_root_unchanged =
        retained_board.is_some() && retained_root.get_owned("board.list", "") == retained_board;
    drop(watchdog);
    let completed = fixture
        .registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == original);
    let rpc = Arc::try_unwrap(fixture.rpc)
        .ok()
        .expect("RPC call joined before cleanup");
    rpc.finish().await;
    FaultProof {
        active,
        completed,
        watchdog_fired,
        reached_wait,
        pre_reached_wait,
        reply_ok,
        reserved_tab_matches,
        lineage_recorded,
        retained_root_unchanged,
    }
}

fn assert_delivered_fault(proof: FaultProof, require_short_intervals: bool) {
    assert!(proof.reply_ok && proof.reserved_tab_matches);
    assert!(proof.reached_wait && proof.pre_reached_wait);
    assert!(
        proof.lineage_recorded,
        "real claim/open/publication settled before assertions"
    );
    assert!(proof.retained_root_unchanged);
    let active = proof
        .active
        .expect("final reply must retain required originating delivery child");
    assert!(
        active.root_finished && !active.outstanding.is_empty(),
        "{active:?}"
    );
    assert!(
        active.acknowledged_at.is_some(),
        "actual final wire reply is an ACK: {active:?}"
    );
    assert!(
        active.blocked_or_sql_union > WAIT_BUDGET && active.breached(),
        "{active:?}"
    );
    assert!(
        proof.watchdog_fired,
        "independent monitor catches post-reply active violation"
    );
    if require_short_intervals {
        assert!(active.max_chargeable_interval < WAIT_BUDGET, "{active:?}");
    }
    let completed = proof
        .completed
        .expect("terminal settlement retains original record");
    assert_eq!(completed.id, active.id);
    assert_eq!(completed.intake, active.intake);
    assert!(completed.outstanding.is_empty() && completed.root_finished && completed.breached());
    assert!(
        completed.active.is_empty(),
        "terminal settlement closes every active interval: {completed:?}"
    );
    assert!(!completed.abandoned && !completed.cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_required_delivery_survives_reply_and_six_second_app_wait() {
    let proof = delivery_fault(
        Duration::ZERO,
        Duration::from_secs(6),
        "post-ack-required-six",
    )
    .await;
    assert_delivered_fault(proof, false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_required_delivery_unions_short_waits_across_reply() {
    let proof = delivery_fault(
        Duration::from_millis(3100),
        Duration::from_millis(3100),
        "post-ack-required-union",
    )
    .await;
    assert_delivered_fault(proof, true);
}
