use super::*;

mod attention_scope;

fn counted_post(state: &Arc<Mutex<AppState>>, params: Value) -> (Value, Value) {
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let samples = clock.capture_phases();
    let timer = clock.frame("thread.post");
    let answer = timer.lock(state).handle(req("thread.post", params));
    drop(timer);
    (answer, samples.snapshot())
}

#[test]
fn same_owner_post_saves_once_and_keeps_history_attention_receipt_and_delivery() {
    for managed in [false, true] {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let owner = adopted_run(&mut app, &repo, dir.path(), "single-post-save");
        let agent = primary_agent_id(&app, &owner);
        let mut active = app.take_run(&owner).unwrap();
        active.run.state = RunState::Review;
        app.finish_run_mutation(owner.clone(), active).unwrap();
        app.initialize_snapshot_projection();
        let mut params = json!({"entity_id":owner, "agent_id":agent, "conversation_id":agent, "body":"one committed post"});
        if managed {
            params["operation_id"] = json!("single-post-operation");
        }
        let state = Arc::new(Mutex::new(app));
        let (answer, samples) = counted_post(&state, params.clone());
        assert_eq!(answer["ok"], true, "{answer}");
        {
            let mut app = state.lock().unwrap();
            let current = app.run_view(
                &owner,
                &app.runs[&owner],
                crate::thread::ThreadDetail::Full,
                crate::app::DigestScope::Detail,
            );
            for field in [
                "state",
                "thread",
                "agents",
                "attention",
                "unread",
                "unread_count",
                "updated_at",
            ] {
                assert_eq!(answer["result"][field], current[field], "{field}: {answer}");
            }
            assert_eq!(
                app.delivery_queue
                    .queued()
                    .filter(|turn| turn.owner == owner)
                    .count(),
                1
            );
            let stored = app.store.as_ref().unwrap().thread_items(&agent).unwrap();
            assert_eq!(stored.iter().filter(|item| matches!(item, crate::thread::ThreadItem::Message(message) if message.body == "one committed post")).count(), 1);
            if managed {
                let receipt = app
                    .operation_receipt("single-post-operation")
                    .unwrap()
                    .unwrap();
                assert_eq!(receipt.conversation_id, agent);
                assert_eq!(answer["result"]["operation_status"], "queued");
                let retry = app.handle(req("thread.post", params));
                assert_eq!(retry["ok"], true, "{retry}");
                assert_eq!(
                    app.operation_receipt("single-post-operation").unwrap(),
                    Some(receipt)
                );
                assert_eq!(
                    app.delivery_queue
                        .queued()
                        .filter(|turn| turn.owner == owner)
                        .count(),
                    1
                );
            }
        }
        assert_eq!(
            samples["thread.post"]["entered.finisher.save"]["count"], 1,
            "normal post repeated its committed owner save: {samples}"
        );
    }
}

#[test]
fn parked_owner_post_keeps_its_separate_resume_save() {
    for parked in [
        RunState::Blocked,
        RunState::Failed,
        RunState::IdleUnreported,
    ] {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let owner = adopted_run(&mut app, &repo, dir.path(), "resume-post-save");
        let mut active = app.take_run(&owner).unwrap();
        active.run.state = parked;
        app.finish_run_mutation(owner.clone(), active).unwrap();
        app.initialize_snapshot_projection();
        let state = Arc::new(Mutex::new(app));
        let (answer, samples) = counted_post(
            &state,
            json!({"entity_id":owner, "body":"resume committed work"}),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"]["state"], "building");
        assert_eq!(
            state.lock().unwrap().runs[&owner].run.state,
            RunState::Building
        );
        assert_eq!(
            samples["thread.post"]["entered.finisher.save"]["count"], 2,
            "the resume still changes canonical run state: {samples}"
        );
    }
}

#[test]
fn alias_owner_post_keeps_both_canonical_and_addressed_owner_saves() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let canonical = adopted_run(&mut app, &repo, dir.path(), "canonical-post-save");
    let canonical_agent = primary_agent_id(&app, &canonical);
    let alias = adopted_run(&mut app, &repo, dir.path(), "alias-post-save");
    let alias_agent = primary_agent_id(&app, &alias);
    let mut active = app.take_run(&alias).unwrap();
    active.run.state = RunState::Review;
    active
        .agents
        .by_id_mut(&alias_agent)
        .unwrap()
        .bind_conversation(&canonical_agent);
    app.finish_run_mutation(alias.clone(), active).unwrap();
    app.initialize_snapshot_projection();
    let state = Arc::new(Mutex::new(app));
    let (answer, samples) = counted_post(
        &state,
        json!({"entity_id":alias, "agent_id":alias_agent, "conversation_id":canonical_agent, "operation_id":"alias-post-operation", "body":"canonical history from alias"}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let app = state.lock().unwrap();
    assert!(app.agent_conversation(&canonical, Some(&canonical_agent)).unwrap().items.iter().any(|item| matches!(item, crate::thread::ThreadItem::Message(message) if message.body == "canonical history from alias")));
    let receipt = app
        .operation_receipt("alias-post-operation")
        .unwrap()
        .unwrap();
    assert_eq!(receipt.conversation_id, canonical_agent);
    assert_eq!(receipt.entity_id, alias);
    assert!(app.delivery_queue.queued().any(|turn| turn.owner == alias
        && turn.agent_id == alias_agent
        && turn.conversation_id == canonical_agent));
    assert_eq!(
        samples["thread.post"]["entered.finisher.save"]["count"], 2,
        "an alias owner retains its own settlement: {samples}"
    );
}
