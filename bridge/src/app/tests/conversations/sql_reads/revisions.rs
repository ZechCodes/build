use super::*;

fn revision_run(state: &mut AppState, owner: &str) -> PersistedRun {
    let mut record = stored_read_run(state, owner, 0);
    record.agents[0].thread.add_revision(
        crate::thread::ArtifactKind::Diff,
        "saved revision witness",
        "2026-10-21T12:00:00Z",
    );
    state.store.as_ref().unwrap().save_run(&record).unwrap();
    state.runs.insert(
        owner.into(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    record
}

#[tokio::test]
async fn sql_thread_revision_matches_content_alias_and_address_errors() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let canonical = revision_run(&mut state, "revision-canonical");
    let mut foreign = revision_run(&mut state, "revision-foreign");
    let foreign_revision = foreign.agents[0].thread.add_revision(
        crate::thread::ArtifactKind::Diff,
        "foreign revision",
        "2026-10-21T12:01:00Z",
    );
    state.store.as_ref().unwrap().save_run(&foreign).unwrap();
    let mut alias = stored_read_run(&mut state, "revision-alias", 0);
    alias.agents[0].bind_conversation(&canonical.agents[0].id);
    state.store.as_ref().unwrap().save_run(&alias).unwrap();
    state.runs.insert(
        alias.id.clone(),
        crate::orchestrator::ActiveRun::reattach(&alias, ".build/plan.md".into()),
    );
    let context = state.thread_read_context();
    let mut reader = state.store.as_ref().unwrap().read_pool().acquire().await;
    let params = json!({"entity_id":alias.id,
        "revision_id":canonical.agents[0].thread.revisions[0].id});
    let expected = state.thread_revision(&params).unwrap();
    assert_eq!(expected["contents"], "saved revision witness");
    state
        .runs
        .get_mut(&canonical.id)
        .unwrap()
        .agents
        .primary_mut()
        .unwrap()
        .thread
        .revisions[0]
        .snapshot = Some("unsaved candidate".into());
    assert_eq!(
        reader
            .read(|tx| context.read(Some(tx), "thread.revision", &params))
            .unwrap()
            .unwrap(),
        expected
    );
    state
        .runs
        .get_mut(&canonical.id)
        .unwrap()
        .agents
        .primary_mut()
        .unwrap()
        .thread
        .revisions[0]
        .snapshot = Some("saved revision witness".into());
    for extras in [
        json!({}),
        json!({"agent_id":alias.agents[0].id}),
        json!({"agent_id":foreign.agents[0].id}),
        json!({"agent_id":"missing"}),
        json!({"agent_id":""}),
        json!({"conversation_id":"stale"}),
        json!({"thread_id":"stale"}),
        json!({"revision_id":"missing"}),
        json!({"revision_id":foreign_revision.id}),
        json!({"entity_id":"missing"}),
    ] {
        let mut query = params.clone();
        query
            .as_object_mut()
            .unwrap()
            .extend(extras.as_object().unwrap().clone());
        assert_eq!(
            reader
                .read(|tx| context.read(Some(tx), "thread.revision", &query))
                .unwrap(),
            state.thread_revision(&query),
            "{query}"
        );
    }
    // Snapshot eviction keeps metadata, but must refuse the absent body.
    let mut evicted = canonical.clone();
    evicted.agents[0].thread.revisions[0].snapshot = None;
    state.store.as_ref().unwrap().save_run(&evicted).unwrap();
    assert_eq!(
        reader
            .read(|tx| context.read(Some(tx), "thread.revision", &params))
            .unwrap(),
        Err("revision snapshot is no longer retained".into())
    );
}

#[tokio::test]
async fn sql_thread_revision_keeps_one_committed_snapshot_across_reset() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = revision_run(&mut state, "revision-reset");
    let context = state.thread_read_context();
    let params = json!({"entity_id":record.id,"thread_id":record.agents[0].thread.id,
        "revision_id":record.agents[0].thread.revisions[0].id});
    let expected = state.thread_revision(&params).unwrap();
    let store = state.store.as_ref().unwrap();
    let mut reader = store.read_pool().acquire().await;
    reader
        .read(|tx| {
            tx.load_agent_record(&record.agents[0].id).unwrap().unwrap();
            let mut replacement = record.agents[0].clone();
            replacement.thread = crate::thread::Thread::for_agent(&replacement.id);
            replacement.thread.id = "thread:revision-after-reset".into();
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
                context.read(Some(tx), "thread.revision", &params).unwrap(),
                expected
            );
        })
        .unwrap();
    assert!(reader
        .read(|tx| context.read(Some(tx), "thread.revision", &params))
        .unwrap()
        .unwrap_err()
        .starts_with("stale thread_id"));
    let mut current = params;
    current.as_object_mut().unwrap().remove("thread_id");
    assert_eq!(
        reader
            .read(|tx| context.read(Some(tx), "thread.revision", &current))
            .unwrap(),
        Err("unknown revision_id".into())
    );
    let mut retired = context;
    retired.forget_owner(&record.id);
    assert_eq!(
        reader
            .read(|tx| retired.read(Some(tx), "thread.revision", &current))
            .unwrap(),
        Err("unknown id".into())
    );
}

#[test]
fn store_free_thread_revision_publishes_only_successful_rosters() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = revision_run(&mut state, "revision-memory");
    state.store = None;
    state.initialize_read_context();
    let service = state.read_service();
    let params =
        json!({"entity_id":record.id,"revision_id":record.agents[0].thread.revisions[0].id});
    let expected = state.thread_revision(&params).unwrap();
    let read = || service.call(req("thread.revision", params.clone()), None);
    assert_eq!(read()["result"], expected);
    state
        .resetting_conversations
        .insert(record.agents[0].id.clone());
    state.publish_read_metadata();
    assert_eq!(
        read()["result"],
        expected,
        "retirement preserves readable committed revisions"
    );
    state
        .runs
        .get_mut(&record.id)
        .unwrap()
        .agents
        .primary_mut()
        .unwrap()
        .thread
        .revisions[0]
        .snapshot = Some("unsaved candidate".into());
    state.publish_read_owner(&record.id, false);
    state.publish_read_metadata();
    assert_eq!(read()["result"], expected);
    state.publish_read_owner(&record.id, true);
    assert_eq!(read()["result"]["contents"], "unsaved candidate");
    state.runs.remove(&record.id);
    state.publish_read_owner(&record.id, false);
    assert_eq!(read()["ok"], false);
}
