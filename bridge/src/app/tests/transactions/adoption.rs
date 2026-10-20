use super::*;

fn opened_adoption(
    app: &mut AppState,
    repo: &Path,
    home: &Path,
    owner: &str,
) -> crate::orchestrator::ActiveRun {
    add_external_worktree(repo, home, owner, owner);
    let project_id = app.project_at(0).id.clone();
    let base_branch = app.base_for(&project_id).unwrap();
    let checkout = app
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some(owner))
        .unwrap();
    app.open_adoption(&crate::lifecycle::AdoptionPrepared {
        project_id,
        run_id: owner.into(),
        base_branch: base_branch.clone(),
        checkout: crate::orchestrator::AdoptableCheckout::judge(&checkout, &base_branch).unwrap(),
        model_choice: ModelChoice::default(),
    })
    .unwrap()
}

#[test]
fn adoption_finisher_first_publication_includes_its_workspace_alias() {
    let (home, repo) = init_repo();
    let mut app = qa_state(&repo, home.path());
    let owner = "adoption-first-publication";
    let active = opened_adoption(&mut app, &repo, home.path(), owner);
    assert!(app.read_view.load().get("workspace.get", owner).is_none());

    // This is the adoption settlement's inner finisher, before any epilogue
    // maintenance can repair the first root that its notices advertise.
    let (_, saved) =
        app.answer_adopted_run_mutation(owner.into(), active, crate::thread::ThreadDetail::Digest);
    saved.unwrap();
    assert!(app.changes.has_pending(), "the finisher staged its notices");
    let reply = app
        .snapshot_service()
        .call(req("workspace.get", json!({"workspace_id": owner})));
    assert_eq!(reply["ok"], true, "the first advertised root: {reply}");
    assert_eq!(reply["result"]["run_id"], owner, "{reply}");
}

#[test]
fn failed_adoption_cannot_publish_a_workspace_alias_or_remove_its_lock_marker() {
    let (home, repo) = init_repo();
    let mut app = qa_state(&repo, home.path());
    let owner = "failed-adoption-alias";
    let active = opened_adoption(&mut app, &repo, home.path(), owner);
    let marker = app.workspaces.root().join(".locks").join(owner);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, b"locked").unwrap();
    app.store.as_ref().unwrap().fail_next_write();
    let (_, saved) =
        app.answer_adopted_run_mutation(owner.into(), active, crate::thread::ThreadDetail::Digest);
    assert!(saved.unwrap_err().contains("injected store failure"));
    assert!(app.workspaces.get(owner).is_none());

    file_legacy_task(&mut app, "unrelated accepted write");
    app.publish_snapshot_projection();
    assert!(app.workspaces.get(owner).is_none());
    assert!(
        marker.is_file(),
        "rollback must preserve the private marker"
    );
    let service = app.snapshot_service();
    let detail = service.call(req("workspace.get", json!({"workspace_id": owner})));
    assert_eq!(detail["ok"], false, "{detail}");
    let listed = service.call(req("workspace.list", json!({})));
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["workspace_id"] != owner));
}
