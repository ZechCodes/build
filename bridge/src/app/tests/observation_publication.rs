//! Producer publication must not rebuild unrelated I/O observation inputs.
use super::*;
use crate::app::observation_reads::capture_metrics;

fn fixture() -> (tempfile::TempDir, Arc<Mutex<AppState>>, String) {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let mut owners = Vec::new();
    for index in 0..3 {
        let plan = file_legacy_task(&mut app, "unrelated retained document");
        for run_index in 0..2 {
            let owner = adopted_run(
                &mut app,
                &repo,
                dir.path(),
                &format!("publication-{index}-{run_index}"),
            );
            let run = app.runs.get_mut(&owner).unwrap();
            run.run.plan_id = Some(crate::plan::PlanId::new(&plan));
            if run_index == 1 {
                run.run.state = RunState::Archived;
            }
            owners.push(owner);
        }
    }
    app.delivery_queue.clear_queued();
    let state = app.shared();
    let _handler = AppState::handler(state.clone());
    capture_metrics::start();
    (dir, state, owners.remove(0))
}

fn assert_scoped(owner: &str) {
    let visits = capture_metrics::take();
    assert!(
        !visits.keys().any(|(family, _)| *family == "full"),
        "owner/status publication recaptured the whole observation graph: {visits:?}"
    );
    assert!(
        visits.keys().all(|(_, key)| key == owner),
        "owner/status publication traversed unrelated observation inputs: {visits:?}"
    );
}

#[test]
fn observation_publication_owner_guard_visits_only_its_declared_input_dependencies() {
    let (_dir, state, owner) = fixture();
    let app = state.lock().unwrap();
    app.publish_owner_snapshot_projection(&owner);
    assert_scoped(&owner);
}

#[test]
fn observation_publication_status_guard_never_captures_the_global_input_graph() {
    let (_dir, state, owner) = fixture();
    let mut app = state.lock().unwrap();
    let agent = app
        .entity_agents(&owner)
        .unwrap()
        .primary()
        .unwrap()
        .id
        .clone();
    let status = crate::harness::SessionStatusSnapshot::new(crate::harness::AgentStatus::Working);
    // This is the production StatusPump's mutation/publication sequence; no
    // context/reset/usage-limit fields change in this ordinary Working sample.
    app.record_agent_status_snapshot(&owner, &agent, &status);
    app.publish_owner_snapshot_projection(&owner);
    assert_scoped(&owner);
}

#[test]
fn observation_publication_board_guard_reuses_unchanged_io_inputs() {
    let (_dir, state, _) = fixture();
    let app = state.lock().unwrap();
    app.note_board_changed();
    let visits = capture_metrics::take();
    assert!(
        visits.is_empty(),
        "board note rebuilt I/O inputs: {visits:?}"
    );
}
