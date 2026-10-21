//! Status metadata shares only the final immutable candidate, never a roster save.
use super::*;

struct MetadataFixture {
    observer: Fixture,
    project: String,
    task: Value,
}

impl MetadataFixture {
    fn new() -> Self {
        let observer = Fixture::new();
        let (project, task) = {
            let mut app = observer.state.lock().unwrap();
            let mut owner = app.take_run(OWNER).unwrap();
            owner.agents.by_id_mut(&observer.agent).unwrap().name =
                Some("Committed status reader".into());
            app.finish_run_mutation(OWNER.into(), owner).unwrap();
            let project = app.project_at(0).id.clone();
            let task = crate::app::tests::tracker::filed(
                &mut app,
                &project,
                &format!("Ask @agent:{}", observer.agent),
            );
            app.delivery_queue.clear_queued();
            app.initialize_read_context();
            app.publish_read_metadata_only();
            (project, task)
        };
        Self {
            observer,
            project,
            task,
        }
    }

    async fn read(&self, method: &str, params: Value) -> Value {
        let (service, pool) = {
            let app = self.observer.state.lock().unwrap();
            (app.read_service(), app.tracker_store().unwrap().read_pool())
        };
        service.call(req(method, params), Some(pool.acquire().await))
    }

    async fn reader_image(&self) -> Value {
        let task = self
            .read("tasks.get", json!({"task_id": self.task["id"]}))
            .await;
        let mut list = self
            .read("tasks.list", json!({"project_id": self.project}))
            .await;
        let page = self
            .read(
                "thread.page",
                json!({"entity_id":OWNER,"agent_id":self.observer.agent,
                    "conversation_id":self.observer.agent}),
            )
            .await;
        for answer in [&task, &list, &page] {
            assert_eq!(answer["ok"], true, "{answer}");
        }
        let identity = &task["result"]["task"]["identities"][&self.observer.agent];
        assert_eq!(identity["name"], "Committed status reader");
        assert_eq!(identity["available"], true);
        // This reply clock is evaluated at each read, independently of the
        // published user-session fields whose exact values we compare below.
        assert!(list["result"]["user_session"]["now_ms"].is_i64());
        list["result"]["user_session"]
            .as_object_mut()
            .unwrap()
            .remove("now_ms");
        json!({"task":task["result"],"list":list["result"],"page":page["result"]})
    }

    fn assert_scope_ended(&self) {
        assert!(self
            .observer
            .state
            .lock()
            .unwrap()
            .snapshot_ownership_context()
            .is_none());
    }
}

#[tokio::test]
async fn first_status_working_shares_final_metadata_capture_without_combining_the_save() {
    let mut fixture = MetadataFixture::new();
    let before = fixture.reader_image().await;
    let old = fixture.observer.state.lock().unwrap().read_view.load();
    let old_image = image(&old);
    let phases = fixture
        .observer
        .observe(&snapshot(AgentStatus::Working, START));
    assert_eq!(
        fixture.observer.agent().working_since.as_deref(),
        Some(START)
    );
    assert_eq!(
        fixture.observer.durable_agent().working_since.as_deref(),
        Some(START)
    );
    assert_eq!(fixture.reader_image().await, before);
    fixture.observer.assert_full_oracle();
    assert_eq!(image(&old), old_image);
    fixture.assert_scope_ended();
    assert_eq!(count(&phases, "finisher.snapshot"), 1);
    assert_eq!(count(&phases, "finisher.attention"), 1);
    assert_eq!(count(&phases, "read_context.owner"), 1);
    assert_eq!(count(&phases, "read_context.metadata"), 1);
    assert_eq!(count(&phases, "publication.global"), 2);
    assert_eq!(count(&phases, "publication.install"), 2);
    assert_eq!(count(&phases, "publication.ownership_capture"), 3,
        "the committed finisher and owner admission retain separate captures; only final snapshot/metadata share: {phases}");
}

