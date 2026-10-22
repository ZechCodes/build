//! Separate fresh dirty closure from the next accepted incidental active set.
use super::*;
use crate::app::snapshot_projection::work_counts::owner_closure as work;

fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|id| (*id).into()).collect()
}

fn agent_id(app: &AppState, owner: &str) -> String {
    app.runs[owner].agents.iter().next().unwrap().id.clone()
}

fn set_working(app: &mut AppState, owner: &str, working: bool) {
    app.runs
        .get_mut(owner)
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .working_since = working.then(|| "2999-01-01T00:00:00Z".into());
}

fn active(root: &ReadView) -> &BTreeSet<String> {
    root.global.as_ref().unwrap().active_owners_for_test()
}

fn output(count: &work::Counts) -> &work::Output {
    assert_eq!(
        count.outputs.len(),
        1,
        "must exercise one fast refresh: {count:?}"
    );
    &count.outputs[0]
}

fn prepare(app: &AppState, owners: &BTreeSet<String>) -> (SnapshotDelta, work::Counts) {
    let _guard = app.begin_snapshot_ownership();
    work::measure(|| {
        app.prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &app.read_view.load(),
            owners,
        )
        .expect("unchanged structural layout must remain fast eligible")
    })
}

fn publish(app: &AppState, owner: &str) -> work::Counts {
    let (_, count) = work::measure(|| app.publish_owner_snapshot_projection(owner));
    output(&count);
    count
}

// Generic Full preserves the same named-agent/history admission as a global
// publication. Incidental active owner refresh does not name unrelated agent.list
// chunks; using FullAll here would change that separate legacy contract.
fn assert_global_reference(app: &AppState, root: &ReadView, requests: &[FactsRequest]) {
    let before = image(root, requests);
    app.publish_snapshot_projection();
    assert_eq!(before, image(&app.read_view.load(), requests));
    assert_eq!(
        before,
        image(root, requests),
        "full reference mutated old root"
    );
}

fn register_retained(app: &mut AppState, owner: &str) {
    let root = app.runs[owner].worktree.path.clone();
    let agent = agent_id(app, owner);
    let key = crate::app::TabKey::agent(&root, &agent);
    let mut tab = dictated_agent_tab(
        &root,
        owner,
        &agent,
        DictatedSession::reporting(crate::harness::AgentStatus::Waiting),
    );
    tab.live = false;
    app.session_registry.test_insert_tab(key, tab);
}

// A count fixture warms once, without persisting N owners through N finishers.
fn count_fixture(count: usize) -> (tempfile::TempDir, AppState) {
    let (tmp, mut app, _) = ownership_fixture(count, false);
    add_tip_agents(&mut app, 1);
    for (owner, run) in &app.runs {
        app.board.attention_mut().restore_entity_clocks(
            owner.clone(),
            CLOCK.into(),
            CLOCK.into(),
            CLOCK.into(),
            crate::app::run_state_str(&run.run.state),
        );
    }
    register_retained(&mut app, "run-01");
    set_working(&mut app, "run-02", true);
    let projects = app
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| (row.project_id.clone(), row.render()))
        .collect();
    app.install_project_snapshot_observations(projects);
    app.warm_snapshot_observations_for_test();
    for owner in app.runs.keys() {
        app.mark_snapshot_owner_pending(owner);
    }
    app.publish_all_snapshot_projection();
    (tmp, app)
}

#[test]
fn incremental_owner_closure_actual_source_probe_covers_full_seed() {
    let (_tmp, app) = count_fixture(4);
    let requests = requests(&app);
    let old = app.read_view.load();
    let retained = image(&old, &requests);
    let (_, count) = work::measure(|| app.publish_snapshot_projection());
    let expected = (0..4)
        .map(|number| (format!("run-{number:02}"), 1))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(count.index_builds, 1);
    assert_eq!(count.index_owners, 4);
    assert_eq!(count.discoveries, 1);
    assert_eq!(count.discovery_owners, 4);
    assert_eq!(count.registry_tabs, 1);
    assert_eq!(count.indexed_by_owner, expected);
    assert_eq!(count.discovered_by_owner, expected);
    assert!(
        count.outputs.is_empty(),
        "full seed is not an incremental refresh"
    );
    assert_eq!(active(&app.read_view.load()), &ids(&["run-01", "run-02"]));
    assert_eq!(retained, image(&app.read_view.load(), &requests));
    assert_eq!(retained, image(&old, &requests));
}

