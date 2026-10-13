//! Provisioning shares the filesystem exclusion held by ordinary RPC jobs.
use super::tracker::{filed, tracked};
use super::*;
use crate::tracker::Actor;

#[test]
fn deferred_provisioning_excludes_workspace_lock_and_rename_until_settlement() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "provisioned task")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let blocker = filed(&mut app, &project, "prerequisite")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = app.tracker_project_path(&project).unwrap();
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &path,
            &task,
            std::slice::from_ref(&blocker),
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
    let assigned = app.handle(req("tasks.assign", json!({
        "task_id":task,"assignee":{"kind":"new_workspace","name":"original name","isolation":"worktree"},
    })));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let closed = app.handle(req("tasks.close", json!({"task_id":blocker})));
    assert_eq!(closed["ok"], true, "{closed}");
    while app
        .tracker_store()
        .unwrap()
        .reconcile_waiting_on(100)
        .unwrap()
        .pending
    {}
    // Pause the service after preparation has captured the workspace snapshot.
    let work = app
        .prepare_task_delivery()
        .unwrap()
        .expect("released assignment has provisioning work");
    let workspace = work.intent.reservation["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(work.git.is_some());
    assert_eq!(
        app.workspace_set_locked(&json!({"workspace_id":workspace,"locked":true})),
        Err(crate::reclaim::BUSY.into())
    );
    assert_eq!(
        app.workspace_rename(&json!({"workspace_id":workspace,"name":"renamed"})),
        Err(crate::reclaim::BUSY.into())
    );
    let result = work.git.as_ref().map(DeferredGit::run);
    app.finish_prepared_task_delivery(work, result).unwrap();
    app.workspace_set_locked(&json!({"workspace_id":workspace,"locked":true}))
        .unwrap();
    app.workspace_rename(&json!({"workspace_id":workspace,"name":"renamed"}))
        .unwrap();
    let persisted = crate::workspace::WorkspaceRegistry::load(app.workspaces.root()).unwrap();
    let saved = persisted.get(&workspace).unwrap();
    assert!(saved.locked);
    assert_eq!(saved.name, "renamed");
}
