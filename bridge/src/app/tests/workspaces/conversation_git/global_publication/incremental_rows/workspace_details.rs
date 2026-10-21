use super::*;
use crate::app::snapshot_projection::work_counts::detail_scans;
use std::collections::BTreeMap;

fn warm_detail_fixture(state: &mut AppState) {
    seed_clocks(state);
    let projects = state
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| (row.project_id.clone(), row.render()))
        .collect();
    state.install_project_snapshot_observations(projects);
    state.warm_snapshot_observations_for_test();
    for owner in state.runs.keys() {
        state.mark_snapshot_owner_pending(owner);
    }
    state.publish_all_snapshot_projection();
}

fn agent_id(state: &AppState, owner: &str, index: usize) -> String {
    state.runs[owner]
        .agents
        .iter()
        .nth(index)
        .unwrap()
        .id
        .clone()
}

fn detail_rows(view: &ReadView) -> BTreeMap<String, Value> {
    view.keys()
        .filter(|key| key.method == "workspace.get")
        .map(|key| {
            (
                key.owner.clone(),
                view.get_owned(key.method, &key.owner).unwrap(),
            )
        })
        .collect()
}

fn assert_detail_full_oracle(state: &AppState, actual: &ReadView) {
    let expected = detail_rows(actual);
    state.publish_all_snapshot_projection();
    assert_eq!(expected, detail_rows(&state.read_view.load()));
}

fn active_detail_measurement(size: usize) -> (usize, usize, usize, detail_scans::Counts) {
    let (_tmp, mut state, _) = ownership_fixture(size, false);
    add_tip_agents(&mut state, 1);
    for number in 0..12 {
        state
            .runs
            .get_mut(&format!("run-{number:02}"))
            .unwrap()
            .agents
            .iter_mut()
            .next()
            .unwrap()
            .working_since = Some("2999-01-01T00:00:00Z".into());
    }
    warm_detail_fixture(&mut state);
    let old = state.read_view.load();
    let retained = detail_rows(&old);
    let workspaces = state.workspaces.list(None).len();
    let agent = agent_id(&state, "run-00", 0);
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&agent)
        .unwrap()
        .topic = Some("one changed owner with incidental active clocks".into());
    let (_, count) = detail_scans::measure(|| state.publish_owner_snapshot_projection("run-00"));
    let actual = state.read_view.load();
    assert_eq!(
        actual.get_owned("workspace.get", "workspace-00").unwrap()["agents"][0]["topic"],
        "one changed owner with incidental active clocks"
    );
    assert_eq!(
        actual.get_owned("workspace.get", "workspace-11").unwrap()["agents"][0]["working_time"]
            ["since"],
        "2999-01-01T00:00:00Z"
    );
    assert_eq!(
        actual.get_owned("workspace.get", "workspace-11").unwrap()["agents"][0]["working_time"]
            ["seconds"],
        0
    );
    assert_eq!(
        detail_rows(&old),
        retained,
        "the accepted old root must remain immutable"
    );
    assert!(old
        .get_shared("workspace.get", "workspace-31")
        .unwrap()
        .ptr_eq(actual.get_shared("workspace.get", "workspace-31").unwrap()));
    assert_detail_full_oracle(&state, &actual);
    let (_, full) = detail_scans::measure(|| state.publish_snapshot_projection());
    assert_eq!(
        full.registry_scans, 1,
        "full None refresh must still enumerate the registry"
    );
    assert_eq!(full.registry_rows, workspaces);
    (size, workspaces, retained.len(), count)
}

fn assert_one_dependency_scan(
    workspaces: usize,
    previous_keys: usize,
    count: detail_scans::Counts,
) {
    assert_eq!(
        count.registry_scans, 1,
        "expanded owners repeated dependency scans"
    );
    assert_eq!(
        count.registry_rows, workspaces,
        "each current workspace must be considered once"
    );
    assert!(
        count.previous_bases <= workspaces,
        "old workspace bases were rescanned"
    );
    assert!(
        count.tombstone_keys <= previous_keys,
        "old detail tombstones were rescanned"
    );
    assert_eq!(
        count.base_renders, 12,
        "each requested or incidental owner must render once"
    );
    assert_eq!(
        count.variant_renders, 12,
        "each affected agent variant must render once"
    );
}

#[test]
fn workspace_detail_batch_scans_once_for_twelve_active_owners() {
    // Observe both real producer sizes before reporting the count regression.
    let measurements = [32, 190].map(active_detail_measurement);
    for (size, workspaces, previous_keys, count) in measurements {
        eprintln!("{size} workspaces / 12 active owner detail producer: {count:?}");
        assert_one_dependency_scan(workspaces, previous_keys, count);
    }
}

