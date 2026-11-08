use super::runtime_tests::{rpc, state};
use crate::app::AppState;
use serde_json::{json, Value};

fn fixture(root: &std::path::Path) -> AppState {
    for name in ["home", "extra", "shared", "target"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let config = json!({"projects_dir":root.join("managed"), "environments":[{
        "id":"env-1","name":"Original","sources":[
            {"id":"source-1","name":"home","mount":"home","path":root.join("home")},
            {"id":"source-2","name":"shared","mount":"shared","path":root.join("shared")}
        ],"next_source":4
    }],"projects":[
        {"id":"proj-1","environment_id":"env-1","project_key":"project:first","path":root.join("home"),"name":"First","added_sources":[]},
        {"id":"proj-2","environment_id":"env-1","project_key":"project:second","path":root.join("home"),"name":"Second","added_sources":[
            {"id":"source-3","name":"extra","mount":"extra","path":root.join("extra")}
        ]}
    ]});
    let path = root.join("config.json");
    std::fs::write(&path, config.to_string()).unwrap();
    state(root).with_config(path).unwrap()
}
fn ok(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = rpc(app, method, params);
    assert_eq!(reply["ok"], true, "{method}: {reply}");
    reply["result"].clone()
}

#[test]
fn environment_rename_keeps_ids_and_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let before = app.environment_list()["environments"][0].clone();
    let row = ok(
        &mut app,
        "environment.rename",
        json!({"environment_id":"env-1","name":"Display / name"}),
    );
    assert_eq!(row["name"], "Display / name");
    for key in ["environment_id", "home_path", "sources", "project_ids"] {
        assert_eq!(row[key], before[key]);
    }
    let before = app.project_json(app.projects.get("proj-1").unwrap());
    let row = ok(
        &mut app,
        "project.rename",
        json!({"project_id":"proj-1","name":"New project"}),
    );
    for key in [
        "project_id",
        "project_key",
        "path",
        "environment_id",
        "sources",
    ] {
        assert_eq!(row[key], before[key]);
    }
    assert_eq!(row["name"], "New project");
    let app = state(root.path())
        .with_config(root.path().join("config.json"))
        .unwrap();
    assert_eq!(
        app.projects.environment("env-1").unwrap().name,
        "Display / name"
    );
    assert_eq!(app.projects.get("proj-1").unwrap().name, "New project");
}

#[test]
fn rename_is_one_bounded_line() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    for (method, key, id) in [
        ("environment.rename", "environment_id", "env-1"),
        ("project.rename", "project_id", "proj-1"),
    ] {
        for name in [
            "".to_string(),
            "   ".into(),
            "two\nlines".into(),
            "trailing\n".into(),
            "tab\tname".into(),
            "unicode\u{2028}line".into(),
            "x".repeat(201),
        ] {
            let reply = rpc(&mut app, method, json!({key:id,"name":name}));
            assert_eq!(reply["ok"], false, "{method}: {reply}");
        }
        assert_eq!(
            ok(&mut app, method, json!({key:id,"name":" x "}))["name"],
            "x"
        );
    }
}

#[test]
fn isolation_resolution_order() {
    use crate::isolation::Isolation;
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.isolation = Isolation::Rift;
    ok(
        &mut app,
        "environment.set_isolation",
        json!({"environment_id":"env-1","isolation":"worktree"}),
    );
    assert_eq!(
        app.project_isolation(app.projects.get("proj-1").unwrap()),
        Isolation::Worktree
    );
    ok(
        &mut app,
        "project.set_isolation",
        json!({"project_id":"proj-1","isolation":"worktree"}),
    );
    ok(
        &mut app,
        "environment.set_isolation",
        json!({"environment_id":"env-1","isolation":null}),
    );
    assert_eq!(
        app.project_isolation(app.projects.get("proj-1").unwrap()),
        Isolation::Worktree
    );
    assert_eq!(
        app.project_isolation(app.projects.get("proj-2").unwrap()),
        Isolation::Rift
    );
    ok(
        &mut app,
        "project.set_isolation",
        json!({"project_id":"proj-1","isolation":null}),
    );
    assert_eq!(
        app.project_isolation(app.projects.get("proj-1").unwrap()),
        Isolation::Rift
    );
}

