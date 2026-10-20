use super::*;

#[test]
fn reset_plan_checkpoint_survives_failed_save_publication_and_retry() {
    reset_checkpoint_round_trip(false, false);
}

#[test]
fn reset_run_checkpoint_survives_failed_save_publication_and_retry() {
    reset_checkpoint_round_trip(true, false);
}

#[test]
fn reset_shared_conversation_checkpoints_its_canonical_plan_owner() {
    reset_checkpoint_round_trip(false, true);
}

#[test]
fn reset_shared_conversation_checkpoints_its_alias_run_owner() {
    reset_checkpoint_round_trip(true, true);
}

fn reset_checkpoint_round_trip(fail_run: bool, shared_conversation: bool) {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let plan = file_legacy_task(&mut app, "reset checkpoint plan");
    let project = app.project_of(&plan).unwrap();
    let orch = app.orch_for(&project).unwrap().clone();
    let workspace = orch
        .prepare_plan_workspace(&plan, app.store.as_ref().unwrap())
        .unwrap();
    let mut active = app.take_plan(&plan).unwrap();
    orch.open_plan_drafting(&mut active, workspace).unwrap();
    app.finish_plan_mutation(plan.clone(), active).unwrap();
    let run = adopted_run(&mut app, &repo, dir.path(), "reset-checkpoint-run");
    let plan_agent = primary_agent_id(&app, &plan);
    let run_agent = primary_agent_id(&app, &run);
    if shared_conversation {
        app.edit_agent_record("test", &run, &run_agent, |agent| {
            agent.bind_conversation(&plan_agent);
        });
    }
    let owners = if shared_conversation {
        vec![plan.clone(), run.clone()]
    } else if fail_run {
        vec![run.clone()]
    } else {
        vec![plan.clone()]
    };
    for owner in &owners {
        let agent_id = primary_agent_id(&app, owner);
        app.edit_agent_record("test", owner, &agent_id, |agent| {
            // No native session lineage: cleanup stays inside the temp fixture.
            agent.resume_session_id = Some("erased-resume-session".into());
            agent.topic = Some("erased topic".into());
            agent.last_context_tokens = Some(1200);
            agent.session_cache_read_tokens = Some(300);
        });
        if owner == &plan {
            let mut active = app.take_plan(owner).unwrap();
            active.last_summary = Some("erased plan summary".into());
            active.last_error = Some("erased plan error".into());
            app.finish_plan_mutation(owner.clone(), active).unwrap();
        } else {
            let mut active = app.take_run(owner).unwrap();
            active.last_summary = Some("erased run summary".into());
            active.last_error = Some("erased run error".into());
            app.finish_run_mutation(owner.clone(), active).unwrap();
        }
    }
    let reset_owner = if shared_conversation || fail_run {
        &run
    } else {
        &plan
    };
    let selected_id = primary_agent_id(&app, reset_owner);
    app.edit_agent_conversation(reset_owner, &selected_id, |thread, _| {
        thread.post_agent("erased checkpoint history", None, "2099-01-01T00:00:00Z");
        Ok(())
    })
    .unwrap();
    let selected = app.entity_agents(reset_owner).unwrap().primary().unwrap();
    let old_thread = app.agent_conversation(reset_owner, None).unwrap();
    let params = json!({
        "project_id": app.projects.project_id_of(reset_owner).unwrap(),
        "entity_id": reset_owner,
        "agent_id": selected.id,
        "conversation_id": selected.conversation_id(),
        "expected_thread_id": old_thread.id,
    });
    let old_generation = old_thread.generation_revision;
    let app = app.shared();
    let handler = AppState::handler(Arc::clone(&app));
    let sender = SessionSender::detached("reset-checkpoint");
    let reset = handler.call(sender.clone(), req("conversation.reset", params));
    assert_eq!(reset["ok"], true, "{reset:?}");
    assert_eq!(
        reset["result"]["thread"]["thread_generation_revision"],
        old_generation + 1,
        "{reset:?}"
    );

    let observe = || observe_reset_checkpoint(&app, &handler, &sender, &owners);
    let expected = observe();
    for row in expected.as_array().unwrap() {
        assert_eq!(row["memory"]["resume_session_id"], Value::Null);
        assert_eq!(row["memory"]["items"], json!([]));
        assert_eq!(row["sql_items"], 0);
        assert_eq!(row["memory_summary"], json!([null, null]));
        assert_eq!(row["sql_summary"], json!([null, null]));
    }
    let failed_owner = if fail_run { &run } else { &plan };
    {
        let mut state = app.lock().unwrap();
        save_checkpoint_owner(&mut state, failed_owner, fail_run, true);
    }
    let after_failure = observe();
    {
        let mut state = app.lock().unwrap();
        file_legacy_task(&mut state, "unrelated publication after reset");
        state.publish_read_metadata();
    }
    let after_unrelated = observe();
    {
        let mut state = app.lock().unwrap();
        save_checkpoint_owner(&mut state, failed_owner, fail_run, false);
    }
    let after_retry = observe();
    // Run the whole sequence before asserting so RED also demonstrates whether
    // a successful retry resurrects the deleted generation in durable storage.
    assert_eq!(
        json!({
            "failed_save": after_failure,
            "unrelated_publication": after_unrelated,
            "successful_retry": after_retry,
        }),
        json!({
            "failed_save": expected,
            "unrelated_publication": expected,
            "successful_retry": expected,
        })
    );
}

