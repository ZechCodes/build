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
        "environments": [{"id":"env-1","name":"home","next_source":2,"sources":[
            {"id":"source-1","path":home,"name":"home","mount":"home"}
        ]}],
        "projects": [{"id":"proj-1","environment_id":"env-1","project_key":"opaque-key", "home_path":home,
            "path":home,"added_sources":[{"id":"source-7","path":extra,"name":"extra","mount":"extra"}]}]
    }).to_string()).unwrap();
    let app = state(root.path()).with_config(config).unwrap();
    let sources = app.sources_for("proj-1").unwrap();
    assert_eq!(
        sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["source-1", "source-7"]
    );
    assert_eq!(app.project_for("proj-1").unwrap().project_key, "opaque-key");
    assert_eq!(app.project_for("proj-1").unwrap().next_source, 8);
    assert_eq!(app.projects.environment("env-1").unwrap().next_source, 8);
}

#[test]
fn next_source_continuity() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let extra = root.path().join("extra");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&extra).unwrap();
    let path = root.path().join("config.json");
    std::fs::write(
        &path,
        json!({"projects":[{"id":"proj-1","path":home,"next_source":19,
        "sources":[{"id":"source-1","path":home,"name":"home","mount":"home"}]}]})
        .to_string(),
    )
    .unwrap();
    let app = state(root.path()).with_config(&path).unwrap();
    let project = app.project_for("proj-1").unwrap();
    assert_eq!(
        app.projects
            .environment(&project.environment_id)
            .unwrap()
            .next_source,
        19
    );
    assert_eq!(project.next_source, 19);
    app.persist();
    let mut app = state(root.path()).with_config(&path).unwrap();
    let result = app.handle(crate::app::Frame {
        session_id: "test".into(), message_id: "test".into(), frame_type: "data".into(),
        sender: "client".into(), created_at: "test".into(),
        payload: json!({"method":"project.add_source","id":"1","params":{"project_id":"proj-1","path":extra,"name":"extra"}}),
    });
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["result"]["sources"][1]["id"], "source-19");
    let reloaded = state(root.path()).with_config(&path).unwrap();
    let project = reloaded.project_for("proj-1").unwrap();
    assert_eq!(project.next_source, 20);
    assert_eq!(
        reloaded
            .projects
            .environment(&project.environment_id)
            .unwrap()
            .next_source,
        20
    );
}

#[test]
fn clearing_project_override_restores_device_default() {
    use crate::isolation::Isolation;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let path = root.path().join("config.json");
    std::fs::write(
        &path,
        json!({"isolation":"worktree","projects":[{
        "id":"proj-1","path":home,"isolation":"rift","sources":[{
            "id":"source-1","name":"home","mount":"home","path":home}]}]})
        .to_string(),
    )
    .unwrap();
    let mut app = state(root.path()).with_config(&path).unwrap();
    let environment_id = app.project_for("proj-1").unwrap().environment_id.clone();
    assert_eq!(
        app.projects.environment(&environment_id).unwrap().isolation,
        Some(Isolation::Rift)
    );
    let set = rpc(
        &mut app,
        "project.set_isolation",
        json!({"project_id":"proj-1","isolation":"worktree"}),
    );
    assert_eq!(set["ok"], true, "{set}");
    let cleared = rpc(
        &mut app,
        "project.set_isolation",
        json!({"project_id":"proj-1","isolation":null}),
    );
    assert_eq!(cleared["ok"], true, "{cleared}");
    let reloaded = state(root.path()).with_config(&path).unwrap();
    assert_eq!(reloaded.project_for("proj-1").unwrap().isolation, None);
    assert_eq!(
        reloaded
            .projects
            .environment(&environment_id)
            .unwrap()
            .isolation,
        None
    );
    assert_eq!(
        reloaded.resolved_isolation("proj-1").isolation,
        Isolation::Worktree
    );
}

