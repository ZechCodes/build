use super::*;

mod operations;
mod revisions;
mod work;

#[test]
fn store_free_thread_read_context_keeps_an_immutable_bounded_history() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    state.store = None;
    pagination::run_with_long_conversation(&mut state, "run-read", 250);
    let params = json!({ "entity_id": "run-read", "limit": 3 });
    let expected = state.thread_page(&params).unwrap();
    let context = state.thread_read_context();
    state.runs.remove("run-read");

    assert_eq!(
        context.read(None, "thread.page", &params).unwrap(),
        expected
    );
    assert_eq!(expected["items"].as_array().unwrap().len(), 3);
    assert_eq!(expected["thread_total"], 250);
    assert_eq!(
        state
            .thread_read_context()
            .read(None, "thread.page", &params),
        Err("unknown id".into())
    );
}

fn stored_read_run(state: &mut AppState, owner: &str, calls: usize) -> PersistedRun {
    let mut record = fake_run_record(owner);
    let thread = &mut record.agents[0].thread;
    thread.post_user("opening", None, "2026-10-19T20:00:00Z");
    for index in 0..calls {
        thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            "2026-10-19T20:01:00Z",
        );
    }
    thread.post_user("follow-up", None, "2026-10-19T20:02:00Z");
    state.store.as_ref().unwrap().save_run(&record).unwrap();
    state.runs.insert(
        owner.into(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    record
}

#[tokio::test]
async fn sql_thread_pages_match_memory_and_read_bounded_rows() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    stored_read_run(&mut state, "run-page", 700);
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    let cases = [
        json!({"limit": 1}),
        json!({"limit": 3}),
        json!({"before_sequence": 300, "limit": 2}),
        json!({"after_sequence": 2, "limit": 10}),
        json!({"after_sequence": 2, "newest": true, "limit": 10}),
        json!({"after_sequence": 800, "newest": true, "limit": 10}),
        json!({"activity": "newest", "limit": 3}),
        json!({"activity": "newest", "after_sequence": 100, "limit": 3}),
        json!({"activity": "newest", "after_sequence": 100, "newest": true, "limit": 3}),
    ];
    for mut params in cases {
        params["entity_id"] = json!("run-page");
        let expected = state.thread_page(&params).unwrap();
        let decoded = crate::store::items_decoded();
        let actual = reader
            .read(|tx| context.read(Some(tx), "thread.page", &params))
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected, "{params}");
        assert!(
            crate::store::items_decoded() - decoded <= 110,
            "read an unbounded history for {params}"
        );
    }
    let params = json!({"entity_id": "run-page", "from_sequence": 2, "through_sequence": 701, "before_sequence": 300, "limit": 7});
    let expected = state.thread_activity(&params).unwrap();
    let decoded = crate::store::items_decoded();
    let actual = reader
        .read(|tx| context.read(Some(tx), "thread.activity", &params))
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
    assert!(crate::store::items_decoded() - decoded <= 8);
}

#[tokio::test]
async fn sql_thread_reads_preserve_address_and_cursor_errors() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    stored_read_run(&mut state, "run-errors", 2);
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    for mut params in [
        json!({"agent_id": "missing"}),
        json!({"agent_id": ""}),
        json!({"conversation_id": "stale"}),
        json!({"thread_id": "stale"}),
        json!({"before_sequence": 1, "after_sequence": 1}),
        json!({"newest": true}),
    ] {
        params["entity_id"] = json!("run-errors");
        assert_eq!(
            reader
                .read(|tx| context.read(Some(tx), "thread.page", &params))
                .unwrap(),
            state.thread_page(&params)
        );
    }
    for (from, through) in [(4, 2), (0, 500)] {
        let params =
            json!({"entity_id": "run-errors", "from_sequence": from, "through_sequence": through});
        assert_eq!(
            reader
                .read(|tx| context.read(Some(tx), "thread.activity", &params))
                .unwrap(),
            state.thread_activity(&params)
        );
    }
}

