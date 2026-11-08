use super::*;

#[test]
fn admit_returns_recorded_outcome_after_project_removed() {
    let f = Fixture::new();
    let params = json!({"project_id":"proj-1","name":"recorded"});
    let id = operation(203);
    let metadata = WriteMetadata {
        operation_id: id.clone(),
        after: None,
    };
    admit(
        &f.state,
        "workspace.create",
        params.clone(),
        metadata.clone(),
    )
    .unwrap();
    f.run(&id);
    let saved = f.record(&id);
    assert_eq!(saved.state, "succeeded");
    f.state.lock().unwrap().projects.remove("proj-1");
    let replay = admit(
        &f.state,
        "workspace.create",
        params.clone(),
        metadata.clone(),
    )
    .unwrap();
    assert_eq!(replay, saved);
    let mut changed = params;
    changed["name"] = json!("different intent");
    assert!(admit(&f.state, "workspace.create", changed, metadata.clone()).is_err());
    let changed_dependencies = WriteMetadata {
        after: Some(vec![operation(204)]),
        ..metadata
    };
    assert!(admit(
        &f.state,
        "workspace.create",
        saved.params,
        changed_dependencies
    )
    .is_err());
}