fn alias_detail_fixture() -> (tempfile::TempDir, AppState, String, String) {
    let (tmp, mut state, _) = ownership_fixture(8, false);
    add_tip_agents(&mut state, 2);
    let canonical = agent_id(&state, "run-00", 0);
    let alias = agent_id(&state, "run-01", 1);
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&canonical)
        .unwrap()
        .thread
        .post_agent("canonical opening", None, "2026-01-01T00:00:00Z");
    let row = state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap();
    row.thread
        .post_agent("owned fallback", None, "2026-01-01T00:00:00Z");
    row.bind_conversation(&canonical);
    let shared = state.runs["run-00"].worktree.path.clone();
    state.runs.get_mut("run-01").unwrap().worktree.path = shared.clone();
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    for id in ["run-00", "run-01", "removed-workspace"] {
        state.workspaces.adopt_root(
            &project,
            id.into(),
            id.into(),
            shared.clone(),
            "source-1".into(),
            false,
        );
    }
    warm_detail_fixture(&mut state);
    (tmp, state, canonical, alias)
}

#[test]
fn workspace_detail_batch_renders_shared_root_and_canonical_alias_closure_once() {
    let (_tmp, mut state, canonical, alias) = alias_detail_fixture();
    let old = state.read_view.load();
    let retained = detail_rows(&old);
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&canonical)
        .unwrap()
        .thread
        .post_agent("new canonical message", None, "2026-01-01T00:00:01Z");
    let (_, count) = detail_scans::measure(|| state.publish_owner_snapshot_projection("run-00"));
    let actual = state.read_view.load();
    let alias_key = format!("run-01/{alias}");
    assert_eq!(
        old.get_owned("workspace.get", &alias_key).unwrap()["thread"]["last_sequence"],
        1
    );
    assert_eq!(
        actual.get_owned("workspace.get", &alias_key).unwrap()["thread"]["last_sequence"],
        2
    );
    assert_eq!(
        actual.get_owned("workspace.get", "run-00").unwrap()["run_id"],
        "run-00"
    );
    assert_eq!(
        actual.get_owned("workspace.get", "run-01").unwrap()["run_id"],
        "run-01"
    );
    assert_eq!(detail_rows(&old), retained);
    assert_detail_full_oracle(&state, &actual);
    eprintln!("shared-root alias detail producer: {count:?}");
    assert_eq!(count.registry_scans, 1);
    assert_eq!(
        count.base_renders, 4,
        "overlapping current/previous dependencies must render each base once"
    );
    assert_eq!(
        count.variant_renders, 8,
        "each overlapping agent variant must render once"
    );
}

#[test]
fn workspace_detail_scopes_keep_previous_aliases_removed_agents_and_tombstones() {
    let (_tmp, mut state, canonical, alias) = alias_detail_fixture();
    let old = state.read_view.load();
    let retained = detail_rows(&old);
    let removed_variant = format!("run-00/{canonical}");
    let alias_key = format!("run-01/{alias}");
    assert!(old.get_shared("workspace.get", &removed_variant).is_some());
    assert!(old
        .get_shared("workspace.get", "removed-workspace")
        .is_some());
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .remove(&canonical)
        .unwrap();
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&alias);
    state.workspaces.forget("removed-workspace");
    let _context = state.begin_snapshot_ownership();
    let delta = state.prepare_workspace_snapshot_details(
        SnapshotDelta::new(state.read_view.next_version()),
        &old,
        Some("run-00"),
    );
    state.publish_prepared_snapshot_projection(delta);
    drop(_context);
    let actual = state.read_view.load();
    assert!(actual
        .get_shared("workspace.get", &removed_variant)
        .is_none());
    assert!(actual.keys().all(|key| key.method != "workspace.get"
        || (key.owner != "removed-workspace" && !key.owner.starts_with("removed-workspace/"))));
    assert_eq!(
        actual.get_owned("workspace.get", &alias_key).unwrap()["thread"]["agent"]["id"],
        alias
    );
    assert_eq!(detail_rows(&old), retained);
    assert!(old
        .get_shared("workspace.get", "workspace-07")
        .unwrap()
        .ptr_eq(actual.get_shared("workspace.get", "workspace-07").unwrap()));
    assert_detail_full_oracle(&state, &actual);
}

mod sharing;