#[tokio::test]
async fn sql_thread_reads_resolve_durable_aliases_and_exclude_retained_owners() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let canonical = stored_read_run(&mut state, "run-canonical", 4);
    let mut alias = stored_read_run(&mut state, "run-alias", 0);
    alias.agents[0].bind_conversation(&canonical.agents[0].id);
    state.store.as_ref().unwrap().save_run(&alias).unwrap();
    // An in-memory candidate that was never saved cannot redirect the read.
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    let params = json!({"entity_id": "run-alias"});
    let expected = state
        .thread_page(&json!({"entity_id": "run-canonical"}))
        .unwrap();
    let (actual, fence) = reader
        .read(|tx| context.read_scoped(Some(tx), "thread.page", &params))
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);

    state.runs.remove("run-alias");
    let current = state.thread_read_context();
    assert!(current.validate_fence(&fence).is_err());
    assert_eq!(
        reader
            .read(|tx| current.read(Some(tx), "thread.page", &params))
            .unwrap(),
        Err("unknown id".into())
    );
    // SQL still retains both records; admission also protects the alias target.
    state.runs.remove("run-canonical");
    let mut current = context.clone();
    current.forget_owner("run-canonical");
    assert!(reader
        .read(|tx| current.read(Some(tx), "thread.page", &params))
        .unwrap()
        .is_err());
}

fn read_receipt(record: &PersistedRun) -> crate::operation::OperationReceipt {
    crate::operation::OperationReceipt {
        operation_id: "operation-read".into(),
        method: crate::operation::THREAD_POST_METHOD.into(),
        entity_id: record.id.clone(),
        agent_id: record.agents[0].id.clone(),
        conversation_id: record.agents[0].id.clone(),
        choice_revision: 0,
        posted_sequence: 1,
        message_start_sequence: 1,
        status: crate::operation::OperationStatus::Queued,
        execution_error: None,
        request_hash: "read-fixture".into(),
        delivery: None,
        requested_by: None,
    }
}

#[tokio::test]
async fn sql_thread_generation_history_and_operation_share_one_snapshot() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-reset", 20);
    let store = state.store.as_ref().unwrap();
    let receipt = read_receipt(&record);
    store
        .accept_thread_post(&record.id, &record.agents, &receipt)
        .unwrap();
    let context = state.thread_read_context();
    let params = json!({"entity_id": record.id, "thread_id": record.agents[0].thread.id});
    let operation = json!({"entity_id": record.id, "operation_id": receipt.operation_id});
    let expected = state.thread_page(&params).unwrap();
    let mut reader = store.read_pool().acquire().await;
    reader
        .read(|tx| {
            tx.load_agent_record(&record.agents[0].id).unwrap().unwrap();
            let mut replacement = record.agents[0].clone();
            replacement.thread = crate::thread::Thread::for_agent(&replacement.id);
            replacement.thread.id = "thread:after-reset".into();
            replacement.thread.generation_revision = 1;
            store
                .reset_conversation(
                    &replacement.id,
                    std::slice::from_ref(&replacement),
                    &Default::default(),
                    &Default::default(),
                )
                .unwrap();
            assert_eq!(
                context.read(Some(tx), "thread.page", &params).unwrap(),
                expected
            );
            assert_eq!(
                context
                    .read(Some(tx), "thread.operation", &operation)
                    .unwrap(),
                receipt.wire_value()
            );
        })
        .unwrap();
    reader
        .read(|tx| {
            assert!(context
                .read(Some(tx), "thread.page", &params)
                .unwrap_err()
                .starts_with("stale thread_id"));
            let page = context
                .read(Some(tx), "thread.page", &json!({"entity_id": record.id}))
                .unwrap();
            assert_eq!(page["thread_total"], 0);
            assert_eq!(page["thread_generation_revision"], 1);
            assert!(page["items"].as_array().unwrap().is_empty());
            assert_eq!(
                context.read(Some(tx), "thread.operation", &operation),
                Err("unknown operation_id: operation-read".into())
            );
        })
        .unwrap();
}