fn rpc(app: &mut AppState, method: &str, params: serde_json::Value) -> serde_json::Value {
    app.handle(crate::app::Frame {
        session_id: "test".into(),
        message_id: "test".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "test".into(),
        payload: json!({"method":method,"id":"1","params":params}),
    })
}

fn state(root: &std::path::Path) -> AppState {
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state"))
            .unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

#[test]
fn source_configuration_failure_restores_environment_and_projection() {
    let root = tempfile::tempdir().unwrap();
    let mut app = state(root.path());
    let id = app.add_project(root.path().to_path_buf(), "main".into());
    app.config_path = Some(root.path().join("config.json"));
    app.config_persist_failure = Some(crate::app::ConfigPersistStep::Write);
    let before = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    let result = app.commit_project_configuration(&id, |projects| {
        projects.source_mut(&id, "source-1").unwrap().sync_base = Some(false);
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        app.config_value(&app.projects_dir, app.default_harness, app.isolation),
        before
    );
}

#[test]
fn configured_environment_replaces_seeded_registry_projection() {
    let root = tempfile::tempdir().unwrap();
    let mut app = state(root.path());
    let id = app.add_project(root.path().to_path_buf(), "main".into());
    let config = json!({
        "environments":[{"id":"env-1","name":"restored", "next_source":12,
            "sources":[{"id":"source-8","name":"restored","mount":"restored","path":root.path(),"base_branch":"saved"}]}],
        "projects":[{"id":id,"environment_id":"env-1","project_key":"persisted-key","home_path":root.path(),"path":root.path(),"name":"Saved name","added_sources":[]}]
    });
    app.projects.restore_environments(&config);
    app.restore_configured_projects(&config);
    assert_eq!(app.projects.environment("env-1").unwrap().name, "restored");
    assert_eq!(app.project_for(&id).unwrap().project_key, "persisted-key");
    assert_eq!(app.project_for(&id).unwrap().sources[0].id, "source-8");
    assert_eq!(app.projects.environments().count(), 1);
}

#[test]
fn parked_source_reclassifies_git_metadata_when_folder_returns() {
    let root = tempfile::tempdir().unwrap();
    let away = root.path().join("away");
    let mut app = state(root.path());
    let config = json!({
        "environments":[{"id":"env-1","name":"away", "next_source":2,
            "sources":[{"id":"source-1","name":"away","mount":"away","path":away,"is_git":true}]}],
        "projects":[{"id":"proj-1","environment_id":"env-1","project_key":away,"home_path":away,"path":away,"added_sources":[]}]
    });
    app.projects.restore_environments(&config);
    app.restore_configured_projects(&config);
    assert_eq!(app.projects.parked().len(), 1);
    std::fs::create_dir(&away).unwrap();
    assert_eq!(app.revive_parked_projects(), ["proj-1"]);
    assert!(!app.project_for("proj-1").unwrap().sources[0].is_git);
    assert!(!app.projects.environment("env-1").unwrap().sources[0].is_git);
}

#[test]
fn source_configuration_does_not_invalidate_unrelated_environment() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let other = root.path().join("other");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&other).unwrap();
    let mut app = state(root.path());
    let id = app.add_project(home, "main".into());
    let unrelated = app.add_project(other, "main".into());
    let before = app.projects.observation_project_changes().snapshot();
    app.commit_project_configuration(&id, |projects| {
        projects.source_mut(&id, "source-1").unwrap().sync_base = Some(false);
        Ok(())
    })
    .unwrap();
    let after = app.projects.observation_project_changes().snapshot();
    assert_eq!(
        before.iter().find(|(id, _)| id == &unrelated),
        after.iter().find(|(id, _)| id == &unrelated)
    );
}

