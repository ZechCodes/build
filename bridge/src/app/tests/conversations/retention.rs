use super::*;

#[test]
fn retention_publication_trims_push_tips_and_rejects_an_old_generation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut record = fake_run_record("retention-push");
    let agent = &mut record.agents[0];
    agent
        .thread
        .post_agent_offering("old", Vec::new(), "2026-10-01T00:00:00Z", false);
    agent
        .thread
        .post_agent_offering("kept", Vec::new(), "2026-10-24T00:00:00Z", false);
    let conversation_id = agent.id.clone();
    let floor = agent.thread.items.last().unwrap().sequence();
    let high = agent.thread.last_sequence();
    let mut boundary = crate::store::RetentionBoundary {
        thread_id: agent.thread.id.clone(),
        history_start_sequence: floor,
        high_watermark: high,
    };
    state.runs.insert(
        record.id.clone(),
        crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into()),
    );
    state.initialize_snapshot_projection();
    state.publish_retention_boundary(&conversation_id, boundary.clone(), 1);
    let tips = state.thread_tips(&record.id, &[(conversation_id.clone(), 0)]);
    assert_eq!(tips[0].history_start_sequence, Some(floor));
    assert!(tips[0]
        .items
        .iter()
        .all(|item| item["data"]["sequence"].as_u64().unwrap() >= floor));
    assert_eq!(tips[0].last_sequence, high);
    boundary.thread_id = "old-generation".into();
    boundary.history_start_sequence = high + 100;
    state.publish_retention_boundary(&conversation_id, boundary, 100);
    assert_eq!(
        state.thread_tips(&record.id, &[])[0].history_start_sequence,
        Some(floor)
    );
}
