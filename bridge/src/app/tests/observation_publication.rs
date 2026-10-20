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
            let mut run = app.take_run(&owner).unwrap();
            run.run.plan_id = Some(crate::plan::PlanId::new(&plan));
            if run_index == 1 {
                run.run.state = RunState::Archived;
            }
            app.finish_run_mutation(owner.clone(), run).unwrap();
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

#[test]
fn observation_sparse_versions_merge_disjoint_keys_and_reject_old_replacements() {
    use crate::app::observation_reads::ObservationPatch;
    use crate::app::read_view::{ReadViewSlot, SnapshotDelta};
    let (_dir, state, first) = fixture();
    capture_metrics::take();
    let app = state.lock().unwrap();
    let second = app.runs.keys().find(|id| **id != first).unwrap().clone();
    let initial = app.read_view.load().observations.clone().unwrap();
    let slot = ReadViewSlot::default();
    let base = app.read_view.next_version();
    assert!(slot.publish_delta(SnapshotDelta::new(base).set_observations(initial.clone())));
    let mut earlier = ObservationPatch::default();
    earlier.owner(&app, &first, &initial);
    let mut later = ObservationPatch::default();
    later.owner(&app, &second, &initial);
    assert!(slot.publish_delta(SnapshotDelta::new(base + 2).patch_observations(later)));
    assert!(slot.publish_delta(SnapshotDelta::new(base + 1).patch_observations(earlier)));
    assert!(!slot.publish_delta(SnapshotDelta::new(base + 1).set_observations(initial)));
    let current = slot.load().observations.clone().unwrap();
    // Replacing a prior sparse root establishes a floor for every key,
    // including keys whose old explicit version was lower than that floor.
    assert!(slot.publish_delta(SnapshotDelta::new(base + 4).set_observations(current.clone())));
    let mut obsolete = ObservationPatch::default();
    obsolete.owner(&app, &first, &current);
    assert!(!slot.publish_delta(SnapshotDelta::new(base + 3).patch_observations(obsolete)));
}

#[test]
fn observation_reset_first_publication_fences_canonical_and_cross_owner_aliases() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, canonical_owner) = planned_run_in_review(&mut app, "canonical reset");
    let (_, alias_owner) = planned_run_in_review(&mut app, "retained alias");
    let (_, unrelated_owner) = planned_run_in_review(&mut app, "unrelated reset witness");
    let canonical = primary_agent_id(&app, &canonical_owner);
    let alias = primary_agent_id(&app, &alias_owner);
    app.edit_agent_record("bind reset witness", &alias_owner, &alias, |agent| {
        agent.bind_conversation(&canonical);
    });
    app.delivery_queue.clear_queued();
    let state = app.shared();
    let _handler = AppState::handler(state.clone());
    let mut app = state.lock().unwrap();
    let initial = app.read_view.load().observations.clone().unwrap();
    let fence = |owner: &str| {
        initial
            .mixed
            .prepare(
                "thread.attachment",
                &json!({
                    "entity_id":owner, "path":".build/attachments/reset-witness.txt"
                }),
                None,
            )
            .unwrap()
            .fence()
    };
    let canonical_fence = fence(&canonical_owner);
    let alias_fence = fence(&alias_owner);
    let unrelated_fence = fence(&unrelated_owner);
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = seen.clone();
    let _observer = AppState::observe_reset_publications(move |view| {
        let context = &view.observations.as_ref().unwrap().mixed;
        assert!(
            context.validate(&canonical_fence).is_err(),
            "canonical old generation survived the first reset publication"
        );
        assert!(
            context.validate(&alias_fence).is_err(),
            "cross-owner alias old generation survived the first reset publication"
        );
        assert!(context.validate(&unrelated_fence).is_ok());
        count.set(count.get() + 1);
    });
    let agent = app
        .entity_agents(&canonical_owner)
        .unwrap()
        .primary()
        .unwrap();
    let params = json!({
        "project_id":app.projects.project_id_of(&canonical_owner),
        "entity_id":canonical_owner,"agent_id":agent.id,
        "conversation_id":agent.conversation_id(),"expected_thread_id":agent.thread.id
    });
    let reply = app.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        seen.get() >= 2,
        "both canonical and alias owners must publish"
    );
}

#[test]
fn observation_rejected_owner_delta_keeps_pending_wire_and_input_retry() {
    use crate::app::read_view::SnapshotDelta;
    let (_dir, state, owner) = fixture();
    capture_metrics::take();
    let mut app = state.lock().unwrap();
    let older = app.prepare_owner_snapshot_projection(&owner).unwrap();
    // An intervening publication wins this owner's wire key. The rejected
    // candidate must retain its pending owner and dirty-input receipt.
    assert!(app
        .read_view
        .publish_delta(SnapshotDelta::new(app.read_view.next_version()).put(
            "agent.list",
            &owner,
            json!({"intervening":true})
        )));
    app.runs.remove(&owner);
    // A permanent removal also retires the committed checkpoint. Leaving it
    // installed would correctly defer publication as a temporarily borrowed owner.
    app.committed_entities.forget(&owner);
    app.publish_prepared_snapshot_projection(older);
    assert_eq!(
        app.read_view.load().get("agent.list", &owner),
        Some(&json!({"intervening":true}))
    );
    app.publish_snapshot_projection();
    let latest = app.read_view.load();
    assert!(
        latest.get("agent.list", &owner).is_none(),
        "rejection discarded pending owner wire tombstone"
    );
    assert!(
        latest
            .observations
            .as_ref()
            .unwrap()
            .scopes
            .prepare("git.status", &json!({"run_id":owner}))
            .is_err(),
        "rejection discarded pending observation tombstone"
    );
}