#[test]
fn seeded_project_rebind_keeps_other_loaded_environments() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let other = root.path().join("other");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&other).unwrap();
    let mut app = state(root.path());
    assert_eq!(app.add_project(home.clone(), "main".into()), "proj-1");
    let config = json!({
        "environments":[
            {"id":"env-1","name":"other","sources":[{"id":"source-1","path":other,"name":"other","mount":"other"}]},
            {"id":"env-2","name":"home","sources":[{"id":"source-1","path":home,"name":"home","mount":"home"}]}
        ],
        "projects":[
            {"id":"proj-1","environment_id":"env-2","project_key":home,"home_path":home,"path":home,"added_sources":[]},
            {"id":"proj-2","environment_id":"env-1","project_key":other,"home_path":other,"path":other,"added_sources":[]}
        ]
    });
    app.projects.restore_environments(&config);
    app.restore_configured_projects(&config);
    assert!(app.projects.environment("env-1").is_some());
    assert!(app.projects.environment("env-2").is_some());
    assert_eq!(app.project_for("proj-1").unwrap().sources[0].path, home);
    assert_eq!(app.project_for("proj-2").unwrap().sources[0].path, other);
    assert!(app.projects.parked().is_empty());
}

#[test]
fn unknown_environment_reference_reserves_runtime_id() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let other = root.path().join("other");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&other).unwrap();
    let mut app = state(root.path());
    let config = json!({"next_environment":1,"environments":[],"projects":[{
        "id":"proj-7","environment_id":"env-99","project_key":home,
        "home_path":home,"path":home,"added_sources":[]} ]});
    app.projects.restore_environments(&config);
    app.restore_configured_projects(&config);
    assert_eq!(app.projects.parked().len(), 1);
    let added = app.add_project(other, "main".into());
    assert_eq!(app.project_for(&added).unwrap().environment_id, "env-100");
    assert!(app.revive_parked_projects().is_empty());
    assert!(app.projects.environment("env-99").is_none());
}

#[test]
fn unavailable_sources_are_stored_but_excluded_from_live_projects_and_workspaces() {
    for legacy in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let missing = root.path().join("missing");
        let not_directory = root.path().join("file");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("notes.txt"), "copy this\n").unwrap();
        std::fs::write(&not_directory, "not a source directory\n").unwrap();
        let sources = json!([
            {"id":"source-1","path":home,"name":"home","mount":"home"},
            {"id":"source-2","path":missing,"name":"missing","mount":"missing"},
            {"id":"source-3","path":not_directory,"name":"file","mount":"file"}
        ]);
        let config = if legacy {
            json!({"projects":[{"id":"proj-1","path":home,"sources":sources}]})
        } else {
            json!({
                "environments":[{"id":"env-1","name":"home","sources":[sources[0].clone(),sources[1].clone()]}],
                "projects":[{"id":"proj-1","environment_id":"env-1","project_key":home,
                    "home_path":home,"path":home,"added_sources":[sources[2].clone()]}]
            })
        };
        let path = root.path().join("config.json");
        std::fs::write(&path, config.to_string()).unwrap();
        let mut app = state(root.path()).with_config(&path).unwrap();
        let live = app.sources_for("proj-1").unwrap();
        assert_eq!(
            live.iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            ["source-1"]
        );
        assert_eq!(app.project_for("proj-1").unwrap().sources.len(), 1);
        // An unrelated source setting must not turn filtering into a stored unlink.
        app.commit_project_configuration("proj-1", |projects| {
            projects.source_mut("proj-1", "source-1").unwrap().sync_base = Some(false);
            Ok(())
        })
        .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let stored_count = saved["environments"][0]["sources"]
            .as_array()
            .unwrap()
            .len()
            + saved["projects"][0]["added_sources"]
                .as_array()
                .unwrap()
                .len();
        assert_eq!(stored_count, 3);
        assert_eq!(saved["projects"][0]["sources"].as_array().unwrap().len(), 1);
        let created = rpc(
            &mut app,
            "workspace.create",
            json!({"project_id":"proj-1","name":"available-only"}),
        );
        assert_eq!(created["ok"], true, "{created}");
        assert_eq!(
            created["result"]["directories"].as_array().unwrap().len(),
            1,
            "{created}"
        );
        let reloaded = state(root.path()).with_config(&path).unwrap();
        assert_eq!(reloaded.sources_for("proj-1").unwrap().len(), 1);
    }
}

