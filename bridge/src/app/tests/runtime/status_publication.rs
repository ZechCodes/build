//! One real observer keeps independently committed roster writes and final attention visibility.
use super::*;
use crate::app::read_view::ReadView;
use crate::app::runtime::pumps::status_publication_probe::StatusPublicationProbe;
use crate::harness::{SessionStatusSnapshot, TurnContext};
use std::collections::BTreeMap;

mod metadata;
mod notifications;

const OWNER: &str = "run-status-publication";
const START: &str = "2030-01-01T00:00:00Z";
const STOP: &str = "2030-01-01T00:01:00Z";

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    pump: StatusPublicationProbe,
    session: Arc<dyn AgentSession>,
    key: TabKey,
    agent: String,
    clock: Arc<crate::timing::FrameClock>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_background_services(true)
    }

    fn passive() -> Self {
        Self::with_background_services(false)
    }

    fn with_background_services(background_services: bool) -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, directory.path());
        let root = insert_run(&mut app, &repo, directory.path(), OWNER, RunState::Building);
        let agent = crate::agent::derived_agent_id(OWNER);
        let key = insert_agent_tab(
            &mut app,
            &root,
            OWNER,
            &agent,
            DictatedSession::reporting(AgentStatus::Waiting),
        );
        app.seed_anchors_for_records_without_one();
        app.board.attention_mut().restore_entity_clocks(
            OWNER.into(),
            START.into(),
            START.into(),
            START.into(),
            "building".into(),
        );
        app.delivery_queue.clear_queued();
        app.initialize_snapshot_projection();
        let tab = app.session_registry.test_tab(&key).unwrap();
        let session = Arc::clone(&tab.session);
        let instance = tab.session_instance.clone().unwrap();
        let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
        app.frame_clock = Arc::clone(&clock);
        let state = if background_services {
            app.shared()
        } else {
            // Metadata reads exercise only this observer, without independent
            // delivery or maintenance producers racing its retained roots.
            app.initialize_read_context();
            Arc::new(Mutex::new(app))
        };
        let pump = StatusPublicationProbe::new(&state, key.clone(), &session, instance);
        Self {
            _directory: directory,
            state,
            pump,
            session,
            key,
            agent,
            clock,
        }
    }

    fn observe(&mut self, snapshot: &SessionStatusSnapshot) -> Value {
        let samples = self.clock.capture_phases();
        assert!(self.pump.observe(snapshot));
        assert_eq!(samples.active_frames(), 0);
        samples.snapshot()["agent.poll_status"].clone()
    }

    fn agent(&self) -> crate::agent::Agent {
        self.state.lock().unwrap().runs[OWNER]
            .agents
            .by_id(&self.agent)
            .unwrap()
            .clone()
    }

    fn durable_agent(&self) -> crate::agent::Agent {
        self.state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.id == OWNER)
            .unwrap()
            .agents
            .into_iter()
            .find(|agent| agent.id == self.agent)
            .unwrap()
    }

    fn sql(&self, sql: &str) {
        self.state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .writer_for_test()
            .execute_batch(sql)
            .unwrap();
    }

    fn assert_full_oracle(&self) {
        let app = self.state.lock().unwrap();
        let retained = app.read_view.load();
        let before = image(&retained);
        app.publish_all_snapshot_projection();
        assert_eq!(
            image(&app.read_view.load()),
            before,
            "post-observe published rows differ from full producer"
        );
        assert_eq!(image(&retained), before, "retained reader root changed");
    }
}

fn image(view: &ReadView) -> BTreeMap<(&'static str, String), Value> {
    view.keys()
        .filter(|key| {
            matches!(
                key.method,
                "board.list"
                    | "project.list"
                    | "workspace.list"
                    | "workspace.get"
                    | "agent.list"
                    | "entity.get"
            )
        })
        .filter_map(|key| {
            view.get_owned(key.method, &key.owner)
                .map(|value| ((key.method, key.owner.clone()), value))
        })
        .collect()
}

fn snapshot(status: AgentStatus, changed_at: &str) -> SessionStatusSnapshot {
    let mut snapshot = SessionStatusSnapshot::new(status);
    snapshot.changed_at = changed_at.into();
    snapshot
}

fn count(sample: &Value, phase: &str) -> u64 {
    sample[phase]["count"].as_u64().unwrap_or(0)
}

#[test]
fn status_attention_is_saved_once_and_joins_the_final_publication() {
    let mut fixture = Fixture::new();
    let old = fixture.state.lock().unwrap().read_view.load();
    let old_image = image(&old);
    let working = snapshot(AgentStatus::Working, START);
    let phases = fixture.observe(&working);
    assert_eq!(fixture.agent().working_since.as_deref(), Some(START));
    assert_eq!(
        fixture.durable_agent().working_since.as_deref(),
        Some(START)
    );
    assert_eq!(
        count(&phases, "finisher.snapshot"),
        1,
        "the roster retains its own committed finisher"
    );
    assert_eq!(
        count(&phases, "finisher.attention"),
        1,
        "the attention map still saves"
    );
    fixture.assert_full_oracle();
    assert_eq!(image(&old), old_image);
    assert_ne!(
        image(&fixture.state.lock().unwrap().read_view.load()),
        old_image
    );
    assert_eq!(count(&phases, "publication.global"), 2, "one committed roster publication plus the final observer publication; no intermediate attention publication");
    assert_eq!(count(&phases, "publication.install"), 2);

    let repeated = fixture.observe(&working);
    assert_eq!(count(&repeated, "finisher.snapshot"), 0);
    assert_eq!(count(&repeated, "finisher.attention"), 0);
    assert_eq!(count(&repeated, "publication.global"), 1);
    let mut waiting = snapshot(AgentStatus::Waiting, STOP);
    waiting.last_worked_at = Some(STOP.into());
    let stopped = fixture.observe(&waiting);
    assert!(fixture.agent().working_since.is_none());
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .board
            .attention()
            .last_worked_at(OWNER),
        Some(STOP)
    );
    fixture.assert_full_oracle();
    assert_eq!(count(&stopped, "publication.global"), 2);
}

