//! Producer publication must not rebuild unrelated I/O observation inputs.
use super::*;
use crate::app::observation_reads::capture_metrics;
use crate::app::read_view::{ReadView, SnapshotDelta};
use crate::app::snapshot_projection::work_counts;
use std::collections::BTreeSet;

/// Isolate incremental input publication from the separately tested bootstrap
/// tail. This installs real captured inputs and acknowledges their dirty keys.
pub(in crate::app::tests) fn initialize_observation_test_baseline(app: &AppState) {
    let previous = app.read_view.load();
    assert!(
        previous.global.is_some(),
        "fixture needs an accepted global cache"
    );
    let delta =
        app.prepare_observation_inputs(SnapshotDelta::new(app.read_view.next_version()), &previous);
    app.publish_prepared_snapshot_projection(delta);
    assert!(app.read_view.load().observations.is_some());
}

pub(in crate::app::tests) fn assert_observation_fast_owner(app: &AppState, owner: &str) {
    let _context = app.begin_snapshot_ownership();
    let previous = app.read_view.load();
    assert!(
        previous.global.is_some(),
        "fast-path assertion needs an accepted cache"
    );
    assert!(
        app.prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &previous,
            &BTreeSet::from([owner.to_owned()]),
        )
        .is_some(),
        "stable owner must be eligible for incremental publication"
    );
}

fn restart_capture() {
    capture_metrics::take();
    capture_metrics::start();
}

fn assert_owner_capture(
    visits: &std::collections::BTreeMap<(&'static str, String), usize>,
    owner: &str,
) {
    for family in ["scope_run", "thread_owner"] {
        assert!(
            visits
                .get(&(family, owner.to_owned()))
                .copied()
                .unwrap_or(0)
                > 0,
            "missing real {family} capture for {owner}: {visits:?}"
        );
    }
}

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
    initialize_observation_test_baseline(&state.lock().unwrap());
    capture_metrics::start();
    (dir, state, owners.remove(0))
}

fn assert_scoped(owner: &str) {
    let visits = capture_metrics::take();
    assert_owner_capture(&visits, owner);
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
    assert_observation_fast_owner(&app, &owner);
    restart_capture();
    let (_, counts) = work_counts::measure(|| app.publish_owner_snapshot_projection(&owner));
    assert_eq!(
        counts.workspace_details_full, 0,
        "actual owner publication fell back: {counts:?}"
    );
    assert_scoped(&owner);
}

#[test]
fn observation_publication_status_guard_never_captures_the_global_input_graph() {
    let (_dir, state, owner) = fixture();
    let mut app = state.lock().unwrap();
    assert_observation_fast_owner(&app, &owner);
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
    restart_capture();
    let (_, counts) = work_counts::measure(|| {
        app.record_agent_status_snapshot(&owner, &agent, &status);
        app.publish_owner_snapshot_projection(&owner);
    });
    assert_eq!(
        counts.workspace_details_full, 0,
        "actual status publication fell back: {counts:?}"
    );
    assert_scoped(&owner);
}

