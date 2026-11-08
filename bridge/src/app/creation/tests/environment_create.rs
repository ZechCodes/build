use super::*;

fn params(f: &Fixture) -> Value {
    json!({"name":"New environment","sources":[{"path":f._root.path().join("home")}],"project":{"name":"First project","workspace":{"name":"first"}}})
}
fn submit(f: &Fixture, n: u128, value: Value) -> String {
    let id = operation(n);
    admit(
        &f.state,
        "environment.create",
        value,
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    id
}
fn added(f: &Fixture, n: u128, path: &std::path::Path) -> String {
    let mut value = f.params();
    value["added_sources"] = json!([{"path":path}]);
    let id = operation(n);
    admit(
        &f.state,
        "environment.create_project",
        value,
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded", "{:?}", f.record(&id));
    id
}
#[test]
fn environment_create_is_one_attempt() {
    let f = Fixture::new();
    let id = submit(&f, 300, params(&f));
    assert_eq!(f.state.lock().unwrap().projects.len(), 1);
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "succeeded", "{record:?}");
    assert_eq!(
        record.outcome.as_ref().unwrap()["result"]["environment"]["environment_id"],
        record.execution["environment_id"]
    );
    let steps = record.execution["steps"].as_array().unwrap();
    for step in ["environment", "project", "workspace"] {
        assert!(steps
            .iter()
            .any(|s| s["step"] == step && s["state"] == "done"));
    }
    assert_eq!(f.store.writer().pending_creations().unwrap().len(), 0);
    f.run(&id);
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
}
#[test]
fn environment_create_refusal_writes_nothing() {
    let f = Fixture::new();
    let before = std::fs::read(f._root.path().join("config.json")).unwrap();
    let mut value = params(&f);
    value["sources"] = json!([{"path":f._root.path().join("missing")}]);
    assert!(admit(
        &f.state,
        "environment.create",
        value,
        WriteMetadata {
            operation_id: operation(301),
            after: None
        }
    )
    .is_err());
    assert_eq!(
        std::fs::read(f._root.path().join("config.json")).unwrap(),
        before
    );
    // Clone failure is an admitted source step; it never publishes config.
    let mut value = params(&f);
    value["sources"] = json!([{"remote":"https://127.0.0.1:1/absent.git"}]);
    let id = submit(&f, 302, value);
    f.run(&id);
    assert_eq!(f.record(&id).state, "failed", "{:?}", f.record(&id));
    assert_eq!(f.record(&id).outcome.unwrap()["effect"], "unapplied");
    assert_eq!(
        std::fs::read(f._root.path().join("config.json")).unwrap(),
        before
    );
    assert_eq!(f.state.lock().unwrap().projects.environments().count(), 1);
}
#[test]
fn project_step_failure_leaves_resumable_empty_environment() {
    let f = Fixture::new();
    let id = submit(&f, 303, params(&f));
    let config = f._root.path().join("config.json");
    let saved = std::fs::read(&config).unwrap();
    let path = config.clone();
    let _observer = observe_boundary(move |at| {
        if at == Boundary::EnvironmentPersisted {
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
    });
    f.run(&id);
    drop(_observer);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.as_ref().unwrap()["effect"], "partial");
    let environment = record.execution["environment_id"].as_str().unwrap();
    assert_eq!(f.state.lock().unwrap().projects.len(), 1);
    assert!(f
        .state
        .lock()
        .unwrap()
        .projects
        .environment(environment)
        .is_some());
    std::fs::remove_dir(&config).unwrap();
    std::fs::write(&config, saved).unwrap();
    f.state.lock().unwrap().persist();
    let next = operation(304);
    admit(
        &f.state,
        "environment.create_project",
        json!({"environment_id":environment,"name":"Resumed","workspace":{},"continues":id}),
        WriteMetadata {
            operation_id: next.clone(),
            after: None,
        },
    )
    .unwrap();
    f.run(&next);
    assert_eq!(f.record(&next).state, "succeeded");
}
#[test]
fn environment_discard_unused_refused_with_projects() {
    let f = Fixture::new();
    let id = submit(&f, 305, params(&f));
    f.run(&id);
    let environment = f.record(&id).execution["environment_id"].clone();
    let error = f
        .state
        .lock()
        .unwrap()
        .route(
            "environment.discard_unused",
            &json!({"environment_id":environment,"creation_operation_id":id}),
            None,
        )
        .unwrap_err();
    assert!(error.message().contains("projects"), "{error:?}");
}
#[test]
fn added_source_makes_project_diverged() {
    let f = Fixture::new();
    let extra = f._root.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    let id = added(&f, 306, &extra);
    let app = f.state.lock().unwrap();
    let project = app
        .projects
        .get(f.record(&id).execution["project_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(project.added_sources.len(), 1);
    assert_eq!(app.project_json(project)["diverged"], true);
    assert_eq!(
        app.project_json(project)["added_sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        app.workspaces.list(Some(&project.id))[0].directories.len(),
        2
    );
}
#[test]
fn added_source_may_equal_other_owners_source() {
    let f = Fixture::new();
    let extra = f._root.path().join("other");
    std::fs::create_dir(&extra).unwrap();
    f.state
        .lock()
        .unwrap()
        .add_project(extra.clone(), "main".into());
    added(&f, 307, &extra);
    added(&f, 308, &extra);
}
#[test]
fn added_source_equal_to_own_environment_source_refused() {
    let f = Fixture::new();
    let mut value = f.params();
    value["added_sources"] = json!([{"path":f._root.path().join("home")}]);
    let result = admit(
        &f.state,
        "environment.create_project",
        value,
        WriteMetadata {
            operation_id: operation(309),
            after: None,
        },
    );
    assert!(result.is_err(), "{result:?}");
    assert!(result.unwrap_err().message().contains("already included"));
    assert_eq!(f.state.lock().unwrap().projects.len(), 1);
}
#[test]
fn project_verbs_refuse_environment_source_ids() {
    let f = Fixture::new();
    let extra = f._root.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    added(&f, 310, &extra);
    let mut app = f.state.lock().unwrap();
    for (verb, extra) in [
        ("project.remove_source", json!({})),
        ("project.update_source", json!({"name":"changed"})),
        ("project.sync_source", json!({})),
    ] {
        let mut value = json!({"project_id":"proj-2","source_id":"source-1"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let error = app.route(verb, &value, None).unwrap_err();
        assert_eq!(error.code(), "conflict");
        assert!(
            error.message().contains("belongs to environment"),
            "{verb}: {error:?}"
        );
    }
}

#[test]
fn environment_creation_recovers_each_new_step_boundary() {
    for (n, boundary) in [
        Boundary::SourceStarted,
        Boundary::SourcePrepared,
        Boundary::EnvironmentCheckpointed,
        Boundary::EnvironmentStarted,
        Boundary::EnvironmentPersisted,
        Boundary::ProjectPersisted,
        Boundary::WorkspaceProvisioned,
    ]
    .into_iter()
    .enumerate()
    {
        let f = Fixture::new();
        let id = submit(&f, 320 + n as u128, params(&f));
        let observer = observe_boundary(move |at| {
            if at == boundary {
                std::panic::panic_any(SimulatedCrash);
            }
        });
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
        drop(observer);
        assert!(crash.unwrap_err().is::<SimulatedCrash>());
        let f = f.reopen();
        f.store.writer().recover_mutations().unwrap();
        f.run(&id);
        assert_eq!(f.record(&id).state, "succeeded", "{:?}", f.record(&id));
        assert_eq!(f.state.lock().unwrap().projects.environments().count(), 2);
        assert_eq!(f.state.lock().unwrap().projects.len(), 2);
        assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
    }
}

#[test]
fn device_project_names_survive_environment_creation_and_sibling_project_create_reload() {
    let f = Fixture::new();
    let id = submit(&f, 340, params(&f));
    f.run(&id);
    let environment = f.record(&id).execution["environment_id"].clone();
    let second = {
        let mut app = f.state.lock().unwrap();
        let row = app
            .dispatch(
                "project.create",
                &json!({"environment_id":environment,"name":"Second project","sources":[]}),
            )
            .unwrap();
        row["project_id"].as_str().unwrap().to_owned()
    };
    let f = f.reopen();
    let app = f.state.lock().unwrap();
    let config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    let first = f.record(&id).execution["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (project, name) in [(first, "First project"), (second, "Second project")] {
        assert_eq!(
            config["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == project)
                .unwrap()["name"],
            name,
            "config projection retains the configured name after reload"
        );
        app.initialize_device_lifecycle().unwrap();
        let connection = f.store.writer_for_test();
        let row: String = connection
            .query_row(
                "SELECT record FROM sync_projection WHERE kind='project' AND key=?1 AND deleted=0",
                [project],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&row).unwrap()["name"], name);
    }
}

#[test]
fn source_ownership_ambiguity_keeps_unknown_attempt_fenced() {
    let f = Fixture::new();
    let mut value = params(&f);
    value["sources"] = json!([{"remote":"https://127.0.0.1:1/unavailable.git"}]);
    let id = submit(&f, 341, value.clone());
    let root = f.record(&id).execution["source_root"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(std::path::Path::new(&root).join("foreign"), b"keep").unwrap();
    f.run(&id);
    assert_eq!(f.record(&id).state, "unknown");
    assert!(f.record(&id).outcome.is_none());
    assert!(admit(
        &f.state,
        "environment.create",
        value,
        WriteMetadata {
            operation_id: operation(342),
            after: None
        }
    )
    .is_err());
    let f = f.reopen();
    f.run(&id);
    assert_eq!(f.record(&id).state, "unknown");
    assert_eq!(
        std::fs::read(std::path::Path::new(&root).join("foreign")).unwrap(),
        b"keep"
    );
}
#[test]
fn source_settlement_terminal_intent_survives_restart() {
    let f = Fixture::new();
    let mut value = params(&f);
    value["sources"] = json!([{"remote":"https://127.0.0.1:1/unavailable.git"}]);
    let id = submit(&f, 343, value);
    let observer = observe_boundary(|at| {
        if at == Boundary::BeforeFailureTerminal {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
    drop(observer);
    assert!(crash.is_err());
    assert_eq!(f.record(&id).execution["effect_state"], "settled");
    let f = f.reopen();
    f.run(&id);
    assert_eq!(f.record(&id).state, "failed");
    assert_eq!(f.state.lock().unwrap().projects.environments().count(), 1);
}

#[tokio::test]
async fn environment_attempt_status_exposes_pending_progress_and_terminal_result() {
    let f = Fixture::new();
    let id = submit(&f, 344, params(&f));
    let service = f.state.lock().unwrap().mutation_service();
    service.set_progress(
        &id,
        vec![WriteProgress {
            step: "environment".into(),
            state: ProgressState::Running,
            detail: None,
        }],
    );
    let (sender, _rx, _) = crate::carrier::SessionSender::observable("environment-status");
    let frame=serde_json::from_value(json!({"session_id":"environment-status","message_id":"status","frame_type":"data","sender":"client","created_at":"","payload":{"id":1,"method":"writes.status","params":{"operation_ids":[id]}}})).unwrap();
    let connection = f.store.read_pool().acquire().await;
    let status = service.call(sender.clone(), frame, Some(connection));
    assert_eq!(
        status["result"]["operations"][0]["state"], "accepted",
        "{status}"
    );
    assert_eq!(
        status["result"]["operations"][0]["progress"][0]["step"],
        "environment"
    );
    f.run(&id);
    let frame=serde_json::from_value(json!({"session_id":"environment-status","message_id":"status2","frame_type":"data","sender":"client","created_at":"","payload":{"id":2,"method":"writes.status","params":{"operation_ids":[id]}}})).unwrap();
    let connection = f.store.read_pool().acquire().await;
    let status = service.call(sender, frame, Some(connection));
    assert_eq!(
        status["result"]["operations"][0]["state"], "succeeded",
        "{status}"
    );
}

fn local_remote(f: &Fixture) -> std::path::PathBuf {
    let path = f._root.path().join("remote");
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main").external_template(false);
    let repository = git2::Repository::init_opts(&path, &options).unwrap();
    std::fs::write(path.join("file"), b"seed").unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(std::path::Path::new("file")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
    repository
        .commit(Some("HEAD"), &signature, &signature, "seed", &tree, &[])
        .unwrap();
    path
}
#[test]
fn clone_source_restarts_reuse_the_owned_checkout() {
    for (index, boundary) in [
        Boundary::SourceStarted,
        Boundary::SourcePrepared,
        Boundary::EnvironmentPersisted,
    ]
    .into_iter()
    .enumerate()
    {
        let f = Fixture::new();
        let remote = local_remote(&f);
        let mut value = params(&f);
        value["sources"] = json!([{"remote":remote,"name":"cloned"}]);
        let id = submit(&f, 350 + index as u128, value);
        let root = f.record(&id).execution["source_root"]
            .as_str()
            .unwrap()
            .to_owned();
        let observer = observe_boundary(move |at| {
            if at == boundary {
                std::panic::panic_any(SimulatedCrash);
            }
        });
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
        drop(observer);
        assert!(crash.is_err());
        let cloned = std::path::Path::new(&root).join("cloned");
        if cloned.exists() {
            std::fs::write(cloned.join("file"), b"local edit").unwrap();
        }
        let existed = cloned.exists();
        let f = f.reopen();
        f.store.writer().recover_mutations().unwrap();
        f.run(&id);
        if boundary == Boundary::SourceStarted {
            assert_eq!(f.record(&id).state, "failed", "{:?}", f.record(&id));
            assert_eq!(f.record(&id).outcome.unwrap()["effect"], "unapplied");
            assert!(!cloned.exists());
            continue;
        }
        assert_eq!(f.record(&id).state, "succeeded", "{:?}", f.record(&id));
        assert_eq!(
            std::fs::read(cloned.join("file")).unwrap(),
            if existed {
                b"local edit".as_slice()
            } else {
                b"seed".as_slice()
            }
        );
        assert_eq!(f.state.lock().unwrap().projects.environments().count(), 2);
        assert_eq!(f.state.lock().unwrap().projects.len(), 2);
    }
}
#[test]
fn environment_config_refusal_rolls_back_prepared_clone_before_unapplied() {
    let f = Fixture::new();
    let remote = local_remote(&f);
    let mut value = params(&f);
    value["sources"] = json!([{"remote":remote}]);
    let id = submit(&f, 354, value);
    let root = f.record(&id).execution["source_root"]
        .as_str()
        .unwrap()
        .to_owned();
    let config = f._root.path().join("config.json");
    let blocked = config.clone();
    let observer = observe_boundary(move |at| {
        if at == Boundary::EnvironmentStarted {
            std::fs::remove_file(&blocked).unwrap();
            std::fs::create_dir(&blocked).unwrap();
        }
    });
    f.run(&id);
    drop(observer);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    assert_eq!(record.execution["effect_state"], "settled");
    assert!(!std::path::Path::new(&root).exists());
    assert_eq!(f.state.lock().unwrap().projects.environments().count(), 1);
}
#[test]
fn discard_empty_environment_retains_source_files_and_requires_terminal_origin() {
    let f = Fixture::new();
    let id = submit(&f, 355, params(&f));
    let record = f.record(&id);
    let writer = f.store.writer();
    writer.claim_mutation(PRINCIPAL, &id).unwrap();
    let mut evidence = record.execution.clone();
    super::super::sources::prepare(&f.state, &writer, &record, &mut evidence).unwrap();
    super::super::environment::step(&f.state, &writer, &record, &mut evidence).unwrap();
    let environment = evidence["environment_id"].clone();
    let value = json!({"environment_id":environment,"creation_operation_id":id});
    assert!(f
        .state
        .lock()
        .unwrap()
        .route("environment.discard_unused", &value, None)
        .is_err());
    writer.finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"creation_failed","message":"project not started","retryable":false}})).unwrap();
    f.state
        .lock()
        .unwrap()
        .route("environment.discard_unused", &value, None)
        .unwrap();
    assert_eq!(
        std::fs::read(f._root.path().join("home/source")).unwrap(),
        b"kept"
    );
    let f = f.reopen();
    assert!(f
        .state
        .lock()
        .unwrap()
        .projects
        .environment(environment.as_str().unwrap())
        .is_none());
}

#[test]
fn concurrent_environment_admission_reuses_the_winning_acknowledgement() {
    let f = Fixture::new();
    let id = operation(360);
    let value = params(&f);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let records = std::thread::scope(|scope| {
        (0..2)
            .map(|_| {
                let state = f.state.clone();
                let value = value.clone();
                let id = id.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let _observer = observe_boundary(move |at| {
                        if at == Boundary::AdmissionPrepared {
                            barrier.wait();
                        }
                    });
                    admit(
                        &state,
                        "environment.create",
                        value,
                        WriteMetadata {
                            operation_id: id,
                            after: None,
                        },
                    )
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(records.iter().all(Result::is_ok), "{records:?}");
    assert_eq!(records[0], records[1]);
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(f.state.lock().unwrap().projects.environments().count(), 2);
}

#[test]
fn legacy_update_source_routes_to_environment_without_added_sources() {
    let f = Fixture::new();
    let id = operation(370);
    f.admit(&id);
    f.run(&id);
    {
        let mut app = f.state.lock().unwrap();
        let legacy = app.projects.get("proj-1").unwrap();
        assert!(legacy.added_sources.is_empty());
        assert_eq!(legacy.project_key, legacy.home_path.display().to_string());
        app.dispatch(
            "project.update_source",
            &json!({"project_id":"proj-1","source_id":"source-1","name":"Legacy shared folder"}),
        )
        .unwrap();
        assert_eq!(
            app.projects.environment("env-1").unwrap().sources[0].name,
            "Legacy shared folder"
        );
        assert_eq!(
            app.projects.get("proj-2").unwrap().sources[0].name,
            "Legacy shared folder"
        );
    }
    let f = f.reopen();
    let app = f.state.lock().unwrap();
    assert_eq!(
        app.projects.environment("env-1").unwrap().sources[0].name,
        "Legacy shared folder"
    );
    assert!(app.projects.get("proj-1").unwrap().added_sources.is_empty());
}
#[test]
fn legacy_sync_source_routes_to_environment_without_added_sources() {
    let f = Fixture::new();
    let (home, sibling) = {
        let mut app = f.state.lock().unwrap();
        assert!(app.projects.get("proj-1").unwrap().added_sources.is_empty());
        app.dispatch("project.init_git", &json!({"project_id":"proj-1"}))
            .unwrap();
        let sibling = app.add_environment_sibling_for_test("proj-1");
        (
            app.projects.environment("env-1").unwrap().sources[0]
                .path
                .clone(),
            sibling,
        )
    };
    let upstream =
        git2::Repository::clone(home.to_str().unwrap(), f._root.path().join("upstream")).unwrap();
    std::fs::write(
        upstream.workdir().unwrap().join("news"),
        b"shared environment update",
    )
    .unwrap();
    let mut index = upstream.index().unwrap();
    index.add_path(std::path::Path::new("news")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = upstream.find_tree(tree_id).unwrap();
    let parent = upstream.head().unwrap().peel_to_commit().unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
    let expected = upstream
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "shared update",
            &tree,
            &[&parent],
        )
        .unwrap();
    git2::Repository::open(&home)
        .unwrap()
        .remote("origin", upstream.workdir().unwrap().to_str().unwrap())
        .unwrap();
    {
        let mut app = f.state.lock().unwrap();
        assert_eq!(
            app.dispatch(
                "project.sync_source",
                &json!({"project_id":"proj-1","source_id":"source-1"})
            )
            .unwrap()["pending"],
            true
        );
    }
    AppState::sync_sources(&f.state, crate::app::SyncPass::Requested, 1000);
    let mut app = f.state.lock().unwrap();
    assert_eq!(
        app.projects.get("proj-1").unwrap().sources[0]
            .sync_status
            .as_ref()
            .unwrap()
            .state,
        crate::app::projects::base_sync::SyncState::Synced
    );
    // This checks the shared Git effect through another project's RPC scope,
    // rather than merely observing the requester's per-reference sync status.
    let observed = app
        .dispatch(
            "git.log",
            &json!({"project_id":sibling,"source_id":"source-1","source_path":home,"limit":1}),
        )
        .unwrap();
    assert_eq!(observed["commits"][0]["hash"], expected.to_string());
    let environment = &app.projects.environment("env-1").unwrap().sources[0];
    assert_eq!(
        git2::Repository::open(&environment.path)
            .unwrap()
            .head()
            .unwrap()
            .target(),
        Some(expected)
    );
    assert_eq!(
        std::fs::read(
            app.projects.get(&sibling).unwrap().sources[0]
                .path
                .join("news")
        )
        .unwrap(),
        b"shared environment update"
    );
    assert!(app.projects.get("proj-1").unwrap().added_sources.is_empty());
}
