use crate::app::AppState;
use serde_json::json;

#[test]
fn sources_for_is_environment_then_added() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let extra = root.path().join("extra");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&extra).unwrap();
    let config = root.path().join("config.json");
    std::fs::write(&config, json!({
        "environments": [{"id":"env-1","name":"home","next_source":9,"sources":[
            {"id":"source-1","path":home,"name":"home","mount":"home"}
        ]}],
        "projects": [{"id":"proj-1","environment_id":"env-1","project_key":"opaque-key", "home_path":home,
            "path":home,"added_sources":[{"id":"source-7","path":extra,"name":"extra","mount":"extra"}]}]
    }).to_string()).unwrap();
    let app = AppState::new_unrooted(root.path().join("worktrees"), "main", true, "/tmp/test-mcp.sock").with_config(config).unwrap();
    let sources = app.sources_for("proj-1").unwrap();
    assert_eq!(sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["source-1", "source-7"]);
    assert_eq!(app.project_for("proj-1").unwrap().project_key, "opaque-key");
}

#[test]
fn next_source_continuity() {
    let root = tempfile::tempdir().unwrap();
    let mut app = AppState::new_unrooted(root.path().join("worktrees"), "main", true, "/tmp/test-mcp.sock");
    let id = app.add_project(root.path().to_path_buf(), "main".into());
    app.projects.reserve_source_ids_through(&id, 15);
    let project = app.project_for(&id).unwrap();
    assert_eq!(app.projects.environment(&project.environment_id).unwrap().next_source, 15);
    let config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    assert_eq!(config["environments"][0]["next_source"], 15);
    assert_eq!(config["projects"][0]["next_source"], 15);
}

#[test]
fn clearing_project_override_restores_device_default() {
    let root = tempfile::tempdir().unwrap();
    let mut app = AppState::new_unrooted(root.path().join("worktrees"), "main", true, "/tmp/test-mcp.sock");
    let id = app.add_project(root.path().to_path_buf(), "main".into());
    app.projects.set_isolation(&id, Some(crate::isolation::Isolation::Worktree));
    let environment_id = app.project_for(&id).unwrap().environment_id.clone();
    assert_eq!(app.projects.environment(&environment_id).unwrap().isolation, Some(crate::isolation::Isolation::Worktree));
    app.projects.set_isolation(&id, None);
    assert_eq!(app.projects.environment(&environment_id).unwrap().isolation, None);
}