#[test]
fn store_free_operation_snapshot_preserves_owner_checks_and_receipts() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-operation", 0);
    stored_read_run(&mut state, "run-other", 0);
    state.store = None;
    let receipt = read_receipt(&record);
    state
        .operation_ledger
        .stage_acceptance(record.id.clone(), receipt.clone());
    state
        .operation_ledger
        .remember_in_memory_acceptance(&record.id);
    let context = state.thread_read_context();
    for owner in ["run-operation", "run-other"] {
        let params = json!({"entity_id": owner, "operation_id": receipt.operation_id});
        assert_eq!(
            context.read(None, "thread.operation", &params),
            state.thread_operation(&params)
        );
    }
    state
        .operation_ledger
        .forget_conversation(&receipt.conversation_id);
    assert_eq!(
        context
            .read(
                None,
                "thread.operation",
                &json!({"entity_id": record.id, "operation_id": receipt.operation_id})
            )
            .unwrap(),
        receipt.wire_value()
    );
}

#[test]
fn failed_roster_publication_only_retires_existing_read_admission() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-failed", 0);
    state.store = None;
    let mut context = state.thread_read_context();
    let params = json!({"entity_id": record.id});
    let expected = context.read(None, "thread.page", &params).unwrap();
    let roster = &mut state.runs.get_mut(&record.id).unwrap().agents;
    let added = roster
        .add(
            record.id.as_str(),
            ModelChoice::default(),
            "2026-10-19T20:03:00Z",
        )
        .id
        .clone();
    roster
        .primary_mut()
        .unwrap()
        .thread
        .post_user("unsaved", None, "2026-10-19T20:03:00Z");
    context.retain_owner_agents(&state, &record.id);
    assert_eq!(
        context.read(None, "thread.page", &params).unwrap(),
        expected
    );
    let added_params = json!({"entity_id": record.id, "agent_id": added});
    assert!(context.read(None, "thread.page", &added_params).is_err());

    state
        .runs
        .get_mut(&record.id)
        .unwrap()
        .agents
        .remove(&record.agents[0].id)
        .unwrap();
    context.retain_owner_agents(&state, &record.id);
    assert!(context.read(None, "thread.page", &params).is_err());
    context.update_owner(&state, &record.id);
    assert!(context.read(None, "thread.page", &added_params).is_ok());
}

#[tokio::test]
async fn sql_operation_reads_preserve_explicit_agent_binding_checks() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut record = stored_read_run(&mut state, "run-operation-scope", 0);
    record.agents.push(crate::agent::Agent::new(
        "agent-other",
        &record.id,
        ModelChoice::default(),
        2,
        "2026-10-19T20:03:00Z",
    ));
    let receipt = read_receipt(&record);
    state
        .store
        .as_ref()
        .unwrap()
        .accept_thread_post(&record.id, &record.agents, &receipt)
        .unwrap();
    state.runs.insert(
        record.id.clone(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    for named in [
        None,
        Some(record.agents[0].id.as_str()),
        Some("agent-other"),
    ] {
        let mut params = json!({"entity_id": record.id, "operation_id": receipt.operation_id});
        if let Some(named) = named {
            params["agent_id"] = json!(named);
        }
        assert_eq!(
            reader
                .read(|tx| context.read(Some(tx), "thread.operation", &params))
                .unwrap(),
            state.thread_operation(&params)
        );
    }
}

#[tokio::test]
async fn retained_thread_reads_publish_the_same_boundary_on_catch_up_and_activity() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-retention", 4);
    let store = state.store.as_ref().unwrap();
    store
        .prune_conversation_batch(
            &record.agents[0].id,
            crate::store::RetentionPolicy {
                min_messages: 1,
                max_messages: 1,
                max_age_days: 7,
            },
            "2026-10-20T00:00:00Z",
        )
        .unwrap();
    let context = state.thread_read_context();
    let mut reader = store.read_pool().acquire().await;
    for (method, params) in [
        ("thread.page", json!({"entity_id": record.id})),
        (
            "thread.page",
            json!({"entity_id": record.id, "after_sequence": 1, "newest": true}),
        ),
        (
            "thread.page",
            json!({"entity_id": record.id, "activity": "newest"}),
        ),
        (
            "thread.activity",
            json!({"entity_id": record.id, "from_sequence": 2, "through_sequence": 5}),
        ),
    ] {
        let actual = reader
            .read(|tx| context.read(Some(tx), method, &params))
            .unwrap()
            .unwrap();
        assert_eq!(actual["history_start_sequence"], 6, "{method}: {actual}");
        assert!(actual["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["data"]["sequence"].as_u64().unwrap() >= 6));
    }
}
