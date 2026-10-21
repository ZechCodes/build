//! A task cancellation is not locally published while its board row is stale.
use super::*;
use crate::app::read_view::ReadView;
use std::sync::Mutex;

const ORIGINAL: &str = "qual/lifecycle/borrowed-board-cancel";
const CANCELLATION: &str = "qual/lifecycle/borrowed-board-unassign";
const PEER: &str = "run-saved-intent-publication-peer";

struct BorrowedPeer {
    state: Arc<Mutex<AppState>>,
    active: Option<crate::orchestrator::ActiveRun>,
}
impl BorrowedPeer {
    fn take(fixture: &Fixture) -> Self {
        let active = fixture.state.lock().unwrap().take_run(PEER).unwrap();
        Self {
            state: Arc::clone(&fixture.state),
            active: Some(active),
        }
    }
    fn restore(mut self) -> Result<(), String> {
        self.release()
    }
    fn release(&mut self) -> Result<(), String> {
        if let Some(active) = self.active.take() {
            let mut app = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A successful native unchanged-owner save publishes the pending
            // task rows. Merely restoring the map need not publish anything.
            app.finish_run_mutation(PEER.into(), active)?;
        }
        Ok(())
    }
}
impl Drop for BorrowedPeer {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn board_task(view: &ReadView, task: &str) -> Option<Value> {
    view.get_owned("board.list", "")?["items"]
        .as_array()?
        .iter()
        .find(|item| item["task_id"] == task)
        .cloned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_retry_cancellation_waits_for_accepted_board_behind_borrowed_owner() {
    borrowed_cancellation(ORIGINAL, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_author_waits_for_accepted_board_behind_borrowed_owner() {
    borrowed_cancellation(CANCELLATION, false).await;
}

async fn borrowed_cancellation(observed_original: &'static str, cancelled: bool) {
    let fixture = Fixture::new();
    fixture.add_publication_peer(PEER);
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let after_ack = row(&registry, ORIGINAL, false).await;
    let retained = fixture.state.lock().unwrap().read_view.load();
    let before = board_task(&retained, &fixture.intent.task_id);
    let peer = BorrowedPeer::take(&fixture);
    let cancellation = rpc
        .request(
            0,
            CANCELLATION,
            "tasks.assign",
            json!({"task_id":fixture.intent.task_id,"assignee":null}),
        )
        .await
        .reply;
    let sql = task(&rpc, &fixture, "qual/lifecycle/borrowed-board-task").await;
    let blocked = fixture.state.lock().unwrap().read_view.load();
    let stale = board_task(&blocked, &fixture.intent.task_id);
    let original_pending = row(&registry, observed_original, false).await;

    let (sent, received) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(5),
        Arc::new(move |row| {
            if row.original_id == observed_original {
                let _ = sent.send(row);
            }
        }),
    );
    // Actual published SQL disagrees with the accepted board while an unrelated
    // native owner is borrowed. No test-created obligation or interval exists.
    offset.fetch_add(6, Ordering::SeqCst);
    let observed = received.recv_timeout(Duration::from_secs(2)).ok();
    let restored = peer.restore();
    let accepted = fixture.state.lock().unwrap().read_view.load();
    let published = board_task(&accepted, &fixture.intent.task_id);
    let completed = row(&registry, observed_original, true).await;
    let service = step(&fixture);
    drop(watchdog);
    let effects = finish(fixture, rpc).await;

    // Restore the owner, finish native service and join all owned receivers
    // before asserting accounting. The deliberate fault is never a success.
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({
            "case":"borrowed_board_cancel", "observed_original":observed_original, "offered_semantic":2,
            "expected_cancelled_semantic":1,"expected_cancellation_commands":1,
            "expected_original_publication_faults":1,"qualification_successes":0,
        })
    );
    assert!(
        restored.is_ok(),
        "native peer restoration failed: {restored:?}"
    );
    assert_eq!(reply["ok"], true);
    assert!(after_ack.is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    assert_eq!(cancellation["ok"], true);
    assert_eq!(sql["ok"], true);
    assert!(sql["result"]["task"]["assignee"].is_null());
    assert!(sql["result"]["task"]["deferred_assignment"].is_null());
    let before = before.expect("watched queued task has an accepted board row");
    assert_eq!(before["deferred_assignment"]["state"], "queued");
    assert_eq!(stale.as_ref(), Some(&before));
    assert_eq!(
        board_task(&retained, &effects.original.task_id),
        Some(before.clone())
    );
    let published = published.expect("restoring peer publishes cancelled task row");
    assert!(published["assignee"].is_null() && published["deferred_assignment"].is_null());
    assert_ne!(published, before);
    assert_eq!(service, Some(Ok(false)));
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert_eq!(effects.saved.state, "cancelled");
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));

    let pending =
        original_pending.expect("original survives cancellation ACK while board is stale");
    assert!(
        pending.root_finished && pending.completed_at.is_none() && !pending.outstanding.is_empty()
    );
    assert!(
        pending
            .active
            .iter()
            .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::Publication)),
        "native publication barrier must replace the retired dispatch wait: {pending:?}"
    );
    assert!(
        !pending
            .active
            .iter()
            .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::Dispatch)),
        "cancelled intent is no longer awaiting dispatch: {pending:?}"
    );
    let observed =
        observed.expect("independent watchdog sees original blocked on accepted publication");
    assert_eq!(observed.id, pending.id);
    assert!(observed.breached() && observed.known_wait_union > WAIT_BUDGET);
    assert!(observed
        .active
        .iter()
        .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::Publication)));
    let completed =
        completed.expect("same original is archived after actual accepted board publication");
    assert_eq!(completed.id, pending.id);
    assert!(completed.root_finished && completed.completed_at.is_some());
    assert!(completed.outstanding.is_empty() && completed.active.is_empty());
    assert_eq!(completed.cancelled, cancelled);
    assert!(!completed.abandoned && completed.breached());
}

mod adversarial;
