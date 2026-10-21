//! Pending shells hold only the checkout they will enter. Physical scope
//! resolution is external work and must leave unrelated frames responsive.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    handler: FrameHandler,
    projects: [String; 2],
    workspaces: [String; 2],
}

impl Fixture {
    fn new() -> Self {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let second = init_repo_named(dir.path(), "second-project");
        let projects = {
            let mut app = state.lock().unwrap();
            let first = app.project_at(0).id.clone();
            let second = app.add_project(second, "main".into());
            let shell = dir.path().join("finite-shell");
            std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
            app.term_shell = shell.display().to_string();
            [first, second]
        };
        let workspaces = projects.each_ref().map(|project| {
            let created = call(
                &handler,
                "workspace.create",
                json!({"project_id":project,"name":"shell-checkout","isolation":"worktree"}),
            );
            assert_eq!(created["ok"], true, "{created}");
            created["result"]["workspace_id"]
                .as_str()
                .unwrap()
                .to_owned()
        });
        Self {
            _dir: dir,
            state,
            handler,
            projects,
            workspaces,
        }
    }

    fn pause_create(&self, gate: OffLockGate) -> std::thread::JoinHandle<Value> {
        self.state.lock().unwrap().terminal_create_spawn_gate = Some(gate);
        self.create()
    }

    fn create(&self) -> std::thread::JoinHandle<Value> {
        let handler = self.handler.clone();
        let workspace = self.workspaces[0].clone();
        std::thread::spawn(move || call(&handler, "term.create", json!({"workspace_id":workspace})))
    }

    fn close(&self, created: &Value) {
        if created["ok"] != true {
            return;
        }
        let child = {
            let app = self.state.lock().unwrap();
            let key = app
                .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
                .unwrap();
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
        };
        let closed = call(
            &self.handler,
            "term.close",
            json!({"term_id":created["result"]["term_id"]}),
        );
        child.end();
        assert!(child.exited_within(Duration::from_secs(2)));
        assert_eq!(closed["ok"], true, "{closed}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_creation_holds_only_its_workspace_and_project() {
    let f = Fixture::new();
    let (gate, held) = OffLockGate::new();
    let creating = f.pause_create(gate);
    held.wait_for_arrival();
    let rename = |workspace: &str| {
        call(
            &f.handler,
            "workspace.rename",
            json!({"workspace_id":workspace,"name":"renamed"}),
        )
    };
    let lock = |workspace: &str, locked| {
        call(
            &f.handler,
            "workspace.set_locked",
            json!({"workspace_id":workspace,"locked":locked}),
        )
    };
    let rename_b = rename(&f.workspaces[1]);
    let lock_b = lock(&f.workspaces[1], true);
    let unlock_b = lock(&f.workspaces[1], false);
    let rename_a = rename(&f.workspaces[0]);
    let lock_a = lock(&f.workspaces[0], true);
    let reclaim_a = call(
        &f.handler,
        "workspace.reclaim",
        json!({"workspace_id":f.workspaces[0]}),
    );
    let reclaim_b = call(
        &f.handler,
        "workspace.reclaim",
        json!({"workspace_id":f.workspaces[1]}),
    );
    let delete_a = call(
        &f.handler,
        "project.delete",
        json!({"project_id":f.projects[0],"confirm":true}),
    );
    let delete_b = call(
        &f.handler,
        "project.delete",
        json!({"project_id":f.projects[1],"confirm":true}),
    );
    held.release();
    let created = creating.join().unwrap();
    f.close(&created);
    assert_eq!(created["ok"], true, "{created}");
    for reply in [rename_b, lock_b, unlock_b, delete_b] {
        assert_eq!(reply["ok"], true, "unrelated scope was held: {reply}");
    }
    for reply in [rename_a, lock_a, reclaim_a, delete_a] {
        assert_eq!(reply["ok"], false, "held scope changed: {reply}");
    }
    assert_ne!(
        reclaim_b["error_code"], "busy",
        "unrelated reclaim could not be admitted: {reclaim_b}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_creation_physical_scope_probe_releases_the_app_lock() {
    let f = Fixture::new();
    let (gate, held) = OffLockGate::new();
    f.state.lock().unwrap().terminal_create_scope_probe_gate = Some(gate);
    let creating = f.create();
    held.wait_for_arrival();
    let (sent, received) = std::sync::mpsc::channel();
    let concurrent = {
        let handler = f.handler.clone();
        std::thread::spawn(move || {
            let _ = sent.send(call(&handler, "session.hello", json!({})));
        })
    };
    let reply = received.recv_timeout(Duration::from_millis(100));
    held.release();
    let created = creating.join().unwrap();
    f.close(&created);
    concurrent.join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(
        reply.expect("physical scope resolution held the app mutex")["ok"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_creation_discovers_unregistered_external_workspace_offlock() {
    let f = Fixture::new();
    let repo = f
        .state
        .lock()
        .unwrap()
        .projects
        .get(&f.projects[0])
        .unwrap()
        .repo_path
        .clone();
    add_external_worktree(&repo, f._dir.path(), "fresh-external", "fresh-external");
    let root = std::fs::canonicalize(f._dir.path().join("fresh-external")).unwrap();
    let workspace_id = crate::worktree::external_worktree_id(&root);
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspaces
        .get(&workspace_id)
        .is_none());
    let (gate, held) = OffLockGate::new();
    f.state.lock().unwrap().terminal_create_scope_probe_gate = Some(gate);
    let creating = {
        let handler = f.handler.clone();
        let workspace_id = workspace_id.clone();
        std::thread::spawn(move || {
            call(
                &handler,
                "term.create",
                json!({"workspace_id":workspace_id}),
            )
        })
    };
    held.wait_for_arrival();
    let (sent, received) = std::sync::mpsc::channel();
    let concurrent = {
        let handler = f.handler.clone();
        std::thread::spawn(move || {
            let _ = sent.send(call(&handler, "session.hello", json!({})));
        })
    };
    let reply = received.recv_timeout(Duration::from_millis(100));
    held.release();
    let created = creating.join().unwrap();
    let actual_root = if created["ok"] == true {
        let app = f.state.lock().unwrap();
        Some(
            app.tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
                .unwrap()
                .root,
        )
    } else {
        None
    };
    // Resolve no other legacy aliases before checking immediate identity.
    // Terminal publication and a later term.list must not need global adoption.
    let adopted = f
        .state
        .lock()
        .unwrap()
        .workspaces
        .get(&workspace_id)
        .cloned();
    f.close(&created);
    concurrent.join().unwrap();
    assert_eq!(
        created["ok"], true,
        "a valid Git worktree must remain addressable before legacy adoption: {created}"
    );
    let adopted = adopted.expect("the selected legacy alias must be installed before publication");
    assert_eq!(adopted.root, root);
    assert!(!adopted.managed);
    assert_eq!(actual_root, Some(root));
    assert_eq!(
        reply.expect("legacy Git discovery held the app mutex")["ok"],
        true
    );
}