fn observe_reset_checkpoint(
    app: &Arc<Mutex<AppState>>,
    handler: &FrameHandler,
    sender: &SessionSender,
    owners: &[String],
) -> Value {
    let (mut rows, facts_handle) = {
        let state = app.lock().unwrap();
        let store = state.store.as_ref().unwrap();
        let plans = store.load_all_plans().unwrap();
        let runs = store.load_all_runs().unwrap();
        let rows = owners
            .iter()
            .map(|owner| {
                let live = state.entity_agents(owner).unwrap().primary().unwrap();
                let saved = plans
                    .iter()
                    .find(|record| &record.id == owner)
                    .map(|record| {
                        record
                            .agents
                            .iter()
                            .find(|agent| agent.id == live.id)
                            .unwrap()
                    })
                    .or_else(|| {
                        runs.iter()
                            .find(|record| &record.id == owner)
                            .map(|record| {
                                record
                                    .agents
                                    .iter()
                                    .find(|agent| agent.id == live.id)
                                    .unwrap()
                            })
                    })
                    .unwrap();
                let memory_summary = state
                    .plans
                    .get(owner)
                    .map(|active| json!([active.last_summary, active.last_error]))
                    .or_else(|| {
                        state
                            .runs
                            .get(owner)
                            .map(|active| json!([active.last_summary, active.last_error]))
                    });
                let sql_summary = plans
                    .iter()
                    .find(|record| &record.id == owner)
                    .map(|record| json!([record.last_summary, record.last_error]))
                    .or_else(|| {
                        runs.iter()
                            .find(|record| &record.id == owner)
                            .map(|record| json!([record.last_summary, record.last_error]))
                    });
                json!({
                    "owner": owner,
                    "memory": checkpoint_agent(live),
                    "sql": checkpoint_agent(saved),
                    "sql_items": store.thread_item_count(live.conversation_id()).unwrap(),
                    "memory_summary": memory_summary,
                    "sql_summary": sql_summary,
                })
            })
            .collect::<Vec<_>>();
        (rows, Arc::clone(&state.facts_handle))
    };
    for (owner, row) in owners.iter().zip(&mut rows) {
        let agents = handler.call(
            sender.clone(),
            req("agent.list", json!({"entity_id": owner})),
        );
        assert_eq!(agents["ok"], true, "{agents:?}");
        row["routed_agents"] = agents["result"]["agents"].clone();
        let page = handler.call(
            sender.clone(),
            req("thread.page", json!({"entity_id": owner})),
        );
        assert_eq!(page["ok"], true, "{page:?}");
        row["routed_page"] = page["result"].clone();
        let requests = [crate::changes::FactsRequest {
            entity_id: owner.clone(),
            thread: true,
            thread_after: vec![(primary_agent_id(&app.lock().unwrap(), owner), 0)],
            ..Default::default()
        }];
        let facts = crate::app::facts::entity_facts(&facts_handle, &requests);
        row["push_threads"] = json!(facts[0]
            .threads
            .iter()
            .map(|tip| json!({
                "agent_id": tip.agent_id,
                "conversation_id": tip.conversation_id,
                "thread_id": tip.thread_id,
                "generation": tip.thread_generation_revision,
                "last_sequence": tip.last_sequence,
                "items": tip.items,
            }))
            .collect::<Vec<_>>());
    }
    json!(rows)
}

fn checkpoint_agent(agent: &crate::agent::Agent) -> Value {
    json!({
        "id": agent.id,
        "conversation_id": agent.conversation_id(),
        "thread_id": agent.thread.id,
        "generation": agent.thread.generation_revision,
        "items": agent.thread.items,
        "sessions": agent.thread.sessions,
        "revisions": agent.thread.revisions,
        "resume_session_id": agent.resume_session_id,
        "topic": agent.topic,
        "last_context_tokens": agent.last_context_tokens,
        "session_cache_read_tokens": agent.session_cache_read_tokens,
    })
}

fn save_checkpoint_owner(state: &mut AppState, owner: &str, run: bool, fail: bool) {
    let result = if run {
        let mut active = state.take_run(owner).unwrap();
        if fail {
            active.agents.primary_mut().unwrap().topic = Some("failed candidate".into());
            state.store.as_ref().unwrap().fail_next_write();
        }
        state.finish_run_mutation(owner.to_owned(), active)
    } else {
        let mut active = state.take_plan(owner).unwrap();
        if fail {
            active.agents.primary_mut().unwrap().topic = Some("failed candidate".into());
            state.store.as_ref().unwrap().fail_next_write();
        }
        state.finish_plan_mutation(owner.to_owned(), active)
    };
    assert_eq!(result.is_err(), fail, "{result:?}");
}
