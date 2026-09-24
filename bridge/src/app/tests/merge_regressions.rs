use super::*;

fn assert_unrelated_frame_completes(state: &Arc<Mutex<AppState>>) {
    assert!(
        state.try_lock().is_ok(),
        "off-lock work retained the app mutex"
    );
    let reply = frame_on_a_thread(state, "unrelated", "bridge.stats", json!({}))
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated frame completes while I/O is stalled");
    assert_eq!(reply["ok"], true, "{reply}");
}

/// An offer's peer work — the answer, and the cleanup of a peer that could
/// not answer — runs with the app mutex free. Held at the peer itself, where
/// the work is, since an offer never takes the mutex at all any more: the
/// peers live in a slot beside the state (`app::rtc::PeersSlot`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merged_rtc_offer_and_failure_cleanup_leave_the_mutex_free() {
    for fails in [false, true] {
        let (dir, repo) = init_repo();
        let (state, handler, factory) = signaling_fixture(&repo, dir.path());
        if fails {
            factory.fail_answers();
        }
        let answering = factory.hold_answers();
        let sender = SessionSender::detached("merge-rtc");
        let offering = tokio::task::spawn_blocking(move || {
            handler.call(sender, req("rtc.offer", offer("v=0 merge")))
        });
        answering.wait_until_answering().await;
        assert_unrelated_frame_completes(&state);
        answering.release();
        let reply = offering.await.unwrap();
        assert_eq!(reply["ok"], !fails, "{reply}");
        let app = state.lock().unwrap();
        assert_eq!(app.peers().count(), usize::from(!fails));
        assert_eq!(app.frame_clock.stats()["methods"]["rtc.offer"]["served"], 1);
    }
}

#[test]
fn merged_branch_holder_creation_and_refusal_leave_the_mutex_free() {
    for branch in ["available", "main"] {
        let (dir, repo) = init_repo();
        let git = git2::Repository::open(&repo).unwrap();
        git.branch(
            "available",
            &git.head().unwrap().peel_to_commit().unwrap(),
            false,
        )
        .unwrap();
        let mut app = qa_state(&repo, dir.path());
        let project_id = app.project_at(0).id.clone();
        let (gate, held) = OffLockGate::new();
        app.off_lock_gate = Some(gate);
        let state = app.shared();
        let creating = frame_on_a_thread(
            &state,
            "holder",
            "worktree.create",
            json!({ "project_id": project_id, "branch": branch }),
        );
        held.wait_for_arrival();
        assert_unrelated_frame_completes(&state);
        held.release();
        let reply = creating.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(reply["ok"], branch == "available", "{reply}");
        assert!(state.lock().unwrap().pending_rows.is_empty());
    }
}

#[test]
fn merged_branch_dispatch_revalidates_the_holder_snapshot_before_apply() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (gate, held) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();
    let dispatching = frame_on_a_thread(
        &state,
        "dispatch-holder",
        "branch.dispatch",
        json!({ "project_id": project_id, "branch": "topic", "instruction": "work here" }),
    );
    held.wait_for_arrival();
    assert_unrelated_frame_completes(&state);
    state.lock().unwrap().project_at_mut(0).base_branch = "changed-while-reading".into();
    held.release();
    let reply = dispatching.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("changed while Git ran"),
        "{reply}"
    );
    let app = state.lock().unwrap();
    assert!(app.runs.is_empty());
    assert!(app.pending_rows.is_empty());
}

#[test]
fn merged_branch_holder_dispatch_joins_the_run_before_the_repository_or_refuses_off_lock() {
    for fails in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let project_id = app.project_at(0).id.clone();
        let run_id = adopted_run(&mut app, &repo, dir.path(), "feature-holder");
        // A run standing in the repository, on the branch the repository is
        // also on: two holders of one branch, which is what a store written
        // before workspaces has.
        app.runs.get_mut(&run_id).unwrap().worktree.path = AppState::canonical_root(&repo);
        if fails {
            app.dispatch_fault = Some(BranchDispatchStep::Post);
        }
        let (gate, held) = OffLockGate::new();
        app.off_lock_gate = Some(gate);
        let state = app.shared();
        let dispatching = frame_on_a_thread(
            &state,
            "join-holder",
            "branch.dispatch",
            json!({ "project_id": project_id, "branch": "main", "instruction": "join here" }),
        );
        held.wait_for_arrival();
        assert_unrelated_frame_completes(&state);
        held.release();
        let reply = dispatching.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(reply["ok"], !fails, "{reply}");
        if !fails {
            assert_eq!(
                reply["result"]["run_id"], run_id,
                "the run outranks the repository holder"
            );
        }
        assert_eq!(state.lock().unwrap().runs.len(), 1);
    }
}

