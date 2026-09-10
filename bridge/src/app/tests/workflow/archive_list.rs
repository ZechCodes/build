use super::*;

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: archived_list_gathers_finished_work_across_every_project is at 19, threshold 15 — bring it under, then remove
fn archived_list_gathers_finished_work_across_every_project() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let (issue_id, run_id) = planned_run_in_review(&mut state, "finished work");
    let branch = state.runs[&run_id].worktree.branch();
    let worktree = state.runs[&run_id].worktree.path.clone();
    git_in(&worktree, &["push", "-u", "origin", &branch]);
    let finished = state.handle(req(
        "branch.finish",
        json!({ "project_id": project_id, "branch": branch, "action": "delete" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    // Deleting the branch left the issue in the inbox; the user marks the
    // issue done separately, and that is what files it away.
    let archived_issue = state.handle(req("issue.archive", json!({ "issue_id": issue_id })));
    assert_eq!(archived_issue["ok"], true, "{archived_issue:?}");

    // A second project's bare checkout, finished on its own: no run behind
    // it, so only its archived-worktree record remembers it.
    let (_other_dir, other_repo) = init_repo();
    let other_project = state.add_project(other_repo.clone(), "main".into());
    add_external_worktree(&other_repo, dir.path(), "loose", "loose");
    let loose_id = external_id(&mut state, &other_project, Some("loose"));
    let loose_finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": other_project, "worktree_id": loose_id, "action": "cleanup" }),
    ));
    assert_eq!(loose_finished["ok"], true, "{loose_finished:?}");

    let archived = state.handle(req("archived.list", json!({})));
    let items = archived["result"]["items"].as_array().unwrap();
    let branches: Vec<&Value> = items.iter().filter(|row| row["kind"] == "branch").collect();
    let issues: Vec<&Value> = items.iter().filter(|row| row["kind"] == "issue").collect();

    assert_eq!(issues.len(), 1, "{items:?}");
    assert_eq!(issues[0]["issue_id"], issue_id, "{items:?}");
    assert_eq!(issues[0]["project_id"], project_id, "{items:?}");
    assert!(issues[0]["finished_at"].as_str().is_some(), "{items:?}");

    assert_eq!(
        branches.len(),
        2,
        "the finished run and its archived worktree are one branch row: {items:?}"
    );
    let implemented = branches
        .iter()
        .find(|row| row["branch"] == json!(branch.clone()))
        .unwrap_or_else(|| panic!("no row for {branch}: {items:?}"));
    assert_eq!(implemented["run_id"], run_id, "{implemented:?}");
    assert_eq!(implemented["issue_id"], issue_id, "{implemented:?}");
    assert_eq!(implemented["project_id"], project_id, "{implemented:?}");
    assert_eq!(implemented["state"], "archived", "{implemented:?}");
    assert_eq!(implemented["action"], "delete", "{implemented:?}");
    assert!(
        implemented["finished_at"].as_str().is_some(),
        "{implemented:?}"
    );

    let loose = branches
        .iter()
        .find(|row| row["branch"] == "loose")
        .unwrap_or_else(|| panic!("no loose row: {items:?}"));
    assert_eq!(loose["project_id"], other_project, "{loose:?}");
    assert_eq!(loose["worktree_id"], loose_id, "{loose:?}");
    assert_eq!(loose["run_id"], Value::Null, "{loose:?}");
    assert_eq!(loose["action"], "cleanup", "{loose:?}");
}

#[test]
fn archived_list_leaves_live_work_alone_and_puts_the_newest_first() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (live_issue, _live_run) = planned_run_in_review(&mut state, "still going");

    for (name, when) in [
        ("older", "2026-01-01T00:00:00Z"),
        ("newer", "2026-06-01T00:00:00Z"),
    ] {
        let issue = state.handle(req("plan.create", json!({ "goal": name })));
        let issue_id = plan_id_of(&issue);
        state.plans.get_mut(&issue_id).unwrap().plan.archived_at = Some(when.to_string());
    }

    let archived = state.handle(req("archived.list", json!({})));
    let items = archived["result"]["items"].as_array().unwrap();
    assert!(
        !items.iter().any(|row| row["issue_id"] == json!(live_issue)),
        "live work is not archive: {items:?}"
    );
    let titles: Vec<&str> = items
        .iter()
        .map(|row| row["title"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(titles, vec!["newer", "older"], "{items:?}");
    assert_eq!(items[0]["project_id"], project_id, "{items:?}");
}

#[test]
fn archive_list_is_scoped_by_project_canonical_path() {
    let (dir, repo) = init_repo();
    let (_other_dir, other_repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let first_project = state.project_at(0).id.clone();
    let second_project = state.add_project(other_repo.clone(), "main".into());
    let first_path = add_external_worktree(&repo, dir.path(), "first", "first");
    let second_path = add_external_worktree(&other_repo, dir.path(), "second", "second");
    let first_id = external_id(&mut state, &first_project, Some("first"));
    let second_id = external_id(&mut state, &second_project, Some("second"));
    assert_eq!(
        state.handle(req(
            "worktree.finish",
            json!({ "project_id": first_project, "worktree_id": first_id, "action": "cleanup" }),
        ))["ok"],
        true
    );
    assert_eq!(
        state.handle(req(
            "worktree.finish",
            json!({ "project_id": second_project, "worktree_id": second_id, "action": "cleanup" }),
        ))["ok"],
        true
    );
    assert!(!first_path.exists() && !second_path.exists());

    let first = state.handle(req("archive.list", json!({ "project_id": first_project })));
    let second = state.handle(req("archive.list", json!({ "project_id": second_project })));
    assert_eq!(first["result"]["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(second["result"]["worktrees"].as_array().unwrap().len(), 1);
    assert_ne!(
        first["result"]["worktrees"][0]["worktree_id"],
        second["result"]["worktrees"][0]["worktree_id"]
    );
}

#[test]
fn archived_worktrees_load_into_archive_list_after_restart() {
    let (dir, repo) = init_repo();
    let worktree_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        add_external_worktree(&repo, dir.path(), "durable-finish", "durable-finish");
        worktree_id = external_id(&mut state, &project_id, Some("durable-finish"));
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let reminted_project_id = reloaded.project_at(0).id.clone();
    let archive = reloaded.handle(req(
        "archive.list",
        json!({ "project_id": reminted_project_id }),
    ));
    assert!(archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .any(|worktree| worktree["worktree_id"] == worktree_id));
}

#[test]
fn restart_completes_a_durable_finish_intent_after_worktree_removal() {
    let (dir, repo) = init_repo();
    let worktree_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        add_external_worktree(
            &repo,
            dir.path(),
            "interrupted-finish",
            "interrupted-finish",
        );
        worktree_id = external_id(&mut state, &project_id, Some("interrupted-finish"));
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
    }

    // Put the record back the way an interrupted daemon would have left it:
    // the intent is durable, the worktree removal never happened.
    {
        let store = Store::new(dir.path().join("store")).expect("store opens");
        let raw = store
            .archived_worktree_json(&worktree_id)
            .expect("the finish wrote an archive record");
        let mut record: Value = serde_json::from_str(&raw).unwrap();
        record["status"] = json!("pending");
        record["archived_at"] = Value::Null;
        store.set_archived_worktree_json(&worktree_id, &record.to_string());
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let project_id = reloaded.project_at(0).id.clone();
    let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
    assert!(archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .any(|worktree| worktree["worktree_id"] == worktree_id));
    // The recovery is durable, not just in memory: a store opened fresh
    // sees the completed finish.
    let recovered: Value = serde_json::from_str(
        &Store::new(dir.path().join("store"))
            .expect("store opens")
            .archived_worktree_json(&worktree_id)
            .expect("the archive record survives"),
    )
    .unwrap();
    assert_eq!(recovered["status"], "archived");
    assert!(recovered["archived_at"].is_string());
}

#[test]
fn pushed_worktrees_load_into_archive_list_after_restart() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let worktree_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        let path = add_external_worktree(&repo, dir.path(), "durable-push", "durable-push");
        git_in(&path, &["push", "-u", "origin", "durable-push"]);
        std::fs::write(path.join("pushed.txt"), "published\n").unwrap();
        worktree_id = external_id(&mut state, &project_id, Some("durable-push"));
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let project_id = reloaded.project_at(0).id.clone();
    let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
    let record = archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["worktree_id"] == worktree_id)
        .unwrap();
    assert_eq!(record["action"], "push");
}

#[test]
fn deleted_worktrees_load_into_archive_list_after_restart() {
    let (dir, repo) = init_repo();
    let worktree_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.project_at(0).id.clone();
        let path = add_external_worktree(&repo, dir.path(), "durable-delete", "durable-delete");
        std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
        worktree_id = external_id(&mut state, &project_id, Some("durable-delete"));
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
    }

    let mut reloaded = qa_state(&repo, dir.path());
    let project_id = reloaded.project_at(0).id.clone();
    let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
    let record = archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["worktree_id"] == worktree_id)
        .unwrap();
    assert_eq!(record["action"], "delete");
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

#[tokio::test]
async fn worktree_finish_closes_and_reaps_scoped_terminals() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    app.term_shell = "/bin/bash".into();
    let project_id = app.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "terminal-finish", "terminal-finish");
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
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(!state.lock().unwrap().session_registry.contains(&term_key));
    assert!(
        process_reaped(pid),
        "scoped terminal must be killed and reaped"
    );
}
