use super::*;

#[test]
fn failed_plan_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let id = file_legacy_task(&mut app, "committed plan");
    app.initialize_snapshot_projection();
    let snapshot = app.read_view.load().get("agent.list", &id).unwrap().clone();
    let before = app.plans[&id].agents.clone();
    let clock = app.board.attention().clock(&id);
    let mut candidate = app.take_plan(&id).unwrap();
    candidate.plan.goal = "failed plan".into();
    candidate.plan.state = crate::plan::PlanState::Failed;
    candidate.last_summary = Some("failed summary".into());
    candidate
        .agents
        .sole_thread_mut()
        .post_agent("failed words", None, "2099-01-01T00:00:00Z");
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_plan_mutation(id.clone(), candidate).is_err());
    assert_eq!(app.plans[&id].plan.goal, "committed plan");
    assert_eq!(app.plans[&id].agents, before);
    assert_eq!(app.board.attention().clock(&id), clock);
    file_legacy_task(&mut app, "unrelated success");
    app.publish_read_metadata();
    assert_eq!(app.read_view.load().get("agent.list", &id), Some(&snapshot));
    let candidate = app.take_plan(&id).unwrap();
    app.finish_plan_mutation(id.clone(), candidate).unwrap();
    let records = app.store.as_ref().unwrap().load_all_plans().unwrap();
    let saved = records.iter().find(|record| record.id == id).unwrap();
    assert_eq!(saved.goal, "committed plan");
    assert_eq!(saved.last_summary, None);
    assert_eq!(json!(saved.roster()), json!(before));
}

#[test]
fn failed_run_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let id = adopted_run(&mut app, &repo, dir.path(), "committed-run");
    let goal = app.runs[&id].run.goal.clone();
    app.initialize_snapshot_projection();
    let snapshot = app.read_view.load().get("agent.list", &id).unwrap().clone();
    let before = app.runs[&id].agents.clone();
    let clock = app.board.attention().clock(&id);
    let mut candidate = app.take_run(&id).unwrap();
    candidate.run.goal = "failed run".into();
    candidate.run.state = RunState::Failed;
    candidate.last_summary = Some("failed summary".into());
    candidate
        .agents
        .sole_thread_mut()
        .post_agent("failed words", None, "2099-01-01T00:00:00Z");
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation(id.clone(), candidate).is_err());
    assert_eq!(app.runs[&id].run.goal, goal);
    assert_eq!(app.runs[&id].agents, before);
    assert_eq!(app.board.attention().clock(&id), clock);
    file_legacy_task(&mut app, "unrelated success");
    app.publish_read_metadata();
    assert_eq!(app.read_view.load().get("agent.list", &id), Some(&snapshot));
    let candidate = app.take_run(&id).unwrap();
    app.finish_run_mutation(id.clone(), candidate).unwrap();
    let records = app.store.as_ref().unwrap().load_all_runs().unwrap();
    let saved = records.iter().find(|record| record.id == id).unwrap();
    assert_eq!(saved.goal, goal);
    assert_eq!(saved.last_summary, None);
    assert_eq!(json!(saved.roster()), json!(before));
}

#[tokio::test]
async fn failed_finishers_emit_no_change_or_news() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (plan, run) = {
        let mut app = state.lock().unwrap();
        let plan = file_legacy_task(&mut app, "committed plan");
        let run = adopted_run(&mut app, &repo, dir.path(), "committed-run");
        (plan, run)
    };
    push::settled_pushes(&mut rx, &key).await;
    {
        let mut app = state.lock().unwrap();
        let mut active = app.take_plan(&plan).unwrap();
        active.agents.sole_thread_mut().post_agent(
            "failed plan news",
            None,
            "2099-01-01T00:00:00Z",
        );
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_plan_mutation(plan.clone(), active).is_err());
        let mut active = app.take_run(&run).unwrap();
        active
            .agents
            .sole_thread_mut()
            .post_agent("failed run news", None, "2099-01-01T00:00:00Z");
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_run_mutation(run.clone(), active).is_err());
    }
    let pushed = push::settled_pushes(&mut rx, &key).await;
    assert!(push::changed_entities(&pushed).is_empty(), "{pushed:?}");
    {
        let mut app = state.lock().unwrap();
        file_legacy_task(&mut app, "unrelated success");
        app.note_entity_changed(&plan);
        app.note_entity_changed(&run);
    }
    let refreshed = push::settled_pushes(&mut rx, &key).await;
    let changes = push::changed_entities(&refreshed);
    assert!(
        changes.contains(&plan) && changes.contains(&run),
        "{refreshed:?}"
    );
    let serialized = serde_json::to_string(&refreshed).unwrap();
    assert!(!serialized.contains("failed plan news"));
    assert!(!serialized.contains("failed run news"));
}

#[test]
fn failed_user_activity_persistence_keeps_its_best_effort_observation() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let saved = app.store.as_ref().unwrap().load_user_session().unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("store/build.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_user_session BEFORE INSERT ON meta WHEN NEW.key = 'user_session' BEGIN SELECT RAISE(ABORT, 'injected user session failure'); END").unwrap();
    app.note_user_activity(42);
    assert_eq!(app.user_session().last_activity_ms, Some(42));
    assert_eq!(
        app.store.as_ref().unwrap().load_user_session().unwrap(),
        saved
    );
    connection
        .execute_batch("DROP TRIGGER refuse_user_session")
        .unwrap();
    app.note_user_activity(43);
    assert_eq!(
        app.store.as_ref().unwrap().load_user_session().unwrap(),
        Some(app.user_session())
    );
}