#[test]
fn removing_first_source_preserves_project_home_metadata() {
    for secondary_git in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = crate::git_fixture::init_repo_named(root.path(), "home");
        let secondary = root.path().join("secondary");
        std::fs::create_dir(&secondary).unwrap();
        if secondary_git {
            git2::Repository::init(&secondary).unwrap();
        }
        let path = root.path().join("config.json");
        std::fs::write(&path, json!({"projects":[{
            "id":"proj-1","path":home,"base_branch":"home-base","sources":[
                {"id":"source-1","path":home,"name":"home","mount":"home","base_branch":"home-base"},
                {"id":"source-2","path":secondary,"name":"secondary","mount":"secondary","base_branch":"other-base"}
            ]}]}).to_string()).unwrap();
        let mut app = state(root.path()).with_config(&path).unwrap();
        let removed = rpc(
            &mut app,
            "project.remove_source",
            json!({"project_id":"proj-1","source_id":"source-1"}),
        );
        assert_eq!(removed["ok"], true, "{removed}");
        assert_home_metadata(&app, &home, &secondary);
        let reloaded = state(root.path()).with_config(&path).unwrap();
        assert_home_metadata(&reloaded, &home, &secondary);
    }
}

fn assert_home_metadata(app: &AppState, home: &std::path::Path, secondary: &std::path::Path) {
    let project = app.project_for("proj-1").unwrap();
    assert_eq!(project.repo_path, home);
    assert!(
        project.is_git,
        "source removal cannot change the original home's classification"
    );
    assert_eq!(project.base_branch, "home-base");
    assert_eq!(project.orch.worktrees().repo_path(), home);
    assert_eq!(project.sources.len(), 1);
    assert_eq!(project.sources[0].path, secondary);
    assert_eq!(project.sources[0].base_branch, "other-base");
}

#[test]
fn already_diverged_legacy_record_keeps_project_home_metadata() {
    for secondary_git in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = crate::git_fixture::init_repo_named(root.path(), "home");
        let secondary = root.path().join("secondary");
        std::fs::create_dir(&secondary).unwrap();
        if secondary_git {
            git2::Repository::init(&secondary).unwrap();
        }
        let path = root.path().join("config.json");
        // This is a legacy record saved after the home source was removed.
        std::fs::write(&path, json!({"projects":[{
            "id":"proj-1","path":home,"base_branch":"home-base","sources":[
                {"id":"source-2","path":secondary,"name":"secondary","mount":"secondary","base_branch":"other-base"}
            ]}]}).to_string()).unwrap();
        let app = state(root.path()).with_config(&path).unwrap();
        assert_home_metadata(&app, &home, &secondary);
        app.persist();
        let reloaded = state(root.path()).with_config(&path).unwrap();
        assert_home_metadata(&reloaded, &home, &secondary);
    }
}

#[test]
fn unavailable_source_list_keeps_legacy_home_fallback_without_storing_it() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let missing = root.path().join("missing");
    let added = root.path().join("added");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&added).unwrap();
    std::fs::write(home.join("notes.txt"), "home remains usable\n").unwrap();
    let path = root.path().join("config.json");
    std::fs::write(
        &path,
        json!({"projects":[{"id":"proj-1","path":home,"sources":[
            {"id":"source-2","name":"missing","mount":"missing","path":missing}
        ]}]})
        .to_string(),
    )
    .unwrap();
    let mut app = state(root.path()).with_config(&path).unwrap();
    let live = app.sources_for("proj-1").unwrap();
    assert_eq!(
        live.len(),
        1,
        "the base loader retains its initial home source"
    );
    assert_eq!(live[0].id, "source-1");
    assert_eq!(live[0].path, home);
    app.commit_project_configuration("proj-1", |projects| {
        projects.source_mut("proj-1", "source-1").unwrap().sync_base = Some(false);
        Ok(())
    })
    .unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["environments"][0]["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        saved["environments"][0]["sources"][0]["path"],
        json!(missing)
    );
    assert!(saved["projects"][0]["added_sources"]
        .as_array()
        .unwrap()
        .is_empty());
    let mut app = state(root.path()).with_config(&path).unwrap();
    assert_eq!(app.sources_for("proj-1").unwrap()[0].sync_base, Some(false));
    let created = rpc(
        &mut app,
        "workspace.create",
        json!({"project_id":"proj-1","name":"fallback"}),
    );
    assert_eq!(created["ok"], true, "{created}");
    let appended = rpc(
        &mut app,
        "project.add_source",
        json!({"project_id":"proj-1","path":added,"name":"added"}),
    );
    assert_eq!(appended["ok"], true, "{appended}");
    let app = state(root.path()).with_config(&path).unwrap();
    assert_eq!(
        app.sources_for("proj-1")
            .unwrap()
            .iter()
            .map(|source| source.id.as_str())
            .collect::<Vec<_>>(),
        ["source-1", "source-3"]
    );
    assert_eq!(
        app.projects.environment("env-1").unwrap().sources[0].path,
        missing
    );
}

