use super::*;

fn cases(record: &PersistedRun) -> [(&'static str, Value); 3] {
    [
        (
            "thread.page",
            json!({"entity_id": record.id, "before_sequence": 6, "limit": 1}),
        ),
        (
            "thread.activity",
            json!({"entity_id": record.id, "from_sequence": 2, "through_sequence": 5, "limit": 2}),
        ),
        (
            "thread.operation",
            json!({"entity_id": record.id, "operation_id": "operation-read"}),
        ),
    ]
}

fn accept(state: &AppState, record: &PersistedRun, addressed: &str) {
    let mut receipt = read_receipt(record);
    receipt.agent_id = addressed.into();
    state
        .store
        .as_ref()
        .unwrap()
        .accept_thread_post(&record.id, &record.agents, &receipt)
        .unwrap();
}

#[tokio::test]
async fn thread_read_work_loads_a_self_bound_roster_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-read-work", 4);
    accept(&state, &record, &record.agents[0].id);
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    for (method, params) in cases(&record) {
        let before = Store::agent_read_counts();
        reader
            .read(|tx| context.read(Some(tx), method, &params))
            .unwrap()
            .unwrap();
        let after = Store::agent_read_counts();
        assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0), "{method}");
    }
}

#[tokio::test]
async fn thread_read_work_reuses_same_owner_alias_roster() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut record = stored_read_run(&mut state, "run-alias-work", 4);
    let mut alias = crate::agent::Agent::new(
        "agent-read-alias",
        &record.id,
        ModelChoice::default(),
        2,
        "2026-10-19T20:03:00Z",
    );
    alias.bind_conversation(&record.agents[0].id);
    record.agents.push(alias);
    accept(&state, &record, "agent-read-alias");
    state.runs.insert(
        record.id.clone(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    for (method, mut params) in cases(&record) {
        params["agent_id"] = json!("agent-read-alias");
        let before = Store::agent_read_counts();
        reader
            .read(|tx| context.read(Some(tx), method, &params))
            .unwrap()
            .unwrap();
        let after = Store::agent_read_counts();
        assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0), "{method}");
    }
}

#[tokio::test]
async fn thread_read_work_does_not_decode_unrequested_inbox_summary_rows() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-summary-work", 4);
    accept(&state, &record, &record.agents[0].id);
    let requests = cases(&record);
    let expected = [
        state.thread_page(&requests[0].1).unwrap(),
        state.thread_activity(&requests[1].1).unwrap(),
        state.thread_operation(&requests[2].1).unwrap(),
    ];
    // This last message is outside every requested page. Inbox summaries
    // decode it, but none of these three read responses contains its fields.
    rusqlite::Connection::open(dir.path().join("store/build.db"))
        .unwrap()
        .execute(
            "UPDATE thread_items SET item = 'unrequested summary' WHERE agent_id = ?1 AND sequence = 6",
            [&record.agents[0].id],
        )
        .unwrap();
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    for ((method, params), expected) in requests.into_iter().zip(expected) {
        let actual = reader
            .read(|tx| context.read(Some(tx), method, &params))
            .unwrap();
        assert_eq!(actual, Ok(expected), "{method}");
    }
}
