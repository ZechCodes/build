//! OFFSOURCE child of incremental_rows. Register and observe RED before the
//! separate Layout.state_root production field is applied.
use super::*;

#[test]
fn incremental_owner_layout_rejects_changed_native_state_root() {
    let (tmp, mut app, _) = ownership_fixture(4, false);
    add_tip_agents(&mut app, 1);
    seed_clocks(&mut app);
    app.initialize_snapshot_projection();
    let previous = app.read_view.load();
    {
        let _ownership = app.begin_snapshot_ownership();
        assert!(
            app.prepare_incremental_owner_snapshot(
                SnapshotDelta::new(app.read_view.next_version()),
                &previous,
                &BTreeSet::from(["run-00".into()])
            )
            .is_some(),
            "positive stable layout control"
        );
    }
    app.state_root = tmp.path().join("different-native-state-root");
    assert_fallback(&app);
}
