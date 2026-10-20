use super::delivery::{open_a_turn, primary_thread};
use super::frame_locks::{gated_tab, GatedHarness};
use super::run_agents::insert_live_run;
use super::*;

fn standalone_checkout(
    state: &Arc<Mutex<AppState>>,
    repo: &Path,
    fixture_root: &Path,
    branch: &str,
) -> (String, PathBuf) {
    let mut app = state.lock().unwrap();
    let owner = adopted_run(&mut app, repo, fixture_root, branch);
    let root = AppState::canonical_root(&app.runs[&owner].worktree.path);
    (owner, root)
}

fn finishing_one_owner_preserves_an_unrelated_vanished_session(plan: bool) {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let owner = {
        let mut app = state.lock().unwrap();
        if plan {
            file_legacy_task(&mut app, "finishing A")
        } else {
            adopted_run(&mut app, &repo, dir.path(), "finishing-a")
        }
    };
    let (key, _) = insert_live_run(&state, &repo, dir.path().join("side-b"), "run-b");
    open_a_turn(&state, "run-b");
    let (session, instance, before) = {
        let app = state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (
            Arc::clone(&tab.session),
            tab.session_instance.clone(),
            app.read_view.load().get("agent.list", "run-b").cloned(),
        )
    };
    std::fs::remove_dir_all(&key.root).unwrap();

    {
        let mut app = state.lock().unwrap();
        if plan {
            let mut active = app.take_plan(&owner).unwrap();
            active.last_summary = Some("A committed".into());
            app.finish_plan_mutation(owner, active).unwrap();
        } else {
            let mut active = app.take_run(&owner).unwrap();
            active.last_summary = Some("A committed".into());
            app.finish_run_mutation(owner, active).unwrap();
        }
        let tab = app.session_registry.test_tab(&key).expect(
            "finishing A must leave B's vanished checkout to the background cleanup service",
        );
        assert!(Arc::ptr_eq(&tab.session, &session));
        assert_eq!(tab.session_instance, instance);
        assert!(
            primary_thread(&app.runs["run-b"].agents)
                .working_since()
                .is_some(),
            "A's finisher must not close B's open turn"
        );
        assert_eq!(
            app.read_view.load().get("agent.list", "run-b").cloned(),
            before,
            "A's completion must not publish unrelated B cleanup"
        );
    }
    let reaped = AppState::reap_orphaned_terminals(&state);
    assert_eq!(reaped, vec![key.tab_id.clone()]);
    let app = state.lock().unwrap();
    assert!(app.session_registry.test_tab(&key).is_none());
    assert!(
        primary_thread(&app.runs["run-b"].agents)
            .working_since()
            .is_none(),
        "background cleanup closes B's turn after A has completed"
    );
    assert_ne!(
        app.read_view.load().get("agent.list", "run-b").cloned(),
        before,
        "cleanup publishes B's stopped session before returning"
    );
}

#[tokio::test]
async fn plan_finisher_does_not_reap_an_unrelated_vanished_checkout() {
    finishing_one_owner_preserves_an_unrelated_vanished_session(true);
}

#[tokio::test]
async fn run_finisher_does_not_reap_an_unrelated_vanished_checkout() {
    finishing_one_owner_preserves_an_unrelated_vanished_session(false);
}

#[tokio::test]
async fn abandoning_a_checkout_closes_its_shell_and_waiting_screen_before_reply() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (owner, root) = standalone_checkout(&state, &repo, dir.path(), "abandon");
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, &owner);
    {
        let mut app = state.lock().unwrap();
        app.session_registry.test_insert_tab(
            shell_key.clone(),
            gated_tab(&root, TabRole::Shell, GatedHarness::new()),
        );
        app.session_registry.test_remember_waiting_screen(
            waiting_key.clone(),
            ScreenHandle::new(&waiting_key.tab_id, 80, 24),
        );
    }

    let reply = call(&handler, "run.abandon", json!({"run_id": owner}));
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert!(!root.exists(), "the lifecycle worker removed the checkout");
    let app = state.lock().unwrap();
    assert!(
        app.session_registry.test_tab(&shell_key).is_none(),
        "the owner-local cleanup closes its shell before abandonment completes"
    );
    assert!(
        !app.session_registry
            .waiting_screen_keys()
            .contains(&waiting_key),
        "the owner-local cleanup closes a screen that can no longer spawn"
    );
}