#[test]
fn environment_source_verbs_add_remove_update_sync() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let target = root.path().join("target");
    crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":target}),
    );
    let source = row["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["path"] == json!(target))
        .unwrap();
    let id = source["id"].as_str().unwrap().to_string();
    for p in ["proj-1", "proj-2"] {
        assert!(app
            .projects
            .get(p)
            .unwrap()
            .sources
            .iter()
            .any(|s| s.id == id));
    }
    let row = ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":id,"name":"Renamed","path":root.path().join("git")}),
    );
    assert_eq!(row["checkouts_updated"], 0);
    assert!(row["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == id && s["name"] == "Renamed"));
    ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":id,"sync_base":false}),
    );
    ok(
        &mut app,
        "environment.sync_source",
        json!({"environment_id":"env-1","source_id":id}),
    );
    ok(
        &mut app,
        "environment.remove_source",
        json!({"environment_id":"env-1","source_id":id}),
    );
    assert!(root.path().join("git").is_dir());
    for p in ["proj-1", "proj-2"] {
        assert!(!app
            .projects
            .get(p)
            .unwrap()
            .sources
            .iter()
            .any(|s| s.id == id));
    }
    assert_eq!(app.projects.get("proj-2").unwrap().added_sources.len(), 1);
    let app = state(root.path())
        .with_config(root.path().join("config.json"))
        .unwrap();
    assert!(!app
        .projects
        .environment("env-1")
        .unwrap()
        .sources
        .iter()
        .any(|s| s.id == id));
}

#[test]
fn environment_sources_refuse_containment_and_dependent_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    std::fs::create_dir(root.path().join("home/nested")).unwrap();
    for method in ["environment.add_source", "environment.update_source"] {
        for path in [
            root.path().join("extra"),
            root.path().join("home/nested"),
            root.path().join("home"),
            root.path().to_path_buf(),
        ] {
            let before = std::fs::read(root.path().join("config.json")).unwrap();
            let mut params = json!({"environment_id":"env-1","path":path});
            if method == "environment.update_source" {
                params["source_id"] = json!("source-2");
            }
            let reply = rpc(&mut app, method, params);
            assert_eq!(reply["ok"], false, "{method}: {reply}");
            assert_eq!(
                std::fs::read(root.path().join("config.json")).unwrap(),
                before
            );
        }
    }
}

#[test]
fn environment_edits_work_without_projects_and_rollback_on_save_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.projects.clear();
    ok(
        &mut app,
        "environment.rename",
        json!({"environment_id":"env-1","name":"Orphan"}),
    );
    ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":root.path().join("target")}),
    );
    ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":"source-2","name":"Edited"}),
    );
    ok(
        &mut app,
        "environment.set_isolation",
        json!({"environment_id":"env-1","isolation":"worktree"}),
    );
    let before = app.environment_list();
    let bytes = std::fs::read(root.path().join("config.json")).unwrap();
    app.config_persist_failure = Some(crate::app::ConfigPersistStep::Write);
    let reply = rpc(
        &mut app,
        "environment.rename",
        json!({"environment_id":"env-1","name":"Cannot save"}),
    );
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(app.environment_list(), before);
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        bytes
    );
}

#[test]
fn environment_source_sync_runs_for_an_orphan_reference() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let git = crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":git}),
    );
    let id = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    app.projects.clear();
    assert_eq!(
        ok(
            &mut app,
            "environment.sync_source",
            json!({"environment_id":"env-1","source_id":id})
        ),
        json!({"pending":true})
    );
    let state = std::sync::Arc::new(std::sync::Mutex::new(app));
    crate::app::AppState::sync_sources(
        &state,
        crate::app::projects::base_sync::SyncPass::Requested,
        1000,
    );
    let app = state.lock().unwrap();
    let sources = app.environment_list()["environments"][0]["sources"].clone();
    assert_eq!(
        sources.as_array().unwrap().last().unwrap()["sync"]["state"],
        "no_remote"
    );
}

#[test]
fn environment_reference_edits_leave_shared_siblings_and_workspace_cuts_alone() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let sibling = root.path().join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let opened = ok(
        &mut app,
        "project.create",
        json!({"name":"Other","sources":[{"path":sibling},{"path":root.path().join("shared")}]}),
    );
    let sibling_id = opened["project_id"].as_str().unwrap().to_owned();
    let sibling_environment = app
        .projects
        .get(&sibling_id)
        .unwrap()
        .environment_id
        .clone();
    ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":sibling_environment,"path":root.path().join("target")}),
    );
    let cut = ok(
        &mut app,
        "workspace.create",
        json!({"project_id":"proj-1","name":"Old cut"}),
    );
    let workspace = cut["workspace_id"].as_str().unwrap();
    let directories = app.workspaces.get(workspace).unwrap().directories.clone();
    ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":"source-2","path":root.path().join("target"),"name":"Repointed"}),
    );
    assert_eq!(
        app.projects.get(&sibling_id).unwrap().sources[1].path,
        root.path().join("shared")
    );
    assert_eq!(
        app.projects.get("proj-1").unwrap().sources[1].path,
        root.path().join("target")
    );
    for (before, after) in directories
        .iter()
        .zip(&app.workspaces.get(workspace).unwrap().directories)
    {
        assert_eq!(before.path, after.path);
        assert_eq!(before.source_path, after.source_path);
    }
}

