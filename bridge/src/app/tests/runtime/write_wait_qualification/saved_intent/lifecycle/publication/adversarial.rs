//! Native adversarial task-publication controls; never qualification successes.
use super::*;
use crate::app::read_view::SnapshotDelta;
use crate::app::snapshot_projection::publication_debt::observe_install_receipts;
use crate::timing::write_wait::current_context;

mod cuts;
mod identity;
mod provenance;
mod tail;

impl BorrowedPeer {
    /// Keep the actual unchanged owner, but let the real prepare/publish seam
    /// run separately. This mirrors the existing owner-publication controls.
    fn restore_for_candidate(mut self) {
        let active = self.active.take().unwrap();
        self.state.lock().unwrap().runs.insert(PEER.into(), active);
    }
}

fn prepare(fixture: &Fixture) -> SnapshotDelta {
    fixture
        .state
        .lock()
        .unwrap()
        .prepare_all_snapshot_projection()
        .expect("restored native owners permit a full candidate")
}
fn publish(fixture: &Fixture, delta: SnapshotDelta) {
    fixture
        .state
        .lock()
        .unwrap()
        .publish_prepared_snapshot_projection(delta);
}
fn settle_peer(fixture: &Fixture) -> Result<(), String> {
    let mut app = fixture.state.lock().unwrap();
    let active = app.take_run(PEER)?;
    app.finish_run_mutation(PEER.into(), active)
}
fn snapshot(registry: &WriteWaitRegistry, original: &str) -> Option<WriteSnapshot> {
    registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == original)
}
fn assert_pending(row: &Option<WriteSnapshot>, original: &str) {
    let row = row
        .as_ref()
        .unwrap_or_else(|| panic!("missing original {original}"));
    assert_eq!(row.original_id, original);
    assert!(
        row.root_finished && row.completed_at.is_none() && !row.outstanding.is_empty(),
        "{row:?}"
    );
}
fn assert_no_delivery(effects: &Effects) {
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert_eq!(effects.saved.state, "cancelled");
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));
}