#[test]
fn status_attention_save_failure_keeps_the_successful_roster_and_final_view() {
    let mut fixture = Fixture::new();
    let before = fixture
        .state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_attention();
    fixture.sql("CREATE TRIGGER reject_status_attention BEFORE INSERT ON attention BEGIN SELECT RAISE(ABORT, 'injected attention failure'); END;");
    let phases = fixture.observe(&snapshot(AgentStatus::Working, START));
    assert_eq!(fixture.agent().working_since.as_deref(), Some(START));
    assert_eq!(
        fixture.durable_agent().working_since.as_deref(),
        Some(START)
    );
    let app = fixture.state.lock().unwrap();
    assert_eq!(app.store.as_ref().unwrap().load_attention(), before);
    assert_eq!(
        app.board
            .attention()
            .attention(OWNER)
            .unwrap()
            .working_observed_at
            .as_deref(),
        Some(START)
    );
    drop(app);
    fixture.assert_full_oracle();
    assert_eq!(count(&phases, "publication.global"), 2);
}

#[test]
fn status_failed_roster_save_does_not_publish_a_rejected_working_interval() {
    let mut fixture = Fixture::new();
    fixture
        .state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .fail_next_write();
    fixture.observe(&snapshot(AgentStatus::Working, START));
    assert!(fixture.agent().working_since.is_none());
    assert!(fixture.durable_agent().working_since.is_none());
    fixture.assert_full_oracle();
    assert_eq!(fixture.pump.recorded_context(), (None, 0));
}

#[test]
fn status_context_failure_preserves_prior_commit_and_existing_recorded_context_semantics() {
    let mut fixture = Fixture::new();
    fixture.sql("CREATE TRIGGER reject_status_context BEFORE INSERT ON agents WHEN json_extract(NEW.record, '$.last_context_tokens') IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected context failure'); END;");
    let context = TurnContext {
        context_tokens: 123,
        cache_read_tokens: 45,
    };
    let mut working = snapshot(AgentStatus::Working, START);
    working.context = Some(context);
    fixture.observe(&working);
    assert_eq!(fixture.agent().working_since.as_deref(), Some(START));
    assert_eq!(
        fixture.durable_agent().working_since.as_deref(),
        Some(START)
    );
    assert!(fixture.agent().last_context_tokens.is_none());
    assert!(fixture.durable_agent().last_context_tokens.is_none());
    assert_eq!(
        fixture.pump.recorded_context(),
        (Some(context), 0),
        "the existing pump records an observed context even when its void save helper fails"
    );
    fixture.sql("DROP TRIGGER reject_status_context;");
    let repeated = fixture.observe(&working);
    assert_eq!(
        count(&repeated, "finisher.snapshot"),
        0,
        "identical reported context is not retried"
    );
    assert!(fixture.agent().last_context_tokens.is_none());
    fixture.assert_full_oracle();
}

#[test]
fn status_context_reset_and_exact_registration_fences_survive_attention_batching() {
    let mut fixture = Fixture::new();
    let context = TurnContext {
        context_tokens: 321,
        cache_read_tokens: 54,
    };
    let mut working = snapshot(AgentStatus::Working, START);
    working.context = Some(context);
    fixture.observe(&working);
    assert_eq!(fixture.agent().last_context_tokens, Some(321));
    working.context = None;
    working.context_reset_count = 1;
    fixture.observe(&working);
    assert_eq!(fixture.pump.recorded_context(), (None, 1));
    assert!(fixture.agent().last_context_tokens.is_none());
    fixture.assert_full_oracle();
    let old = fixture.state.lock().unwrap().read_view.load();
    let original_instance = fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&fixture.key)
        .unwrap()
        .session_instance
        .clone();
    {
        let mut app = fixture.state.lock().unwrap();
        let tab = app.session_registry.test_tab_mut(&fixture.key).unwrap();
        assert!(Arc::ptr_eq(&tab.session, &fixture.session));
        let mut replaced = tab.session_instance.clone().unwrap();
        replaced.agent_id = "replacement-lineage".into();
        tab.session_instance = Some(replaced);
    }
    assert!(!fixture.pump.observe(&working));
    assert!(Arc::ptr_eq(
        &old,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    assert_eq!(fixture.pump.recorded_context(), (None, 1));
    {
        let mut app = fixture.state.lock().unwrap();
        let tab = app.session_registry.test_tab_mut(&fixture.key).unwrap();
        tab.session_instance = original_instance;
        tab.session = Arc::new(DictatedSession::reporting(AgentStatus::Working));
    }
    assert!(
        !fixture.pump.observe(&working),
        "same key and lineage cannot authenticate a replacement session handle"
    );
    assert!(Arc::ptr_eq(
        &old,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    assert_eq!(fixture.pump.recorded_context(), (None, 1));
}
