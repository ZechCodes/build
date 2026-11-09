use super::*;
use std::path::Path;

const REMOTE: &str = "https://example.invalid/environment.git";

#[test]
fn environment_source_remote_uses_newest_observation() {
    let root = tempfile::tempdir().unwrap();
    let home = repository(root.path());
    let mut app = state(root.path());
    let project = app.add_project(home.clone(), "main".into());
    let sibling = app.add_environment_sibling_for_test(&project);
    let source = app.project_for(&project).unwrap().sources[0].clone();
    let environment = app.project_for(&project).unwrap().environment_id.clone();
    {
        let mut observations = app.snapshot_observations.projects.lock().unwrap();
        observations.insert(
            project.clone(),
            source_observation(&source, 1, json!("old-project")),
        );
        observations.insert(
            sibling.clone(),
            source_observation(&source, 2, json!("older-sibling")),
        );
    }
    app.snapshot_observations
        .environments
        .lock()
        .unwrap()
        .insert(
            environment.clone(),
            source_observation(&source, 3, json!("new-environment")),
        );
    assert_eq!(environment_remote(&app), "new-environment");

    app.snapshot_observations.projects.lock().unwrap().insert(
        sibling,
        source_observation(&source, 4, json!("newest-project")),
    );
    assert_eq!(environment_remote(&app), "newest-project");

    let mut wrong_id = source.clone();
    wrong_id.id = "different-source".into();
    let mut wrong_path = source.clone();
    wrong_path.path = home.join("different-location");
    app.snapshot_observations.projects.lock().unwrap().insert(
        project,
        ProjectObservation {
            source_versions: Default::default(),
            version: 5,
            value: json!({"sources":[
                wrong_id.wire_observed(json!("wrong-id")),
                wrong_path.wire_observed(json!("wrong-path")),
            ]}),
        },
    );
    assert_eq!(environment_remote(&app), "newest-project");

    app.snapshot_observations
        .environments
        .lock()
        .unwrap()
        .insert(environment, source_observation(&source, 6, Value::Null));
    assert_eq!(environment_remote(&app), Value::Null);
}

fn source_observation(source: &ProjectSource, version: u64, remote: Value) -> ProjectObservation {
    ProjectObservation {
        source_versions: Default::default(),
        version,
        value: json!({"sources":[source.wire_observed(remote)]}),
    }
}

fn environment_remote(app: &AppState) -> Value {
    app.environment_list()["environments"][0]["sources"][0]["remote"].clone()
}

