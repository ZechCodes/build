//! Actual activity admission, persistence and immutable publication.
use super::*;
use crate::app::runtime::pumps::activity_probe::ActivityProbe;
use crate::harness::{ActivityReport, AgentActivity};

const OWNER: &str = "run-activity-publication";

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    session: Arc<dyn AgentSession>,
    key: TabKey,
    agent: String,
    pump: ActivityProbe,
    clock: Arc<crate::timing::FrameClock>,
}

impl Fixture {
    fn new() -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state_before_admission(&repo, directory.path());
        let root = insert_run(&mut app, &repo, directory.path(), OWNER, RunState::Building);
        let agent = crate::agent::derived_agent_id(OWNER);
        let key = insert_agent_tab(
            &mut app, &root, OWNER, &agent,
            DictatedSession::reporting(AgentStatus::Working).named("activity-provider-session"),
        );
        app.delivery_queue.clear_queued();
        app.initialize_snapshot_projection();
        app.initialize_read_context();
        let tab = app.session_registry.test_tab(&key).unwrap();
        let session = Arc::clone(&tab.session);
        let instance = tab.session_instance.clone().unwrap();
        let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
        app.frame_clock = Arc::clone(&clock);
        let state = Arc::new(Mutex::new(app));
        let pump = ActivityProbe::new(&state, key.clone(), &session, instance);
        Self { _directory: directory, state, session, key, agent, pump, clock }
    }

    fn report(&self, summary: &str) -> bool {
        self.pump.report(&ActivityReport::own_work(AgentActivity::Narration { summary: summary.into() }))
    }
}

#[test]
fn activity_admission_is_timed_and_named() {
    let fixture = Fixture::new();
    let samples = fixture.clock.capture_phases();
    assert!(fixture.report("observed activity"));
    let captured = samples.snapshot();
    assert_eq!(captured["agent.activity"]["entered.lock"]["count"], 1,
        "activity admission must join frame timing: {captured}");
    assert_eq!(fixture.clock.stats()["methods"]["agent.activity"]["served"], 1);
}

#[test]
fn activity_replacement_refuses_the_old_session_before_persisting() {
    let fixture = Fixture::new();
    let before = {
        let mut app = fixture.state.lock().unwrap();
        app.session_registry.test_remove_tab(&fixture.key);
        app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread.clone()
    };
    assert!(!fixture.report("stale activity"));
    let app = fixture.state.lock().unwrap();
    assert_eq!(serde_json::to_value(&app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread).unwrap(),
        serde_json::to_value(before).unwrap());
    assert_eq!(fixture.session.session_id().as_deref(), Some("activity-provider-session"));
}

#[test]
fn repeated_session_name_does_not_save_or_prepare_another_owner_candidate() {
    let fixture = Fixture::new();
    let instance = fixture.state.lock().unwrap().session_registry.test_tab(&fixture.key)
        .unwrap().session_instance.clone().unwrap();
    let name = || SelfReport { named: Some("activity-provider-session".into()), model: None, effort: None };
    {
        let mut app = fixture.state.lock().unwrap();
        app.note_self_report(OWNER, &fixture.agent, &instance, name());
    }
    let samples = fixture.clock.capture_phases();
    let timer = fixture.clock.frame("agent.deliver");
    {
        let mut app = timer.lock(&fixture.state);
        app.note_self_report(OWNER, &fixture.agent, &instance, name());
    }
    drop(timer);
    let captured = samples.snapshot();
    assert_eq!(captured["agent.deliver"]["entered.finisher.save"]["count"].as_u64().unwrap_or(0), 0,
        "unchanged session name must not save the owner again: {captured}");
    assert_eq!(captured["agent.deliver"]["entered.finisher.snapshot"]["count"].as_u64().unwrap_or(0), 0);
}

#[tokio::test]
async fn activity_overflow_counts_loss_and_continues_with_retained_reports() {
    let fixture = Fixture::new();
    let (sender, receiver) = broadcast::channel(2);
    for index in 0..8 {
        sender.send(ActivityReport::own_work(AgentActivity::Narration {
            summary: format!("overflow witness {index}"),
        })).unwrap();
    }
    spawn_activity_pump(&fixture.state, fixture.key.clone(), Some(receiver));
    wait_for(Duration::from_secs(5), || {
        let app = fixture.state.lock().unwrap();
        let thread = &app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread;
        serde_json::to_string(thread).unwrap().contains("overflow witness 7").then_some(())
    }).await.expect("the retained tail must still publish after lag");
    assert_eq!(fixture.clock.stats()["activity_lagged_reports"], 6,
        "overflow must be visible rather than silently pretending all reports persisted");
}

#[test]
fn repeated_activity_saves_and_publishes_its_committed_owner_once() {
    let fixture = Fixture::new();
    assert!(fixture.report("warm provider identity"));
    let retained = fixture.state.lock().unwrap().read_view.load();
    let samples = fixture.clock.capture_phases();
    assert!(fixture.report("single activity commit"));
    let captured = samples.snapshot();
    assert_eq!(captured["agent.activity"]["entered.finisher.save"]["count"], 1, "{captured}");
    assert_eq!(captured["agent.activity"]["entered.publication.install"]["count"], 1, "{captured}");
    let app = fixture.state.lock().unwrap();
    let durable = app.store.as_ref().unwrap().thread_items(&fixture.agent).unwrap();
    assert!(serde_json::to_string(&durable).unwrap().contains("single activity commit"));
    let published = app.read_view.load();
    assert!(serde_json::to_string(&published.history(&fixture.agent).unwrap().render(&json!({}))).unwrap().contains("single activity commit"));
    assert!(!serde_json::to_string(&retained.history(&fixture.agent).unwrap().render(&json!({}))).unwrap().contains("single activity commit"));
}

#[test]
fn activity_failed_save_never_publishes_the_candidate() {
    let fixture = Fixture::new();
    assert!(fixture.report("committed activity"));
    {
        let app = fixture.state.lock().unwrap();
        app.store.as_ref().unwrap().writer_for_test().execute_batch(
            "CREATE TRIGGER reject_activity BEFORE INSERT ON thread_items WHEN instr(NEW.item, 'rejected activity candidate') > 0 BEGIN SELECT RAISE(ABORT, 'injected activity failure'); END;"
        ).unwrap();
    }
    assert!(fixture.report("rejected activity candidate"));
    let app = fixture.state.lock().unwrap();
    let durable = app.store.as_ref().unwrap().thread_items(&fixture.agent).unwrap();
    assert!(!serde_json::to_string(&durable).unwrap().contains("rejected activity candidate"));
    let published = app.read_view.load();
    assert!(!serde_json::to_string(&published.history(&fixture.agent).unwrap().render(&json!({}))).unwrap().contains("rejected activity candidate"));
    assert!(!serde_json::to_string(&app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread).unwrap().contains("rejected activity candidate"));
}