fn measure_owner_and_task(size: usize) -> (work::Counts, work::Counts) {
    let (_tmp, mut app) = count_fixture(size);
    let requests = requests(&app);
    let old = app.read_view.load();
    let retained = image(&old, &requests);
    assert_eq!(active(&old), &ids(&["run-01", "run-02"]));
    app.runs.get_mut("run-00").unwrap().run.goal = "one explicitly changed goal".into();
    let owner = publish(&app, "run-00");
    assert_eq!(output(&owner).dirty, ids(&["run-00", "run-01", "run-02"]));
    assert_eq!(output(&owner).active, ids(&["run-01", "run-02"]));
    let root = app.read_view.load();
    assert!(format!("{:?}", image(&root, &requests)).contains("one explicitly changed goal"));
    assert_global_reference(&app, &root, &requests);
    assert_eq!(retained, image(&old, &requests));

    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.install_snapshot_watched_task(
        &project,
        "closure-task",
        Some(json!({
            "kind":"tracker_task", "task_id":"closure-task", "title":"new closure task",
            "anchor":CLOCK,
        })),
    );
    let (_, task) = work::measure(|| app.publish_task_snapshot_projection());
    assert_eq!(output(&task).dirty, ids(&["run-01", "run-02"]));
    assert_eq!(output(&task).active, ids(&["run-01", "run-02"]));
    let root = app.read_view.load();
    assert!(root.get_owned("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == "closure-task" && row["title"] == "new closure task"));
    assert_global_reference(&app, &root, &requests);
    (owner, task)
}

fn assert_complete_source_coverage(size: usize, label: &str, count: &work::Counts) {
    eprintln!("{size} {label}: {count:?}");
    assert!(count.index_builds > 0 && count.discoveries > 0);
    let index_visits = (0..size)
        .map(|number| (format!("run-{number:02}"), count.index_builds))
        .collect::<BTreeMap<_, _>>();
    let discovery_visits = (0..size)
        .map(|number| (format!("run-{number:02}"), count.discoveries))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(count.indexed_by_owner, index_visits);
    assert_eq!(count.discovered_by_owner, discovery_visits);
    assert_eq!(count.index_owners, size * count.index_builds);
    assert_eq!(count.discovery_owners, size * count.discoveries);
    assert_eq!(count.registry_tabs, count.discoveries);
}

fn assert_once_only_work(size: usize, label: &str, count: &work::Counts) {
    assert!(count.index_owners > 0 && count.discovery_owners > 0 && count.registry_tabs > 0);
    assert_eq!(
        count.index_builds, 1,
        "{size} {label}: repeated actual index construction"
    );
    assert_eq!(
        count.index_owners, size,
        "{size} {label}: actual index source owner visits"
    );
    assert_eq!(
        count.discoveries, 1,
        "{size} {label}: repeated active discovery"
    );
    assert_eq!(
        count.discovery_owners, size,
        "{size} {label}: actual active-source owner visits"
    );
    assert_eq!(
        count.registry_tabs, 1,
        "{size} {label}: actual retained-tab source visits"
    );
    let expected_visits = (0..size)
        .map(|number| (format!("run-{number:02}"), 1))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        count.indexed_by_owner, expected_visits,
        "complete actual index-owner traversal"
    );
    assert_eq!(
        count.discovered_by_owner, expected_visits,
        "complete actual discovery-owner traversal"
    );
}

#[test]
fn incremental_owner_closure_builds_and_discovers_once_for_owner_and_task() {
    let mut measured = Vec::new();
    for size in [32, 200] {
        let (owner, task) = measure_owner_and_task(size);
        measured.push((size, owner, task));
    }
    // Print and prove complete source coverage for ALL cohorts before any
    // optimization bound can fail the RED run at its first cohort.
    for (size, owner, task) in &measured {
        for (label, count) in [("owner", owner), ("task", task)] {
            assert_complete_source_coverage(*size, label, count);
        }
    }
    // Both sizes and both real producer semantics are checked before work RED.
    for (size, owner, task) in measured {
        for (label, count) in [("owner", owner), ("task", task)] {
            assert_once_only_work(size, label, &count);
        }
    }
}

#[test]
fn incremental_owner_closure_keeps_prior_active_only_for_one_cleanup_publication() {
    let (_tmp, mut app) = fixture();
    set_working(&mut app, "run-01", true);
    app.publish_owner_snapshot_projection("run-01");
    let working = app.read_view.load();
    let requests = requests(&app);
    let retained = image(&working, &requests);
    assert_eq!(active(&working), &ids(&["run-01"]));
    let working_wire = working
        .facts
        .state_for_test("run-01")
        .unwrap()
        .materialize();
    assert_eq!(working_wire["working"], true);
    assert_eq!(working_wire["working_time"]["seconds"], 0);
    set_working(&mut app, "run-01", false);
    let cleanup = publish(&app, "run-00");
    assert_eq!(output(&cleanup).dirty, ids(&["run-00", "run-01"]));
    assert_eq!(output(&cleanup).active, BTreeSet::new());
    assert_eq!(active(&app.read_view.load()), &BTreeSet::new());
    assert_eq!(
        app.read_view
            .load()
            .facts
            .state_for_test("run-01")
            .unwrap()
            .materialize()["working"],
        false
    );
    assert_global_reference(&app, &app.read_view.load(), &requests);
    let unrelated = publish(&app, "run-00");
    assert_eq!(output(&unrelated).dirty, ids(&["run-00"]));
    assert_eq!(output(&unrelated).active, BTreeSet::new());
    assert_global_reference(&app, &app.read_view.load(), &requests);
    assert_eq!(retained, image(&working, &requests));
    assert_eq!(active(&working), &ids(&["run-01"]));
}

#[test]
fn incremental_owner_closure_preserves_agent_id_alias_and_native_root_edges() {
    let (tmp, mut app) = fixture_with_count(7);
    #[cfg(unix)]
    let shared = {
        use std::os::unix::ffi::OsStringExt;
        tmp.path()
            .join(std::ffi::OsString::from_vec(b"shared-\xff".to_vec()))
    };
    #[cfg(not(unix))]
    let shared = tmp.path().join("shared-native-root");
    std::fs::create_dir_all(&shared).unwrap();
    for owner in ["run-00", "run-01", "run-06"] {
        app.runs.get_mut(owner).unwrap().worktree.path = shared.clone();
    }
    let other_root = tmp.path().join("other-project");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = app.add_project(other_root, "main".into());
    app.projects.bind_entity("run-06".into(), other);
    // Canonical owner is itself rebound. Own agent-ID edges still matter.
    let zero = agent_id(&app, "run-00");
    let two = agent_id(&app, "run-02");
    let four = agent_id(&app, "run-04");
    for (owner, target) in [("run-00", &four), ("run-02", &zero), ("run-03", &two)] {
        app.runs
            .get_mut(owner)
            .unwrap()
            .agents
            .iter_mut()
            .next()
            .unwrap()
            .bind_conversation(target);
    }
    set_working(&mut app, "run-00", true);
    app.publish_all_snapshot_projection();
    let requests = requests(&app);
    let old = app.read_view.load();
    let retained = image(&old, &requests);
    let expected = ids(&["run-00", "run-01", "run-02", "run-03", "run-04"]);
    assert_eq!(active(&old), &expected);
    let (candidate, count) = prepare(&app, &ids(&["run-05", "unknown-owner"]));
    let mut dirty = expected.clone();
    dirty.extend(ids(&["run-05", "unknown-owner"]));
    assert_eq!(output(&count).dirty, dirty);
    assert_eq!(output(&count).active, expected);
    assert!(
        !output(&count).dirty.contains("run-06"),
        "same root in a foreign project is not an edge"
    );
    app.publish_prepared_snapshot_projection(candidate);
    assert_global_reference(&app, &app.read_view.load(), &requests);
    assert_eq!(retained, image(&old, &requests));
}

#[test]
fn incremental_owner_closure_discovers_clockless_and_retained_sessions() {
    let (_tmp, mut app) = fixture();
    register_retained(&mut app, "run-02");
    app.publish_all_snapshot_projection();
    app.board.attention_mut().remove_entity_clocks("run-01");
    assert!(app.board.attention().clock("run-01").created_at.is_none());
    let (candidate, count) = prepare(&app, &ids(&["run-00"]));
    assert_eq!(output(&count).dirty, ids(&["run-00", "run-01", "run-02"]));
    assert_eq!(output(&count).active, ids(&["run-01", "run-02"]));
    assert!(
        count.registry_tabs > 0,
        "retained nonlive registry source is observed"
    );
    app.publish_prepared_snapshot_projection(candidate);
    assert_eq!(active(&app.read_view.load()), &ids(&["run-01", "run-02"]));
    // Clockless attention renders use wall-clock fallback; compare closure
    // literally instead of pretending two sequential full renders are timeless.
    for owner in ["run-01", "run-02"] {
        assert!(app.read_view.load().facts.state_for_test(owner).is_some());
    }
}

#[test]
fn incremental_owner_closure_retains_state_stat_and_structural_fallback_rules() {
    let (_tmp, mut app) = fixture();
    app.runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .state = crate::agent::AgentLifecycle::Ended;
    let effects = app.board.diff_mut().store_run_stat_for_adapter(
        "run-02".into(),
        json!({"files_changed":7,"insertions":11,"deletions":3}),
        std::time::Instant::now(),
    );
    drop(effects); // Install an observed token without a separate publication.
    let count = publish(&app, "run-00");
    assert_eq!(output(&count).dirty, ids(&["run-00", "run-01", "run-02"]));
    assert!(output(&count).active.is_empty());
    let requests = requests(&app);
    let root = app.read_view.load();
    assert_eq!(
        root.facts.state_for_test("run-02").unwrap().materialize()["stat"]["files_changed"],
        7
    );
    assert_global_reference(&app, &root, &requests);
    app.runs.get_mut("run-02").unwrap().run.state = crate::run::RunState::Merged;
    let (_, count) = work::measure(|| app.publish_owner_snapshot_projection("run-00"));
    assert!(
        count.outputs.is_empty(),
        "exact lifecycle shape must still choose full fallback"
    );
    assert_global_reference(&app, &app.read_view.load(), &requests);
}

fn assert_rejected_candidate_keeps_active_set(app: &mut AppState) {
    set_working(app, "run-01", true);
    let (rejected, count) = prepare(app, &ids(&["run-00"]));
    assert_eq!(output(&count).active, ids(&["run-01"]));
    assert!(
        active(&app.read_view.load()).is_empty(),
        "preparation must not mutate accepted active state"
    );
    set_working(app, "run-01", false);
    publish(app, "run-02");
    let accepted = app.read_view.load();
    assert!(active(&accepted).is_empty());
    app.publish_prepared_snapshot_projection(rejected);
    assert!(
        Arc::ptr_eq(&accepted, &app.read_view.load()),
        "older candidate must reject atomically"
    );
    let retry = publish(app, "run-00");
    assert_eq!(output(&retry).dirty, ids(&["run-00"]));
    assert!(output(&retry).active.is_empty());
}

fn assert_failed_finisher_restores_active_set(app: &mut AppState, requests: &[FactsRequest]) {
    let mut failed = app.take_run("run-01").unwrap();
    let committed_goal = failed.run.goal.clone();
    assert!(failed.agents.iter().next().unwrap().working_since.is_none());
    failed.agents.iter_mut().next().unwrap().working_since = Some("2999-01-01T00:00:00Z".into());
    failed.run.goal = "UNCOMMITTED ACTIVE CANDIDATE".into();
    let retained = app.read_view.load();
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation("run-01".into(), failed).is_err());
    let restored = &app.runs["run-01"];
    assert_eq!(restored.run.goal, committed_goal);
    assert!(restored
        .agents
        .iter()
        .next()
        .unwrap()
        .working_since
        .is_none());
    assert!(Arc::ptr_eq(&retained, &app.read_view.load()));
    assert!(active(&app.read_view.load()).is_empty());
    assert_eq!(
        image(&retained, requests),
        image(&app.read_view.load(), requests)
    );
    // Preparing the failed finisher marked run-01 pending. Its committed
    // restored row must be retried, while the failed candidate's active set
    // must never become accepted. Only that accepted retry clears the receipt.
    let retry = publish(app, "run-02");
    assert_eq!(output(&retry).dirty, ids(&["run-01", "run-02"]));
    assert!(output(&retry).active.is_empty());
    assert!(active(&app.read_view.load()).is_empty());
    let unrelated = publish(app, "run-02");
    assert_eq!(output(&unrelated).dirty, ids(&["run-02"]));
    assert!(output(&unrelated).active.is_empty());
}

#[test]
fn incremental_owner_closure_rejected_or_failed_candidate_cannot_replace_active_set() {
    let (_tmp, mut app) = fixture();
    let requests = requests(&app);
    let old = app.read_view.load();
    let before = image(&old, &requests);
    assert_rejected_candidate_keeps_active_set(&mut app);
    assert_failed_finisher_restores_active_set(&mut app, &requests);
    assert_global_reference(&app, &app.read_view.load(), &requests);
    assert_eq!(before, image(&old, &requests));
}
