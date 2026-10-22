use super::*;
use crate::harness::{AgentStatus, SessionStatusSnapshot};

#[test]
fn status_snapshot_attention_publishes_only_its_known_owner_and_preserves_clocks() {
    let fixture = Fixture::new();
    fixture.mutate(|app| {
        app.see_attention(&fixture.owner, Some(&fixture.agents[0]), Default::default());
    });
    let cursors = fixture
        .agents
        .iter()
        .map(|agent| {
            (
                agent.clone(),
                fixture.agent(agent)["read_through_sequence"].clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut observed = Vec::new();
    for (status, changed, completed) in [
        (AgentStatus::Working, "2999-01-01T12:00:01Z", None),
        (
            AgentStatus::Waiting,
            "2999-01-01T12:00:02Z",
            Some("2999-01-01T12:00:02Z"),
        ),
    ] {
        let mut snapshot = SessionStatusSnapshot::new(status);
        snapshot.changed_at = changed.into();
        snapshot.last_worked_at = completed.map(str::to_owned);
        let (count, _) = fixture.mutate(|app| {
            app.record_agent_status_snapshot(&fixture.owner, &fixture.agents[0], &snapshot);
        });
        {
            let app = fixture.state.lock().unwrap();
            let saved = app.store.as_ref().unwrap().load_attention();
            let attention = &saved[&fixture.owner];
            assert_eq!(
                app.runs[&fixture.owner]
                    .agents
                    .by_id(&fixture.agents[0])
                    .unwrap()
                    .working_since
                    .as_deref(),
                matches!(status, AgentStatus::Working).then_some(changed)
            );
            assert_eq!(
                attention.working_observed_at.as_deref(),
                matches!(status, AgentStatus::Working).then_some(changed)
            );
            assert_eq!(attention.last_worked_at.as_deref(), completed);
        }
        for (agent, cursor) in &cursors {
            assert_eq!(fixture.agent(agent)["read_through_sequence"], *cursor);
        }
        assert_eq!(fixture.agent(&fixture.agents[0])["unread_count"], 0);
        assert_eq!(fixture.agent(&fixture.agents[1])["unread_count"], 1);
        fixture.assert_full_publication_parity();
        eprintln!("{status:?}: {count:?}");
        observed.push(count);
    }
    for count in observed {
        assert_eq!(
            count.workspace_details_full, 0,
            "known status requested all workspace details"
        );
        assert!(
            count.branches > 0,
            "render counter must observe the changed owner"
        );
        assert!(
            count.branches <= 2,
            "status repeated owner or unrelated rendering: {count:?}"
        );
    }
}

#[tokio::test]
async fn failed_status_owner_save_restores_working_roster_and_published_thread_admission() {
    let fixture = Fixture::new();
    let (reads, pool, roster, saved) = {
        let app = fixture.state.lock().unwrap();
        app.initialize_read_context();
        let store = app.store.as_ref().unwrap();
        (
            app.read_service(),
            store.read_pool(),
            app.runs[&fixture.owner].agents.clone(),
            store.load_all_runs().unwrap(),
        )
    };
    let params = json!({"entity_id":fixture.owner,"agent_id":fixture.agents[0],"conversation_id":fixture.agents[0]});
    let before = reads.call(
        req("thread.page", params.clone()),
        Some(pool.acquire().await),
    );
    assert_eq!(before["ok"], true, "{before}");
    {
        let mut app = fixture.state.lock().unwrap();
        let snapshot = SessionStatusSnapshot::new(AgentStatus::Working);
        app.store.as_ref().unwrap().fail_next_write();
        app.record_agent_status_snapshot(&fixture.owner, &fixture.agents[0], &snapshot);
        assert_eq!(app.runs[&fixture.owner].agents, roster);
        assert_eq!(app.store.as_ref().unwrap().load_all_runs().unwrap(), saved);
    }
    assert_eq!(
        reads.call(req("thread.page", params), Some(pool.acquire().await)),
        before
    );
}
