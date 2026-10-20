use super::delivery::{open_a_turn, primary_thread};
use super::frame_locks::{gated_tab, GatedHarness};
use super::run_agents::{insert_live_run, insert_run_without_agent};
use super::*;

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
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-abandon");
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, "run-abandon");
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

    let reply = call(&handler, "run.abandon", json!({"run_id": "run-abandon"}));
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
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-abandon");
    let shell_key = TabKey {
        root: root.clone(),
        tab_id: "term-1".into(),
    };
    let waiting_key = derived_agent_key(&root, "run-abandon");
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
        app.run_abandon(&json!({"run_id": "run-abandon"})).unwrap();
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
