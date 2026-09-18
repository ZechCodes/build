use super::*;

#[test]
fn archived_worktrees_load_into_archive_list_after_restart() {
    let (dir, repo) = init_repo();
    let worktree_path = dir.path().join("historical-finish");
    let record = PersistedArchivedWorktree {
        status: WorktreeFinishStatus::Archived,
        project_path: repo.display().to_string(),
        worktree_id: crate::worktree::external_worktree_id(&worktree_path),
        worktree_name: "historical-finish".to_string(),
        worktree_path: worktree_path.display().to_string(),
        branch: Some("historical-finish".to_string()),
        head_sha: git_stdout(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string(),
        upstream: None,
        unpushed: None,
        dirty_files: 0,
        uncommitted_files: 0,
        uncommitted_insertions: 0,
        uncommitted_deletions: 0,
        action: WorktreeFinishAction::Cleanup,
        archived_at: Some(now_rfc3339()),
    };
    {
        let state = qa_state(&repo, dir.path());
        state
            .require_store()
            .unwrap()
            .save_archived_worktree(&record)
            .unwrap();
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let project_id = reloaded.project_at(0).id.clone();
    let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
    let restored = archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["worktree_id"] == record.worktree_id)
        .expect("the historical archive record remains readable");
    assert_eq!(restored["action"], "cleanup");
}

#[test]
fn external_worktree_json_sets_can_finish_for_an_idle_agent_tab() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(&repo, dir.path(), "idle-agent", "idle-agent");
    let worktree_id = external_id(&mut state, &project_id, Some("idle-agent"));
    let root = AppState::canonical_root(&path);
    let (tab, _rx) = Tab::spawn_agent(
        "idle-agent-owner".to_string(),
        crate::agent::derived_agent_id("idle-agent-owner"),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(80, 24),
        ),
    )
    .unwrap();
    tab.session
        .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    let key = derived_agent_key(&root, "idle-agent-owner");
    state.session_registry.test_insert_tab(key.clone(), tab);

    let board = state.handle(req("board.list", json!({})));
    let entry = board["result"]["external_worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["worktree_id"] == worktree_id)
        .unwrap();
    assert_eq!(entry["agent_working"], false, "{entry:?}");
    assert_eq!(entry["can_finish"], true, "{entry:?}");
    state
        .session_registry
        .test_remove_tab(&key)
        .unwrap()
        .session
        .end();
}

/// Done removes the workspace it finishes, and an adopted checkout is not
/// Build's to remove — so Done refuses it, the way Delete already does, and
/// the checkout and the terminal standing in it are left alone.
#[tokio::test]
async fn workspace_finish_refuses_an_adopted_checkout_and_leaves_its_terminal() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut app = qa_state(&repo, dir.path());
    app.term_shell = "/bin/bash".into();
    let project_id = app.project_at(0).id.clone();
    let checkout = add_external_worktree(&repo, dir.path(), "terminal-finish", "terminal-finish");
    let worktree_id = external_id(&mut app, &project_id, Some("terminal-finish"));
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
    let (term_key, pid) = {
        let app = state.lock().unwrap();
        let term_key = app.tab_key_of_wire_id(&term_id).unwrap();
        let pid = agent_pid(app.session_registry.test_tab(&term_key).unwrap()).unwrap();
        (term_key, pid)
    };

    let finished = handler.call(
        SessionSender::detached("s1"),
        req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ),
    );
    assert_eq!(finished["ok"], false, "{finished:?}");
    assert!(
        finished["error"]
            .as_str()
            .unwrap()
            .contains("Build cannot remove an adopted checkout"),
        "{finished:?}"
    );
    assert!(checkout.exists(), "a refused Done retains the checkout");
    assert!(
        state.lock().unwrap().session_registry.contains(&term_key),
        "the workspace terminal remains registered"
    );
    assert!(!process_reaped(pid), "the workspace terminal remains alive");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_remove_tab(&term_key)
        .unwrap()
        .session
        .end();
}