#[tokio::test]
async fn a_removed_checkout_receipt_does_not_close_replacement_tabs() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let (owner, root) = standalone_checkout(&state, &repo, dir.path(), "abandon");
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, &owner);
    let job = {
        let mut app = state.lock().unwrap();
        app.session_registry.test_insert_tab(
            shell_key.clone(),
            gated_tab(&root, TabRole::Shell, GatedHarness::new()),
        );
        app.session_registry.test_remember_waiting_screen(
            waiting_key.clone(),
            ScreenHandle::new(&waiting_key.tab_id, 80, 24),
        );
        app.run_abandon(&json!({"run_id": owner})).unwrap();
        match app
            .deferred_work
            .take()
            .expect("abandon defers checkout removal")
        {
            DeferredWork::Lifecycle(job) => job,
            _ => panic!("abandon must schedule a lifecycle job"),
        }
    };
    let outcome = job.run();
    assert!(!root.exists(), "the receipt describes a removed root");
    std::fs::create_dir_all(&root).unwrap();
    let replacement = gated_tab(&root, TabRole::Shell, GatedHarness::new());
    let session = Arc::clone(&replacement.session);
    {
        let mut app = state.lock().unwrap();
        app.session_registry
            .test_insert_tab(shell_key.clone(), replacement);
        app.session_registry.test_remember_waiting_screen(
            waiting_key.clone(),
            ScreenHandle::new(&waiting_key.tab_id, 100, 30),
        );
        app.apply_lifecycle(outcome).unwrap();
        assert!(Arc::ptr_eq(
            &app.session_registry
                .test_tab(&shell_key)
                .expect("the removal receipt must not close a replacement tab")
                .session,
            &session,
        ));
        assert!(
            app.session_registry
                .waiting_screen_keys()
                .contains(&waiting_key),
            "the removal receipt must not close a replacement waiting screen"
        );
    }
}

#[test]
fn a_blocked_root_probe_leaves_the_app_mutex_available() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = dir.path().join("vanished");
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(&root, TabRole::Shell, GatedHarness::new()),
    );
    let (arrived, arrival) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let reaping = Arc::clone(&state);
    let worker = std::thread::spawn(move || {
        AppState::reap_orphaned_terminals_with_probe(&reaping, |path| {
            arrived.send(()).unwrap();
            released.recv().unwrap();
            path.exists()
        })
    });
    arrival
        .recv_timeout(Duration::from_secs(5))
        .expect("the root probe started");
    let available = state.try_lock().is_ok();
    release.send(()).unwrap();
    let reaped = worker.join().unwrap();
    assert!(available, "filesystem discovery must release the app guard");
    assert_eq!(reaped, vec![key.tab_id]);
}

#[test]
fn a_captured_disappearance_cannot_reap_a_replacement_agent_session() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = dir.path().join("vanished");
    let agent_id = crate::agent::derived_agent_id("run-b");
    let key = TabKey::agent(&root, &agent_id);
    let role = TabRole::Agent {
        owner: "run-b".into(),
        agent_id,
        provider: AgentProvider::default(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(&root, role.clone(), GatedHarness::new()),
    );
    let replacement = gated_tab(&root, role, GatedHarness::new());
    let session = Arc::clone(&replacement.session);
    let mut replacement = Some(replacement);
    let reaped = AppState::reap_orphaned_terminals_with_probe(&state, |path| {
        let exists = path.exists();
        std::fs::create_dir_all(path).unwrap();
        state
            .lock()
            .unwrap()
            .session_registry
            .test_insert_tab(key.clone(), replacement.take().unwrap());
        exists
    });
    assert!(
        reaped.is_empty(),
        "the disappearance belongs to the old registration"
    );
    assert!(Arc::ptr_eq(
        &state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .session,
        &session
    ));
}

#[test]
fn a_captured_disappearance_cannot_reap_a_replacement_waiting_screen() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = dir.path().join("vanished");
    let key = derived_agent_key(&root, "run-b");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_remember_waiting_screen(key.clone(), ScreenHandle::new(&key.tab_id, 80, 24));
    let replacement = ScreenHandle::new(&key.tab_id, 100, 30);
    let reaped = AppState::reap_orphaned_terminals_with_probe(&state, |path| {
        let exists = path.exists();
        std::fs::create_dir_all(path).unwrap();
        state
            .lock()
            .unwrap()
            .session_registry
            .test_remember_waiting_screen(key.clone(), replacement.clone());
        exists
    });
    assert!(reaped.is_empty());
    let app = state.lock().unwrap();
    assert!(
        app.session_registry
            .test_waiting_screens()
            .any(|screen| screen.same_screen(&replacement)),
        "the new waiting generation remains registered"
    );
}

#[test]
fn background_cleanup_skips_surviving_roots() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let key = TabKey {
        root: repo.clone(),
        tab_id: "term-1".into(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(&repo, TabRole::Shell, GatedHarness::new()),
    );
    assert!(AppState::reap_orphaned_terminals(&state).is_empty());
    assert!(state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .is_some());
}

