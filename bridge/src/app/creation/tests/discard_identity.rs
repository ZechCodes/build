use super::*;

#[test]
fn discard_key_guard_keeps_same_home_legacy_sibling_task() {
    let f = Fixture::new();
    let id = operation(180);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    project_step(&f.state, &record).unwrap();
    f.store.writer().finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace not begun","retryable":false}})).unwrap();
    let legacy_key = {
        let app = f.state.lock().unwrap();
        let legacy = app.projects.get("proj-1").unwrap();
        let created = app.projects.get("proj-2").unwrap();
        assert_eq!(legacy.home_path, created.home_path);
        assert_ne!(legacy.project_key, created.project_key);
        legacy.project_key.clone()
    };
    let task = f
        .store
        .create_tracker_task(
            crate::tracker::Task::drafted(
                &legacy_key,
                "legacy sibling task",
                crate::tracker::Actor::User,
                &crate::store::now_rfc3339(),
            ),
            &[],
        )
        .unwrap();
    f.state
        .lock()
        .unwrap()
        .project_discard_unused(&json!({"project_id":"proj-2","creation_operation_id":id}))
        .unwrap();
    assert!(f.store.load_tracker_task(&task.id).unwrap().is_some());
    let app = f.state.lock().unwrap();
    assert!(app.projects.get("proj-1").is_some());
    assert!(app.projects.get("proj-2").is_none());
}

#[test]
fn discard_refuses_reused_project_number_with_different_key() {
    let f = Fixture::new();
    let id = operation(181);
    f.admit(&id);
    f.state.lock().unwrap().config_persist_failure =
        Some(super::super::super::ConfigPersistStep::Write);
    f.run(&id);
    let f = f.reopen();
    super::admission_recovery::register_legacy(&f.state, f._root.path());
    let key = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-2")
        .unwrap()
        .project_key
        .clone();
    let error = f
        .state
        .lock()
        .unwrap()
        .project_discard_unused(&json!({"project_id":"proj-2","creation_operation_id":id}))
        .unwrap_err();
    assert!(error.contains("not created by this attempt"), "{error}");
    assert_eq!(
        f.state
            .lock()
            .unwrap()
            .projects
            .get("proj-2")
            .unwrap()
            .project_key,
        key
    );
}