#[test]
fn observation_publication_bootstrap_initializes_observation_inputs() {
    let (dir, repo) = init_repo();
    let app = qa_state(&repo, dir.path());
    let initial = app.read_view.load();
    assert!(initial.global.is_some());
    assert!(
        initial.observations.is_some(),
        "real bootstrap omitted observation inputs"
    );
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
    let (_dir, state, owner) = fixture();
    let mut app = state.lock().unwrap();
    assert_observation_fast_owner(&app, &owner);
    restart_capture();
    let (older, counts) =
        work_counts::measure(|| app.prepare_owner_snapshot_projection(&owner).unwrap());
    let captures = capture_metrics::take();
    assert_eq!(counts.workspace_details_full, 0);
    let installed = advance_owner_input_only(&app, &owner);
    app.publish_prepared_snapshot_projection(older);
    assert!(
        Arc::ptr_eq(&installed, &app.read_view.load()),
        "same-owner observation version must reject wire, facts and cache together"
    );
    assert!(
        owner_input_is_dirty(&app, &owner),
        "rejection acknowledged dirty owner input"
    );
    assert_owner_capture(&captures, &owner);
    app.runs.remove(&owner);
    // A permanent removal also retires the committed checkpoint. Leaving it
    // installed would correctly defer publication as a temporarily borrowed owner.
    app.committed_entities.forget(&owner);
    capture_metrics::start();
    app.publish_snapshot_projection();
    let retries = capture_metrics::take();
    assert_owner_capture(&retries, &owner);
    assert!(
        !owner_input_is_dirty(&app, &owner),
        "accepted retry retained dirty input"
    );
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

fn owner_input_is_dirty(app: &AppState, owner: &str) -> bool {
    app.snapshot_observations
        .observation_owners
        .snapshot()
        .iter()
        .any(|(id, _)| id == owner)
}

fn advance_owner_input_only(app: &AppState, owner: &str) -> Arc<ReadView> {
    use crate::app::observation_reads::ObservationPatch;
    let previous = app.read_view.load();
    let mut patch = ObservationPatch::default();
    patch.owner(app, owner, previous.observations.as_ref().unwrap());
    assert!(app
        .read_view
        .publish_delta(SnapshotDelta::new(app.read_view.next_version()).patch_observations(patch)));
    let installed = app.read_view.load();
    assert!(Arc::ptr_eq(&previous.facts, &installed.facts));
    assert!(Arc::ptr_eq(
        previous.global.as_ref().unwrap(),
        installed.global.as_ref().unwrap()
    ));
    assert_eq!(
        previous.get_owned("agent.list", owner),
        installed.get_owned("agent.list", owner)
    );
    installed
}

#[test]
fn incremental_task_publication_carries_and_retries_pending_observation_receipts() {
    let (_dir, state, owner) = fixture();
    let app = state.lock().unwrap();
    let project = app.projects.project_id_of(&owner).unwrap().to_owned();
    app.install_snapshot_watched_task(
        &project,
        "observation-task",
        Some(json!({
            "kind":"tracker_task","task_id":"observation-task","title":"accepted on retry",
            "anchor":"2026-01-01T00:00:00Z"
        })),
    );
    app.mark_observation_owner_changed(&owner);
    let previous = app.read_view.load();
    app.flush_pending_snapshot_projection();
    assert!(
        Arc::ptr_eq(&previous, &app.read_view.load()),
        "task case must have no pending wire owners"
    );
    restart_capture();
    let (older, count) = work_counts::measure(|| {
        app.prepare_incremental_task_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &previous,
        )
        .expect("stable task write must use the task-fast builder")
    });
    let captures = capture_metrics::take();
    assert_eq!(count.workspace_details_full, 0);
    let installed = advance_owner_input_only(&app, &owner);
    app.publish_prepared_snapshot_projection(older);
    assert!(
        Arc::ptr_eq(&installed, &app.read_view.load()),
        "stale task inputs installed cache/wire/facts"
    );
    assert!(owner_input_is_dirty(&app, &owner));
    assert_owner_capture(&captures, &owner);
    capture_metrics::start();
    app.publish_task_snapshot_projection();
    assert_owner_capture(&capture_metrics::take(), &owner);
    assert!(!owner_input_is_dirty(&app, &owner));
    let latest = app.read_view.load();
    let board = latest.get_owned("board.list", "").unwrap();
    assert_eq!(
        board["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["task_id"] == "observation-task")
            .unwrap()["title"],
        "accepted on retry"
    );
    assert!(installed.get_owned("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["task_id"] != "observation-task"));
}

#[test]
fn incremental_task_publication_without_input_changes_retains_observation_arc() {
    let (_dir, state, owner) = fixture();
    capture_metrics::take();
    let mut app = state.lock().unwrap();
    let project = app.projects.project_id_of(&owner).unwrap().to_owned();
    let initial = app.read_view.load();
    capture_metrics::start();
    let (created, count) = work_counts::measure(|| {
        app.tasks_create(&json!({
            "project_id":project,"title":"real task with unchanged I/O inputs"
        }))
        .unwrap()
    });
    assert_eq!(count.workspace_details_full, 0);
    assert!(
        capture_metrics::take().is_empty(),
        "task-only publication recaptured I/O inputs"
    );
    let latest = app.read_view.load();
    assert!(Arc::ptr_eq(
        initial.observations.as_ref().unwrap(),
        latest.observations.as_ref().unwrap()
    ));
    assert_ne!(
        initial.get_owned("board.list", ""),
        latest.get_owned("board.list", "")
    );
    let board = latest.get_owned("board.list", "").unwrap();
    assert!(board["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == created["task"]["id"]));
}