#[test]
fn fallback_promotion_preserves_hidden_source_one_and_saved_choices() {
    for parked_number in [None, Some(40)] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let missing = root.path().join("missing");
        let added = root.path().join("added");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&added).unwrap();
        let path = root.path().join("config.json");
        std::fs::write(
            &path,
            json!({"projects":[{"id":"proj-1","path":home,"next_source":9,"sources":[
                {"id":"source-1","name":"missing","mount":"missing","path":missing}
            ]}]})
            .to_string(),
        )
        .unwrap();
        state(root.path()).with_config(&path).unwrap();
        if let Some(number) = parked_number {
            let mut config: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            config["projects"].as_array_mut().unwrap().push(json!({
                "id":"proj-7","environment_id":"env-1","project_key":"project:parked",
                "home_path":root.path().join("away"),"path":root.path().join("away"),
                "added_sources":[{"id":format!("source-{number}"),"path":root.path().join("away-source")}]
            }));
            std::fs::write(&path, config.to_string()).unwrap();
        }
        let mut app = state(root.path()).with_config(&path).unwrap();
        app.commit_project_configuration("proj-1", |projects| {
            let source = projects.source_mut("proj-1", "source-1").unwrap();
            source.name = "Chosen home".to_string();
            source.sync_base = Some(false);
            Ok(())
        })
        .unwrap();
        let mut app = state(root.path()).with_config(&path).unwrap();
        assert_eq!(app.sources_for("proj-1").unwrap()[0].sync_base, Some(false));
        let appended = rpc(
            &mut app,
            "project.add_source",
            json!({
                "project_id":"proj-1","path":added,"name":"added"
            }),
        );
        assert_eq!(appended["ok"], true, "{appended}");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["environments"][0]["sources"][0]["id"], "source-1");
        assert_eq!(
            saved["environments"][0]["sources"][0]["path"],
            json!(missing),
            "promoting the fallback must not replace the hidden reference"
        );
        let expected_number = parked_number.map_or(10, |number| number + 1);
        let app = state(root.path()).with_config(&path).unwrap();
        let sources = app.sources_for("proj-1").unwrap();
        let promoted = sources.iter().find(|source| source.path == home).unwrap();
        assert_eq!(promoted.id, format!("source-{expected_number}"));
        assert_eq!(promoted.name, "Chosen home");
        assert_eq!(promoted.mount, "home");
        assert_eq!(promoted.sync_base, Some(false));
        assert_eq!(
            sources
                .iter()
                .find(|source| source.path == added)
                .unwrap()
                .id,
            "source-9"
        );
        assert_eq!(
            app.projects.environment("env-1").unwrap().sources[0].path,
            missing
        );
        assert_eq!(
            app.projects.environment("env-1").unwrap().next_source,
            expected_number + 1
        );
        let stored_home = saved["projects"][0]["added_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["path"] == json!(home))
            .unwrap();
        assert_eq!(stored_home["id"], promoted.id);
    }
}
