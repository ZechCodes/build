//! Adjacent immutable publications can share one ephemeral ownership capture.
use super::*;
use crate::app::read_view::ReadView;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
enum Wrapper {
    Owner,
    Metadata,
}

impl Wrapper {
    fn publish(self, app: &AppState, owner: &str) {
        match self {
            Self::Owner => app.note_owner_board_changed(owner),
            Self::Metadata => app.publish_read_metadata(),
        }
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    owner: String,
    agent: String,
    other: String,
    task: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (mut app, project) = tracked_with_origin(root.path());
        let (owner, agent) = agent_on_checkout(&mut app, &project, "first");
        let (_, other) = agent_on_checkout(&mut app, &project, "second");
        let task = filed(
            &mut app,
            &project,
            &format!("@agent:{agent} @agent:{other}"),
        );
        app.delivery_queue.clear_queued();
        app.initialize_snapshot_projection();
        app.initialize_read_context();
        Self {
            _root: root,
            state: Arc::new(Mutex::new(app)),
            owner,
            agent,
            other,
            task,
        }
    }

    async fn read(&self, method: &str, params: Value) -> Value {
        let (service, pool) = {
            let app = self.state.lock().unwrap();
            (app.read_service(), app.tracker_store().unwrap().read_pool())
        };
        service.call(req(method, params), Some(pool.acquire().await))
    }

    async fn task(&self) -> Value {
        let answer = self
            .read("tasks.get", json!({"task_id":self.task["id"]}))
            .await;
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"]["task"].clone()
    }

    fn publish_measured(&self, wrapper: Wrapper) -> Value {
        let clock = self.state.lock().unwrap().frame_clock.clone();
        let samples = clock.capture_phases();
        let timer = clock.frame("board.list");
        {
            let app = timer.lock(&self.state);
            wrapper.publish(&app, &self.owner);
            assert!(
                app.snapshot_ownership_context().is_none(),
                "{wrapper:?} leaked its context"
            );
        }
        drop(timer);
        samples.snapshot()
    }

    fn assert_full_view_parity(&self) {
        let app = self.state.lock().unwrap();
        let actual = app.read_view.load();
        let actual_values = values(&actual);
        app.publish_all_snapshot_projection();
        let full = app.read_view.load();
        assert_eq!(actual_values, values(&full));
        assert_eq!(
            actual.history_keys().collect::<Vec<_>>(),
            full.history_keys().collect::<Vec<_>>()
        );
        for agent in actual.history_keys() {
            assert_eq!(
                actual.history(agent).unwrap().render(&json!({})),
                full.history(agent).unwrap().render(&json!({})),
                "history/{agent}"
            );
        }
        assert!(app.snapshot_ownership_context().is_none());
    }
}

fn values(view: &ReadView) -> BTreeMap<(&'static str, String), Value> {
    view.keys()
        .map(|key| {
            (
                (key.method, key.owner.clone()),
                view.get_owned(key.method, &key.owner).unwrap(),
            )
        })
        .collect()
}

async fn assert_single_capture(wrapper: Wrapper) {
    let fixture = Fixture::new();
    let before = fixture.task().await;
    assert_eq!(before["identities"][&fixture.agent]["available"], true);
    assert_eq!(before["identities"][&fixture.other]["available"], true);
    assert_eq!(
        before["identities"][&fixture.agent]["workspace_name"],
        "first"
    );
    assert_eq!(
        before["identities"][&fixture.other]["workspace_name"],
        "second"
    );
    assert!(before["identities"][&fixture.agent]["workspace_id"].is_string());
    let samples = fixture.publish_measured(wrapper);
    assert_eq!(
        fixture.task().await,
        before,
        "{wrapper:?} changed reader identity/workspace association"
    );
    fixture.assert_full_view_parity();
    assert_eq!(
        samples["board.list"]["entered.publication.ownership_capture"]["count"], 1,
        "{wrapper:?} recaptured the same immutable ownership inputs: {samples}"
    );
}

#[tokio::test]
async fn owner_board_wrapper_shares_capture_with_reader_metadata() {
    assert_single_capture(Wrapper::Owner).await;
}

#[tokio::test]
async fn metadata_wrapper_shares_capture_with_reader_metadata() {
    assert_single_capture(Wrapper::Metadata).await;
}

#[tokio::test]
async fn wrapper_context_expires_before_failed_save_and_later_successful_mutation() {
    for wrapper in [Wrapper::Owner, Wrapper::Metadata] {
        let fixture = Fixture::new();
        let committed = fixture.task().await;
        fixture.publish_measured(wrapper);
        let retained = fixture.state.lock().unwrap().read_view.load();
        let rejected = {
            let mut app = fixture.state.lock().unwrap();
            let mut candidate = app.take_run(&fixture.owner).unwrap();
            candidate.run.goal = "Rejected wrapper candidate".into();
            candidate.agents.by_id_mut(&fixture.agent).unwrap().name =
                Some("Rejected wrapper name".into());
            let added = candidate
                .agents
                .add(
                    fixture.owner.as_str(),
                    candidate.model_choice.clone(),
                    "now",
                )
                .id
                .clone();
            app.store.as_ref().unwrap().fail_next_write();
            assert!(app
                .finish_run_mutation(fixture.owner.clone(), candidate)
                .is_err());
            assert!(app.snapshot_ownership_context().is_none());
            added
        };
        fixture.publish_measured(wrapper);
        assert_eq!(fixture.task().await, committed);
        let refused = fixture
            .read(
                "thread.page",
                json!({"entity_id":fixture.owner,
            "agent_id":rejected,"conversation_id":rejected}),
            )
            .await;
        assert_eq!(
            refused["ok"], false,
            "failed roster candidate became readable: {refused}"
        );
        {
            let mut app = fixture.state.lock().unwrap();
            assert!(app.snapshot_ownership_context().is_none());
            app.set_agent_name(&fixture.owner, &fixture.agent, "accepted after wrapper")
                .unwrap();
        }
        fixture.publish_measured(wrapper);
        assert_eq!(
            fixture.task().await["identities"][&fixture.agent]["name"],
            "accepted after wrapper"
        );
        let prior = retained.get_owned("agent.list", &fixture.owner).unwrap();
        let agent = prior["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == fixture.agent)
            .unwrap();
        assert_eq!(
            agent["name"], "first",
            "retained root changed after later mutation"
        );
        assert!(!values(&fixture.state.lock().unwrap().read_view.load())
            .values()
            .any(|value| value.to_string().contains("Rejected wrapper")));
        fixture.assert_full_view_parity();
    }
}