fn state(root: &Path) -> AppState {
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state"))
            .unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

fn repository(root: &Path) -> PathBuf {
    let home = crate::git_fixture::init_repo_named(root, "home");
    git2::Repository::open(&home)
        .unwrap()
        .remote("origin", REMOTE)
        .unwrap();
    home
}

#[test]
fn empty_environment_metadata_survives_sole_project_removal() {
    let root = tempfile::tempdir().unwrap();
    let home = repository(root.path());
    let mut app = state(root.path());
    let project = app.add_project(home.clone(), "main".into());
    app.initialize_snapshot_projection();
    assert_eq!(
        app.environment_list()["environments"][0]["sources"][0]["remote"],
        REMOTE
    );
    let latest_remote = "https://example.invalid/latest-environment.git";
    git2::Repository::open(&home)
        .unwrap()
        .remote_set_url("origin", latest_remote)
        .unwrap();
    app.project_json(app.projects.get(&project).unwrap());
    app.dispatch(
        "project.delete",
        &json!({"project_id":project,"confirm":true}),
    )
    .unwrap();
    app.publish_all_snapshot_projection();
    let row = &app.environment_list()["environments"][0];
    assert_eq!(row["project_ids"], json!([]));
    assert_eq!(row["sources"][0]["remote"], latest_remote);
    assert_eq!(
        app.read_view.load().get("environment.list", "").unwrap()["environments"][0],
        *row
    );
}

#[test]
fn cold_empty_and_parked_environments_warm_their_own_repository_metadata() {
    let root = tempfile::tempdir().unwrap();
    let home = repository(root.path());
    for parked in [false, true] {
        let config = root.path().join("config.json");
        let projects = if parked {
            json!([{
                "id":"proj-1", "name":"Away", "path":root.path().join("away"),
                "home_path":root.path().join("away"), "environment_id":"env-1", "project_key":"legacy-away", "sources":[]
            }])
        } else {
            json!([])
        };
        std::fs::write(&config, serde_json::to_vec(&json!({
            "environments":[{"id":"env-1","name":"Home","isolation":"rift","sources":[{
                "id":"source-1","name":"Home","mount":"home","path":home,"is_git":true,"base_branch":"main"
            }]}], "projects":projects
        })).unwrap()).unwrap();
        let mut app = state(root.path()).with_config(&config).unwrap();
        app.initialize_snapshot_projection();
        assert!(app.projects.iter().next().is_none());
        let row = &app.environment_list()["environments"][0];
        assert_eq!(row["sources"][0]["remote"], REMOTE, "parked={parked}");
        let available = crate::isolation::IsolationAvailability::of(&home, &app.worktrees_root);
        assert_eq!(
            row["isolation_effective"],
            if available.rift.is_ok() {
                "rift"
            } else {
                "worktree"
            }
        );
    }
}

#[test]
fn delayed_environment_probe_cannot_overwrite_newer_empty_environment_metadata() {
    let root = tempfile::tempdir().unwrap();
    let home = repository(root.path());
    let mut app = state(root.path());
    let project = app.add_project(home.clone(), "main".into());
    app.initialize_snapshot_projection();
    app.dispatch(
        "project.delete",
        &json!({"project_id":project,"confirm":true}),
    )
    .unwrap();
    let delayed = app.capture_project_snapshot_refresh().render();
    let updated_remote = "https://example.invalid/newer-environment.git";
    git2::Repository::open(&home)
        .unwrap()
        .remote_set_url("origin", updated_remote)
        .unwrap();
    let current = app.capture_project_snapshot_refresh().render();
    app.install_project_snapshot_refresh(current);
    app.install_project_snapshot_refresh(delayed);
    assert_eq!(
        app.environment_list()["environments"][0]["sources"][0]["remote"],
        updated_remote
    );
}

#[test]
fn environment_availability_uses_its_current_home_and_newest_matching_probe() {
    let root = tempfile::tempdir().unwrap();
    let original_home = repository(root.path());
    let environment_home = root.path().join("secondary");
    std::fs::create_dir(&environment_home).unwrap();
    let config = root.path().join("config.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"projects":[{
            "id":"proj-1", "path":original_home, "sources":[
                {"id":"source-1","path":original_home,"name":"home","mount":"home"},
                {"id":"source-2","path":environment_home,"name":"secondary","mount":"secondary"}
            ]
        }]}))
        .unwrap(),
    )
    .unwrap();
    let mut app = state(root.path()).with_config(&config).unwrap();
    let project = "proj-1".to_owned();
    let secondary = app.sources_for(&project).unwrap().pop().unwrap();
    assert!(app.projects.set_sources(&project, vec![secondary]));
    assert_eq!(app.project_for(&project).unwrap().home_path, original_home);
    let environment_id = app.project_for(&project).unwrap().environment_id.clone();
    // This preference belongs to the environment and must survive its last project.
    let mut configured = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    configured["environments"][0]["isolation"] = json!("rift");
    app.projects.restore_environments(&configured);
    assert_eq!(app.project_for(&project).unwrap().isolation, None);
    app.snapshot_observations.projects.lock().unwrap().insert(
        project.clone(),
        ProjectObservation {
            source_versions: Default::default(),
            version: 2,
            value: json!({"path":original_home,"sources":[],"isolation_available":{"rift":true}}),
        },
    );
    app.snapshot_observations.environments.lock().unwrap().insert(environment_id.clone(), ProjectObservation {
        source_versions: Default::default(),
        version:3, value:json!({"path":environment_home,"sources":[],"isolation_available":{"rift":false}}),
    });
    let row = &app.environment_list()["environments"][0];
    assert_eq!(row["home_path"], json!(environment_home));
    assert_eq!(row["isolation_effective"], "worktree");
    // An older matching live-project probe cannot displace the newer environment probe.
    app.snapshot_observations
        .projects
        .lock()
        .unwrap()
        .get_mut(&project)
        .unwrap()
        .value["path"] = json!(environment_home);
    assert_eq!(
        app.environment_list()["environments"][0]["isolation_effective"],
        "worktree"
    );
    // The last project's newer A probe supplies remotes, not B availability.
    app.snapshot_observations
        .environments
        .lock()
        .unwrap()
        .get_mut(&environment_id)
        .unwrap()
        .value["isolation_available"]["rift"] = json!(true);
    let latest_remote = "https://example.invalid/latest-secondary.git";
    let source = app.project_for(&project).unwrap().sources[0].wire_observed(json!(latest_remote));
    app.snapshot_observations.projects.lock().unwrap().insert(project.clone(), ProjectObservation {
        source_versions: Default::default(),
        version:4, value:json!({"path":original_home,"sources":[source],"isolation_available":{"rift":false}}),
    });
    app.dispatch(
        "project.delete",
        &json!({"project_id":project,"confirm":true}),
    )
    .unwrap();
    let row = &app.environment_list()["environments"][0];
    assert_eq!(row["project_ids"], json!([]));
    assert_eq!(row["sources"][0]["remote"], latest_remote);
    assert_eq!(row["isolation_effective"], "rift");
}