#[tokio::test]
async fn attention_only_status_refreshes_user_session_once_and_unchanged_status_skips_metadata() {
    let mut fixture = MetadataFixture::new();
    let mut working = snapshot(AgentStatus::Working, START);
    fixture.observer.observe(&working);
    let prior = fixture.reader_image().await;
    let activity = {
        let mut app = fixture.observer.state.lock().unwrap();
        let activity = app.user_session.last_activity_ms.unwrap_or(0) + 1;
        // The observer must refresh metadata itself, without a presence producer.
        app.user_session.last_activity_ms = Some(activity);
        activity
    };
    working.last_worked_at = Some(STOP.into());
    let phases = fixture.observer.observe(&working);
    let after = fixture.reader_image().await;
    assert_eq!(after["task"], prior["task"]);
    assert_eq!(after["page"], prior["page"]);
    assert_eq!(after["list"]["user_session"]["last_activity_ms"], activity);
    assert_eq!(
        fixture.observer.agent().working_since.as_deref(),
        Some(START)
    );
    fixture.observer.assert_full_oracle();
    fixture.assert_scope_ended();
    assert_eq!(count(&phases, "finisher.snapshot"), 0);
    assert_eq!(count(&phases, "finisher.attention"), 1);
    assert_eq!(count(&phases, "publication.install"), 1);
    assert_eq!(count(&phases, "read_context.metadata"), 1);
    assert_eq!(
        count(&phases, "publication.ownership_capture"),
        1,
        "attention-only metadata and final publication observed the same candidate twice: {phases}"
    );
    let repeated = fixture.observer.observe(&working);
    assert_eq!(fixture.reader_image().await, after);
    assert_eq!(count(&repeated, "read_context.metadata"), 0);
    assert_eq!(count(&repeated, "publication.ownership_capture"), 1);
    fixture.assert_scope_ended();
}

#[tokio::test]
async fn status_metadata_keeps_committed_identity_and_admission_after_roster_failure() {
    let mut fixture = MetadataFixture::new();
    let before = fixture.reader_image().await;
    let rejected = {
        let mut app = fixture.observer.state.lock().unwrap();
        let mut candidate = app.take_run(OWNER).unwrap();
        candidate
            .agents
            .by_id_mut(&fixture.observer.agent)
            .unwrap()
            .name = Some("Rejected status name".into());
        let added = candidate
            .agents
            .add(OWNER, candidate.model_choice.clone(), "now")
            .id
            .clone();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_run_mutation(OWNER.into(), candidate).is_err());
        // The first Working interval is a second, independent failed save.
        app.store.as_ref().unwrap().fail_next_write();
        added
    };
    fixture
        .observer
        .observe(&snapshot(AgentStatus::Working, START));
    assert!(fixture.observer.agent().working_since.is_none());
    assert!(fixture.observer.durable_agent().working_since.is_none());
    assert_eq!(fixture.reader_image().await, before);
    let refused = fixture
        .read(
            "thread.page",
            json!({"entity_id":OWNER,"agent_id":rejected,
            "conversation_id":rejected}),
        )
        .await;
    assert_eq!(
        refused["ok"], false,
        "failed candidate gained admission: {refused}"
    );
    fixture.observer.assert_full_oracle();
    fixture.assert_scope_ended();
}

#[tokio::test]
async fn status_metadata_preserves_successful_roster_when_attention_save_fails() {
    let mut fixture = MetadataFixture::new();
    let before = fixture.reader_image().await;
    fixture.observer.sql("CREATE TRIGGER reject_status_metadata_attention BEFORE INSERT ON attention BEGIN SELECT RAISE(ABORT, 'injected attention failure'); END;");
    let phases = fixture
        .observer
        .observe(&snapshot(AgentStatus::Working, START));
    assert_eq!(
        fixture.observer.durable_agent().working_since.as_deref(),
        Some(START)
    );
    assert_eq!(fixture.reader_image().await, before);
    fixture.observer.assert_full_oracle();
    fixture.assert_scope_ended();
    assert_eq!(count(&phases, "publication.install"), 2);
}
