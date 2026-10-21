use super::*;
use crate::app::snapshot_projection::work_counts::policy_observations;
use crate::compaction::{CompactionScope, CompactionSource};

fn policy_fixture(size: usize) -> (tempfile::TempDir, AppState, String) {
    let (tmp, mut state, _) = ownership_fixture(size, false);
    state.config_path = Some(tmp.path().join("config.json"));
    seed_clocks(&mut state);
    let projects = state
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| (row.project_id.clone(), row.render()))
        .collect();
    state.install_project_snapshot_observations(projects);
    state.warm_snapshot_observations_for_test();
    state.publish_all_snapshot_projection();
    state.persist();
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    (tmp, state, project)
}

fn workspace_policy(view: &ReadView) -> Value {
    view.get_owned("workspace.list", "").unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["workspace_id"] == "workspace-00")
        .unwrap()["compaction"]
        .clone()
}

fn previous_layout_rejects(state: &AppState, previous: &ReadView) {
    let _ownership = state.begin_snapshot_ownership();
    assert!(
        state
            .prepare_incremental_owner_snapshot(
                SnapshotDelta::new(state.read_view.next_version()),
                previous,
                &BTreeSet::from(["run-00".into()]),
            )
            .is_none(),
        "changed committed policy must invalidate the previous layout"
    );
}

#[test]
fn incremental_layout_avoids_workspace_policy_wire_rendering_for_unrelated_rows() {
    let mut measurements = Vec::new();
    for size in [32, 190] {
        let (_tmp, mut state, project) = policy_fixture(size);
        let (_, positive) = policy_observations::measure(|| {
            state.scope_compaction_observation(&CompactionScope::Workspace("workspace-00".into()))
        });
        assert_eq!(positive, 1, "the actual policy renderer must be observed");
        state.install_snapshot_watched_task(
            &project,
            "policy-task",
            Some(json!({
                "kind":"tracker_task", "task_id":"policy-task", "title":"published policy task",
                "anchor":"2026-01-01T00:00:00Z",
            })),
        );
        let (_, task) = policy_observations::measure(|| state.publish_task_snapshot_projection());
        let task_view = state.read_view.load();
        assert!(task_view.get_owned("board.list", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["task_id"] == "policy-task"));
        state.runs.get_mut("run-00").unwrap().run.goal = "changed owner policy control".into();
        let retained = task_view.get_owned("board.list", "").unwrap();
        let (_, owner) =
            policy_observations::measure(|| state.publish_owner_snapshot_projection("run-00"));
        let actual = state.read_view.load();
        assert_eq!(
            actual.get_owned("board.list", "").unwrap()["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["run_id"] == "run-00")
                .unwrap()["goal"],
            "changed owner policy control"
        );
        assert_eq!(task_view.get_owned("board.list", "").unwrap(), retained);
        assert_full_reference(&state, &actual);
        eprintln!("{size} workspace policy renders: task={task} owner={owner}");
        measurements.push((size, task, owner));
    }
    for (size, task, owner) in measurements {
        assert_eq!(
            task, 0,
            "{size}: task-only publication rendered unrelated workspace policies"
        );
        assert!(owner > 0, "the selected workspace policy must still render");
        assert!(
            owner <= 2,
            "{size}: owner publication rendered unrelated workspace policies"
        );
    }
}

#[test]
fn incremental_layout_policy_changes_and_resets_preserve_inherited_wire_and_fallback() {
    let (_tmp, mut state, project) = policy_fixture(4);
    let scopes = [
        CompactionScope::Device,
        CompactionScope::Project(project.clone()),
        CompactionScope::Workspace("workspace-00".into()),
    ];
    for (scope, tokens) in scopes.iter().zip([110_001, 220_002, 330_003]) {
        let old = state.read_view.load();
        let retained = workspace_policy(&old);
        state
            .patch_compaction_thresholds(scope.clone(), &json!({"claude":tokens}))
            .unwrap();
        previous_layout_rejects(&state, &old);
        let actual = state.read_view.load();
        let policy = workspace_policy(&actual);
        assert_eq!(policy["effective"]["claude"]["tokens"], tokens);
        assert_eq!(
            policy["effective"]["claude"]["source"],
            json!(scope.source())
        );
        assert_eq!(workspace_policy(&old), retained);
        assert_full_reference(&state, &actual);
    }
    for (index, tokens, source) in [
        (2, 220_002, CompactionSource::Project(project)),
        (1, 110_001, CompactionSource::Device),
        (0, 200_000, CompactionSource::Default),
    ] {
        let old = state.read_view.load();
        state
            .patch_compaction_thresholds(scopes[index].clone(), &json!({"claude":null}))
            .unwrap();
        previous_layout_rejects(&state, &old);
        let actual = state.read_view.load();
        let policy = workspace_policy(&actual);
        assert_eq!(policy["effective"]["claude"]["tokens"], tokens);
        assert_eq!(policy["effective"]["claude"]["source"], json!(source));
        assert_full_reference(&state, &actual);
    }
}

#[test]
fn incremental_layout_reuses_committed_policy_after_each_failed_config_save() {
    let (tmp, mut state, project) = policy_fixture(4);
    for scope in [
        CompactionScope::Device,
        CompactionScope::Project(project),
        CompactionScope::Workspace("workspace-00".into()),
    ] {
        for failure in [
            crate::app::ConfigPersistStep::Write,
            crate::app::ConfigPersistStep::Rename,
        ] {
            let old = state.read_view.load();
            let retained = workspace_policy(&old);
            let disk = std::fs::read(tmp.path().join("config.json")).unwrap();
            let stored = state.compaction_thresholds(&scope).unwrap().stored_value();
            state.config_persist_failure = Some(failure);
            assert!(state
                .patch_compaction_thresholds(scope.clone(), &json!({"claude":456_789}))
                .is_err());
            state.config_persist_failure = None;
            assert!(Arc::ptr_eq(&old, &state.read_view.load()));
            assert_eq!(
                state.compaction_thresholds(&scope).unwrap().stored_value(),
                stored
            );
            assert_eq!(std::fs::read(tmp.path().join("config.json")).unwrap(), disk);
            fast_owner(&state, "run-01");
            let actual = state.read_view.load();
            assert_eq!(workspace_policy(&actual), retained);
            assert_eq!(workspace_policy(&old), retained);
            assert_full_reference(&state, &actual);
        }
    }
}