#[test]
fn failed_agent_remove_keeps_committed_roster_and_cursors_but_retires_delivery() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let owner = adopted_run(&mut app, &repo, dir.path(), "failed-removal");
    let added = app.agent_add(&json!({"entity_id": owner})).unwrap();
    let agent = added["agent"]["id"].as_str().unwrap();
    let posted = app.handle(req(
        "thread.post",
        json!({"entity_id": owner, "agent_id": agent, "body": "queued"}),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    app.handle(req("entity.seen", json!({"entity_id": owner})));
    assert!(app.board.attention().has_agent_cursor(&owner, agent));
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app
        .agent_remove(&json!({"entity_id": owner, "agent_id": agent}))
        .is_err());
    assert!(app.runs[&owner].agents.by_id(agent).is_some());
    assert!(app.board.attention().has_agent_cursor(&owner, agent));
    assert!(app
        .delivery_queue
        .queued()
        .all(|turn| turn.agent_id != agent));
    let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
    assert!(saved
        .iter()
        .find(|record| record.id == owner)
        .unwrap()
        .roster()
        .by_id(agent)
        .is_some());
}

#[test]
fn retry_after_bounded_rollback_keeps_older_committed_history() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let owner = adopted_run(&mut app, &repo, dir.path(), "bounded-rollback");
    let mut active = app.take_run(&owner).unwrap();
    let count = crate::store::RESIDENT_CONVERSATION_TAIL + 5;
    for index in 0..count {
        active.agents.sole_thread_mut().post_agent(
            format!("committed {index}"),
            None,
            "2099-01-01T00:00:00Z",
        );
    }
    let agent = active.agents.iter().next().unwrap().id.clone();
    app.finish_run_mutation(owner.clone(), active).unwrap();
    let mut active = app.take_run(&owner).unwrap();
    active
        .agents
        .sole_thread_mut()
        .post_agent("failed words", None, "2099-01-01T00:00:01Z");
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation(owner.clone(), active).is_err());
    let mut restored = app.take_run(&owner).unwrap();
    let thread = restored.agents.sole_thread();
    assert_eq!(thread.items.len(), crate::store::RESIDENT_CONVERSATION_TAIL);
    assert_eq!(thread.total_item_count(), count as u64);
    restored.last_summary = Some("unrelated successful metadata".into());
    app.finish_run_mutation(owner, restored).unwrap();
    let stored = app.store.as_ref().unwrap().thread_items(&agent).unwrap();
    assert_eq!(stored.len(), count);
    assert!(!serde_json::to_string(&stored)
        .unwrap()
        .contains("failed words"));
}

#[tokio::test]
async fn borrowed_owner_defers_committed_events_until_failed_peer_is_restored() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (plan, run, snapshots) = {
        let mut app = state.lock().unwrap();
        let plan = adopted_run(&mut app, &repo, dir.path(), "committed-peer");
        let run = adopted_run(&mut app, &repo, dir.path(), "borrowed-peer");
        (plan, run, app.snapshot_service())
    };
    push::settled_pushes(&mut rx, &key).await;
    let before = snapshots.call(req("board.list", json!({})));
    let (failed, original_run_goal) = {
        let mut app = state.lock().unwrap();
        let mut first = app.take_run(&plan).unwrap();
        let mut second = app.take_run(&run).unwrap();
        let original_run_goal = second.run.goal.clone();
        first.run.goal = "committed first owner".into();
        first.agents.sole_thread_mut().post_agent(
            "committed first news",
            None,
            "2099-01-01T00:00:00Z",
        );
        second.run.goal = "failed second owner".into();
        second.agents.sole_thread_mut().post_agent(
            "failed second news",
            None,
            "2099-01-01T00:00:00Z",
        );
        app.finish_run_mutation(plan.clone(), first).unwrap();
        (second, original_run_goal)
    };
    assert_eq!(snapshots.call(req("board.list", json!({}))), before);
    let premature = push::settled_pushes(&mut rx, &key).await;
    assert!(
        push::changed_entities(&premature).is_empty(),
        "{premature:?}"
    );
    {
        let mut app = state.lock().unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_run_mutation(run.clone(), failed).is_err());
        assert_eq!(app.runs[&run].run.goal, original_run_goal);
        let plans = app.store.as_ref().unwrap().load_all_runs().unwrap();
        assert_eq!(
            plans.iter().find(|record| record.id == plan).unwrap().goal,
            "committed first owner"
        );
        let runs = app.store.as_ref().unwrap().load_all_runs().unwrap();
        assert_eq!(
            runs.iter().find(|record| record.id == run).unwrap().goal,
            original_run_goal
        );
    }
    let after = snapshots.call(req("board.list", json!({})));
    let serialized = serde_json::to_string(&after).unwrap();
    assert!(serialized.contains("committed first owner"), "{after}");
    assert!(!serialized.contains("failed second owner"), "{after}");
    let pushed = push::settled_pushes(&mut rx, &key).await;
    let changed = push::changed_entities(&pushed);
    assert!(changed.contains(&plan), "{pushed:?}");
    assert!(!changed.contains(&run), "{pushed:?}");
    assert!(!serde_json::to_string(&pushed)
        .unwrap()
        .contains("failed second news"));
}
