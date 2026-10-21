use super::*;

fn configured_project(root: &Path, repository: &Path) -> AppState {
    let mut state = qa_state(repository, root)
        .with_config(root.join("config.json"))
        .unwrap();
    let config = state.config_value(&state.projects_dir, state.default_harness, state.isolation);
    state.persist_config(&config).unwrap();
    state.initialize_snapshot_projection();
    state
}

fn assert_committed_after_later_success(state: &mut AppState, before: &Value) {
    state.config_persist_failure = None;
    state
        .settings_set(&json!({"projects_dir": state.projects_dir}))
        .unwrap();
    // Repository availability reflects the actual Git initialization even
    // when its config commit fails; compare the durable project fields.
    let durable_rows = |mut rows: Value| {
        for project in rows["projects"].as_array_mut().unwrap() {
            project
                .as_object_mut()
                .unwrap()
                .remove("isolation_available");
        }
        rows
    };
    let expected = durable_rows(before.clone());
    assert_eq!(durable_rows(state.project_list()), expected);
    assert_eq!(
        durable_rows(
            state
                .read_view
                .load()
                .get("project.list", "")
                .unwrap()
                .into_owned()
        ),
        expected
    );
    let config: Value =
        serde_json::from_slice(&std::fs::read(state.config_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        config["projects"][0]["sources"].as_array().unwrap().len(),
        before["projects"][0]["sources"].as_array().unwrap().len()
    );
}

#[test]
fn failed_project_configuration_edits_never_reappear_after_later_success() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        for (method, change) in [
            ("project.remove_source", json!({"source_id": "source-2"})),
            (
                "project.update_source",
                json!({"source_id": "source-2", "name": "Unsaved label"}),
            ),
            (
                "project.update_source",
                json!({"source_id": "source-2", "sync_base": false}),
            ),
            ("project.set_isolation", json!({"isolation": "worktree"})),
        ] {
            let (directory, repository) = init_repo();
            let second = init_repo_named(directory.path(), "second");
            let mut state = configured_project(directory.path(), &repository);
            let id = state.project_at(0).id.clone();
            state
                .append_project_sources(
                    &id,
                    vec![crate::app::projects::ProjectSource::added(
                        "source-2".into(),
                        "Second".into(),
                        "second".into(),
                        second,
                        true,
                        "main".into(),
                    )],
                    vec![],
                )
                .unwrap();
            let before = state.project_list();
            let mut params = change;
            params["project_id"] = json!(id);
            state.config_persist_failure = Some(failure);
            let response = state.handle(req(method, params));
            assert_eq!(
                response["ok"], false,
                "{method} acknowledged failed save: {response}"
            );
            assert_committed_after_later_success(&mut state, &before);
        }
    }
}

#[test]
fn failed_append_restores_source_ids_and_removes_only_owned_checkouts() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let (directory, repository) = init_repo();
        let mut state = configured_project(directory.path(), &repository);
        let id = state.project_at(0).id.clone();
        let before = state.project_list();
        let next = state.project_at(0).next_source;
        let owned = directory.path().join("owned");
        let user = directory.path().join("user");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let added = [owned.clone(), user.clone()]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                crate::app::projects::ProjectSource::added(
                    format!("source-{}", index + 2),
                    format!("Extra {index}"),
                    format!("extra-{index}"),
                    path,
                    false,
                    "main".into(),
                )
            })
            .collect();
        state.config_persist_failure = Some(failure);
        assert!(state
            .append_project_sources(&id, added, vec![owned.clone()])
            .is_err());
        assert!(!owned.exists());
        assert!(user.exists());
        assert_eq!(state.project_at(0).next_source, next);
        assert_committed_after_later_success(&mut state, &before);
    }
}

#[test]
fn failed_git_initialization_keeps_actual_git_but_not_unsaved_project_fields() {
    crate::git_fixture::environment::isolated_git_test!();
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("plain");
        std::fs::create_dir(&repository).unwrap();
        let mut state = configured_project(directory.path(), &repository);
        let id = state.project_at(0).id.clone();
        let before = state.project_list();
        state.config_persist_failure = Some(failure);
        let response = state.handle(req("project.init_git", json!({"project_id": id})));
        assert_eq!(response["ok"], false, "{response}");
        assert!(repository.join(".git").exists());
        assert!(!state.project_at(0).is_git);
        assert!(!state.project_at(0).sources[0].is_git);
        assert_committed_after_later_success(&mut state, &before);
    }
}

#[test]
fn deleting_project_removes_its_cached_watched_tasks_from_snapshot() {
    let (directory, repository) = init_repo();
    let mut state = configured_project(directory.path(), &repository);
    let id = state.project_at(0).id.clone();
    state.install_snapshot_watched_task(
        &id,
        "task-observed",
        Some(json!({"task_id": "task-observed", "project_id": id})),
    );
    assert_eq!(state.snapshot_watched_task_rows().len(), 1);
    let response = state.handle(req(
        "project.delete",
        json!({"project_id": id, "confirm": true}),
    ));
    assert_eq!(response["ok"], true, "{response}");
    assert!(state.snapshot_watched_task_rows().is_empty());
}
