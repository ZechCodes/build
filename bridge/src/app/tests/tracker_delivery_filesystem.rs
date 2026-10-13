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
    assert_eq!(
        app.workspace_reclaim(&json!({"workspace_id":workspace}), &Actor::User),
        Err(crate::reclaim::BUSY.into())
    );
    let source = app.projects.get(&project).unwrap().sources[0].path.clone();
    let (held, _) =
        crate::source_sync::SyncLock::acquire(&source, std::time::Duration::ZERO).unwrap();
    let (sent, done) = std::sync::mpsc::channel();
    let running = std::thread::spawn(move || {
        let result = work.git.as_ref().map(DeferredGit::run);
        sent.send((work, result)).unwrap();
    });
    let early = done.recv_timeout(std::time::Duration::from_millis(150));
    assert_eq!(app.active_deferred_filesystem_jobs, 1);
    let refusal = app.workspace_reclaim(&json!({"workspace_id":workspace}), &Actor::User);
    drop(held);
    running.join().unwrap();
    assert!(
        early.is_err(),
        "provisioning must wait while the source is syncing"
    );
    assert_eq!(refusal, Err(crate::reclaim::BUSY.into()));
    let (work, result) = done.recv().unwrap();
    app.finish_prepared_task_delivery(work, result).unwrap();
    assert_eq!(app.active_deferred_filesystem_jobs, 0);
    app.workspace_set_locked(&json!({"workspace_id":workspace,"locked":true}))
        .unwrap();
    app.workspace_rename(&json!({"workspace_id":workspace,"name":"renamed"}))
        .unwrap();
    let persisted = crate::workspace::WorkspaceRegistry::load(app.workspaces.root()).unwrap();
    let saved = persisted.get(&workspace).unwrap();
    assert!(saved.locked);
    assert_eq!(saved.name, "renamed");
}

#[test]
fn deferred_workspace_pins_saved_git_inputs_while_source_is_held() {
    use super::project_agent::project_agent;
    use crate::app::tracker::AssignTarget;
    use crate::source_sync::SyncLock;
    use crate::workspace::Workspace;
    use serde_json::json;
    use std::sync::mpsc;
    use std::time::Duration;

    for changed_base in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (_repo, mut app, project_id) = tracked(temp.path());
        let (owner, _) = project_agent(&mut app, &project_id);
        let worker = app
            .agent_add(&json!({"entity_id":owner,"project_role":"worker"}))
            .unwrap();
        let worker_id = worker["agent"]["id"].as_str().unwrap();
        app.thread_post(&json!({"entity_id":owner,"agent_id":worker_id,"body":"hold source"}))
            .unwrap();
        let target = AssignTarget::NewWorkspace {
            name: None,
            isolation: Some("worktree".into()),
            choice: Default::default(),
            notify_user: None,
            agent_name: None,
        };
        let saved = app
            .reserve_deferred_workspace(&project_id, &target, "saved git", &Actor::User)
            .unwrap();
        let workspace: Workspace = serde_json::from_value(saved.clone()).unwrap();
        let source = &workspace.directories[0].source_path;
        let original = git2::Repository::open(source)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        assert!(app
            .refuse_moving_sources_under_workers(&project_id)
            .is_err());
        if changed_base {
            app.projects
                .source_mut(&project_id, &workspace.directories[0].source_id)
                .unwrap()
                .base_branch = "missing-current-base".into();
        }
        let state = app.shared();
        let job = state
            .lock()
            .unwrap()
            .prepare_deferred_workspace(&saved)
            .unwrap();
        let (held, _) = SyncLock::acquire(source, Duration::ZERO).unwrap();
        let (sent, done) = mpsc::channel();
        let running = std::thread::spawn(move || sent.send(job.call.run(&job.params)).unwrap());
        let early = done.recv_timeout(Duration::from_millis(150));
        let responsive = state.lock().unwrap().project_list();
        drop(held);
        running.join().unwrap();
        assert!(
            early.is_err(),
            "saved Git preparation must wait for the source lock: {early:?}"
        );
        assert_eq!(responsive["projects"].as_array().unwrap().len(), 1);
        done.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        let checkout = git2::Repository::open(&workspace.directories[0].path).unwrap();
        assert_eq!(checkout.head().unwrap().target(), Some(original));
        assert!(state
            .lock()
            .unwrap()
            .refuse_moving_sources_under_workers(&project_id)
            .is_err());
    }
}