#[test]
fn environment_remote_edits_publish_for_orphan_and_shared_references() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let git = crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":git}),
    );
    let id = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    let path = root.path().join("config.json");
    let mut config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    config["environments"].as_array_mut().unwrap().push(json!({"id":"env-2","name":"Other","sources":[
        {"id":"source-1","name":"git","mount":"git","path":git,"is_git":true,"base_branch":"main"}
    ]}));
    config["projects"] = json!([]);
    std::fs::write(&path, config.to_string()).unwrap();
    app = state(root.path()).with_config(path).unwrap();
    let row = ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":id,"remote":"https://example.invalid/new.git"}),
    );
    assert_eq!(
        row["sources"].as_array().unwrap().last().unwrap()["remote"],
        "https://example.invalid/new.git"
    );
    for environment in app.environment_list()["environments"].as_array().unwrap() {
        assert_eq!(
            environment["sources"].as_array().unwrap().last().unwrap()["remote"],
            "https://example.invalid/new.git"
        );
    }
}

#[test]
fn environment_source_edits_refuse_parked_project_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    drop(app);
    let path = root.path().join("config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["projects"][1]["path"] = json!(root.path().join("away"));
    config["projects"][1]["home_path"] = json!(root.path().join("away"));
    std::fs::write(&path, config.to_string()).unwrap();
    let mut app = state(root.path()).with_config(&path).unwrap();
    assert_eq!(app.projects.parked().len(), 1);
    for method in ["environment.add_source", "environment.update_source"] {
        let mut params = json!({"environment_id":"env-1","path":root.path().join("extra")});
        if method == "environment.update_source" {
            params["source_id"] = json!("source-2");
        }
        let reply = rpc(&mut app, method, params);
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("already included in this project"),
            "{reply}"
        );
    }
}

#[test]
fn environment_edits_roll_back_names_and_reference_projections() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.config_persist_failure = Some(crate::app::ConfigPersistStep::Write);
    let before = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    for (method, params) in [
        (
            "project.rename",
            json!({"project_id":"proj-1","name":"Unsaved"}),
        ),
        (
            "environment.set_isolation",
            json!({"environment_id":"env-1","isolation":"worktree"}),
        ),
        (
            "environment.add_source",
            json!({"environment_id":"env-1","path":root.path().join("target")}),
        ),
        (
            "environment.remove_source",
            json!({"environment_id":"env-1","source_id":"source-2"}),
        ),
        (
            "environment.update_source",
            json!({"environment_id":"env-1","source_id":"source-2","name":"Unsaved"}),
        ),
    ] {
        let reply = rpc(&mut app, method, params);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(
            app.config_value(&app.projects_dir, app.default_harness, app.isolation),
            before,
            "{method}"
        );
    }
    assert!(root.path().join("target").is_dir());
}

#[test]
fn environment_sync_status_survives_project_configuration() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let git = crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":git}),
    );
    let id = row["sources"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        &mut app,
        "environment.sync_source",
        json!({"environment_id":"env-1","source_id":id}),
    );
    let state = std::sync::Arc::new(std::sync::Mutex::new(app));
    AppState::sync_sources(
        &state,
        crate::app::projects::base_sync::SyncPass::Requested,
        1000,
    );
    let mut app = state.lock().unwrap();
    assert!(app
        .projects
        .environment("env-1")
        .unwrap()
        .sources
        .last()
        .unwrap()
        .sync_status
        .is_some());
    app.projects.source_mut("proj-1", &id).unwrap().sync_status = None;
    ok(
        &mut app,
        "project.rename",
        json!({"project_id":"proj-1","name":"Changed"}),
    );
    assert!(app
        .projects
        .environment("env-1")
        .unwrap()
        .sources
        .last()
        .unwrap()
        .sync_status
        .is_some());
    assert!(app
        .projects
        .get("proj-1")
        .unwrap()
        .sources
        .last()
        .unwrap()
        .sync_status
        .is_none());
}