#[test]
fn merged_branch_holder_listing_and_git_failure_leave_the_mutex_free() {
    for fails in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let project_id = app.project_at(0).id.clone();
        let (gate, held) = OffLockGate::new();
        app.off_lock_gate = Some(gate);
        let state = app.shared();
        let listing = frame_on_a_thread(
            &state,
            "list-holder",
            "git.branches",
            json!({ "project_id": project_id }),
        );
        held.wait_for_arrival();
        assert_unrelated_frame_completes(&state);
        if fails {
            std::fs::rename(repo.join(".git"), repo.join("git-unavailable")).unwrap();
        }
        held.release();
        let reply = listing.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(reply["ok"], !fails, "{reply}");
    }
}

fn merged_provider_spec(provider: AgentProvider, root: &Path, fails: bool) -> HarnessSpec {
    if fails {
        return HarnessSpec::new("/definitely/missing/merge-test-agent");
    }
    if provider != AgentProvider::CodexAppServer {
        return HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'ready\\n'; cat");
    }
    // The existing app-server fixture protocol: initialize, initialized,
    // thread/start. No installed provider, credentials, or network is used.
    let initialized =
        json!({ "id": 1, "result": { "userAgent": "build_bridge/0.153.0 (fixture)" } });
    let thread = json!({ "id": 2, "result": {
    "thread": { "id": "merge-thread" }, "model": "gpt-5.6-sol", "reasoningEffort": "high",
    "cwd": root.display().to_string(), "approvalPolicy": "never", "sandbox": { "type": "dangerFullAccess" }
} });
    HarnessSpec::new("sh").arg("-c").arg(format!(
    "read initialize; printf '%s\\n' '{initialized}'; read initialized; read thread; printf '%s\\n' '{thread}'; cat >/dev/null"
))
}

async fn merged_provider_spawn_leaves_the_mutex_free(provider: AgentProvider, fails: bool) {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-merge-spawn");
    a_provider_running(&state, &repo, merged_provider_spec(provider, &root, fails));
    let held = spawns_parked_at(&state);
    let choice = ModelChoice {
        provider,
        model: Some("gpt-5.6-sol".into()),
        effort: Some("high".into()),
    };
    let spawning_state = Arc::clone(&state);
    let spawning = tokio::task::spawn_blocking(move || {
        ensure_agent_tab(
            &spawning_state,
            &root,
            "run-merge-spawn",
            &crate::agent::derived_agent_id("run-merge-spawn"),
            &choice,
            "start",
        )
    });
    held.wait_for_arrival();
    assert_unrelated_frame_completes(&state);
    held.release();
    let result = spawning.await.unwrap();
    assert_eq!(result.is_err(), fails, "{provider:?}: {result:?}");
    let session = {
        let app = state.lock().unwrap();
        assert!(app.session_registry.test_counts().claims == 0);
        if fails {
            assert!(app.session_registry.test_counts().tokens == 0);
        }
        let session = app
            .session_registry
            .test_tabs()
            .next()
            .map(|(_, tab)| Arc::clone(&tab.session));
        session
    };
    if let Some(session) = session {
        session.end();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merged_pi_spawns_and_failure_cleanup_leave_the_mutex_free() {
    for fails in [false, true] {
        merged_provider_spawn_leaves_the_mutex_free(AgentProvider::Pi, fails).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merged_codex_spawns_and_failure_cleanup_leave_the_mutex_free() {
    for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
        for fails in [false, true] {
            merged_provider_spawn_leaves_the_mutex_free(provider, fails).await;
        }
    }
}