#[tokio::test]
async fn successful_merge_prune_closes_only_its_roots_shell_and_waiting_screen() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let (owner, root) = standalone_checkout(&state, &repo, dir.path(), "prune");
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, &owner);
    let surviving_key = derived_agent_key(&repo, "surviving");
    let mut app = state.lock().unwrap();
    app.session_registry.test_insert_tab(
        shell_key.clone(),
        gated_tab(&root, TabRole::Shell, GatedHarness::new()),
    );
    app.session_registry.test_remember_waiting_screen(
        waiting_key.clone(),
        ScreenHandle::new(&waiting_key.tab_id, 80, 24),
    );
    app.session_registry.test_remember_waiting_screen(
        surviving_key.clone(),
        ScreenHandle::new(&surviving_key.tab_id, 80, 24),
    );
    let worktree = app.runs[&owner].worktree.clone();
    let project_id = app.project_at(0).id.clone();
    app.apply_merge_cleanup("run-prune", &project_id, &worktree, MergeCleanup::Prune);
    assert!(!root.exists());
    assert!(app.session_registry.test_tab(&shell_key).is_none());
    assert!(!app
        .session_registry
        .waiting_screen_keys()
        .contains(&waiting_key));
    assert!(app
        .session_registry
        .waiting_screen_keys()
        .contains(&surviving_key));
}

#[tokio::test]
async fn successful_workspace_delete_closes_its_waiting_screen_before_reply() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let created = call(
        &handler,
        "workspace.create",
        json!({
            "project_id": project_id, "name": "remove-me", "isolation": "worktree",
        }),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let workspace_id = created["result"]["workspace_id"].as_str().unwrap();
    let root = PathBuf::from(created["result"]["root"].as_str().unwrap());
    let key = derived_agent_key(&root, "waiting");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_remember_waiting_screen(key.clone(), ScreenHandle::new(&key.tab_id, 80, 24));
    let deleted = call(
        &handler,
        "workspace.delete",
        json!({"workspace_id": workspace_id}),
    );
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert!(!root.exists());
    assert!(!state
        .lock()
        .unwrap()
        .session_registry
        .waiting_screen_keys()
        .contains(&key));
}

#[cfg(unix)]
#[tokio::test]
async fn abandon_preserves_the_canonical_removed_root_through_a_symlinked_ancestor() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (owner, root) = standalone_checkout(&state, &repo, dir.path(), "abandon");
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(root.parent().unwrap(), &alias).unwrap();
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, &owner);
    {
        let mut app = state.lock().unwrap();
        app.runs.get_mut(&owner).unwrap().worktree.path = alias.join(root.file_name().unwrap());
        app.session_registry.test_insert_tab(
            shell_key.clone(),
            gated_tab(&root, TabRole::Shell, GatedHarness::new()),
        );
        app.session_registry.test_remember_waiting_screen(
            waiting_key.clone(),
            ScreenHandle::new(&waiting_key.tab_id, 80, 24),
        );
    }
    let abandoned = call(&handler, "run.abandon", json!({"run_id": owner}));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert!(!root.exists());
    let app = state.lock().unwrap();
    assert!(app.session_registry.test_tab(&shell_key).is_none());
    assert!(!app
        .session_registry
        .waiting_screen_keys()
        .contains(&waiting_key));
}

fn runtime_cleanup_publishes_without_a_successful_lineage_save(fail_save: bool) {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let (key, _) = insert_live_run(&state, &repo, dir.path().join("side"), "run-b");
    let (before, roster) = {
        let mut app = state.lock().unwrap();
        if fail_save {
            app.store.as_ref().unwrap().fail_next_write();
        } else {
            app.session_registry
                .test_tab_mut(&key)
                .unwrap()
                .session_instance = None;
        }
        (
            app.read_view.load().get("agent.list", "run-b").cloned(),
            app.runs["run-b"].agents.clone(),
        )
    };
    std::fs::remove_dir_all(&key.root).unwrap();
    assert_eq!(
        AppState::reap_orphaned_terminals(&state),
        vec![key.tab_id.clone()]
    );
    let app = state.lock().unwrap();
    assert!(app.session_registry.test_tab(&key).is_none());
    assert_eq!(
        app.runs["run-b"].agents, roster,
        "a failed lineage save or absent lineage must not publish a candidate roster"
    );
    assert_ne!(
        app.read_view.load().get("agent.list", "run-b").cloned(),
        before,
        "the actual retired registration must be published before cleanup returns"
    );
}

#[tokio::test]
async fn background_retirement_without_lineage_publishes_runtime_state() {
    runtime_cleanup_publishes_without_a_successful_lineage_save(false);
}

#[tokio::test]
async fn background_retirement_with_a_failed_lineage_save_publishes_committed_runtime_state() {
    runtime_cleanup_publishes_without_a_successful_lineage_save(true);
}
