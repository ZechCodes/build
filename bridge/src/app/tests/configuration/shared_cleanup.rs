//! Failed creation owns only checkouts that nobody else has referenced.
use super::*;

#[test]
fn failed_clone_settlement_keeps_directory_referenced_by_another_project() {
    let root = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(root.path(), "upstream");
    let home_a = root.path().join("home-a");
    let home_b = root.path().join("home-b");
    std::fs::create_dir(&home_a).unwrap();
    std::fs::create_dir(&home_b).unwrap();
    let projects = root.path().join("projects");
    let config = root.path().join("config.json");
    std::fs::write(&config, json!({"projects_dir":projects}).to_string()).unwrap();
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().join("state"),
    )
    .unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("workspaces"), "main", true, context)
            .with_config(&config)
            .unwrap();
    let other = app.add_project(home_b, "main".into());

    let params = json!({
        "name":"attempt",
        "sources":[{"path":home_a},{"remote":upstream,"name":"fresh"}]
    });
    let (admitted, work) = app.dispatch_deferring("project.create", &params);
    assert!(admitted.is_ok(), "{admitted:?}");
    let done = work
        .expect("opening and cloning run off the app lock")
        .run();
    let cloned = projects.join("attempt-sources/fresh");
    assert!(cloned.join(".git").is_dir());
    let original = std::fs::read(cloned.join("README.md")).unwrap();

    // The clone exists, but the attempt has not published a project yet.
    // A different project can acquire an independent directory reference.
    let linked = app.handle(req(
        "project.add_source",
        json!({"project_id":other,"path":cloned,"name":"borrowed"}),
    ));
    assert_eq!(linked["ok"], true, "{linked}");
    let source_id = linked["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let committed = std::fs::read(&config).unwrap();

    app.config_persist_failure = Some(ConfigPersistStep::Write);
    let failed = app.apply_deferred("project.create", &params, done);
    assert!(
        failed.is_err(),
        "the original attempt must fail its durable write"
    );

    assert!(
        cloned.join(".git").is_dir(),
        "failed creation deleted a now-shared checkout"
    );
    assert_eq!(std::fs::read(cloned.join("README.md")).unwrap(), original);
    assert!(app
        .sources_for(&other)
        .unwrap()
        .iter()
        .any(|source| { source.id == source_id && source.path == cloned }));
    assert_eq!(std::fs::read(&config).unwrap(), committed);
    app.config_persist_failure = None;
    let listed = app.project_list();
    let other = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == other)
        .unwrap();
    assert!(other["sources"].as_array().unwrap().iter().any(|source| {
        source["id"] == source_id && source["path"] == cloned.to_string_lossy().as_ref()
    }));
}
