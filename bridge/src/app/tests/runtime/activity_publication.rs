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
            &mut app,
            &root,
            OWNER,
            &agent,
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
        Self {
            _directory: directory,
            state,
            session,
            key,
            agent,
            pump,
            clock,
        }
    }

    fn report(&self, summary: &str) -> bool {
        self.pump
            .report(&ActivityReport::own_work(AgentActivity::Narration {
                summary: summary.into(),
            }))
    }
}

#[test]
fn activity_admission_is_timed_and_named() {
    let fixture = Fixture::new();
    let samples = fixture.clock.capture_phases();
    assert!(fixture.report("observed activity"));
    let captured = samples.snapshot();
    assert_eq!(
        captured["agent.activity"]["entered.lock"]["count"], 1,
        "activity admission must join frame timing: {captured}"
    );
    assert_eq!(
        fixture.clock.stats()["methods"]["agent.activity"]["served"],
        1
    );
}

#[test]
fn activity_replacement_refuses_the_old_session_before_persisting() {
    let fixture = Fixture::new();
    let before = {
        let mut app = fixture.state.lock().unwrap();
        app.session_registry.test_remove_tab(&fixture.key);
        app.runs[OWNER]
            .agents
            .by_id(&fixture.agent)
            .unwrap()
            .thread
            .clone()
    };
    assert!(!fixture.report("stale activity"));
    let app = fixture.state.lock().unwrap();
    assert_eq!(
        serde_json::to_value(&app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread)
            .unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(
        fixture.session.session_id().as_deref(),
        Some("activity-provider-session")
    );
}

#[test]
fn repeated_session_name_does_not_save_or_prepare_another_owner_candidate() {
    let fixture = Fixture::new();
    let instance = fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&fixture.key)
        .unwrap()
        .session_instance
        .clone()
        .unwrap();
    let name = || SelfReport {
        named: Some("activity-provider-session".into()),
        model: None,
        effort: None,
    };
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
    assert_eq!(
        captured["agent.deliver"]["entered.finisher.save"]["count"]
            .as_u64()
            .unwrap_or(0),
        0,
        "unchanged session name must not save the owner again: {captured}"
    );
    assert_eq!(
        captured["agent.deliver"]["entered.finisher.snapshot"]["count"]
            .as_u64()
            .unwrap_or(0),
        0
    );
}

#[tokio::test]
async fn activity_overflow_counts_loss_and_continues_with_retained_reports() {
    let fixture = Fixture::new();
    let (sender, receiver) = broadcast::channel(2);
    for index in 0..8 {
        sender
            .send(ActivityReport::own_work(AgentActivity::Narration {
                summary: format!("overflow witness {index}"),
            }))
            .unwrap();
    }
    spawn_activity_pump(&fixture.state, fixture.key.clone(), Some(receiver));
    wait_for(Duration::from_secs(5), || {
        let app = fixture.state.lock().unwrap();
        let thread = &app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread;
        serde_json::to_string(thread)
            .unwrap()
            .contains("overflow witness 7")
            .then_some(())
    })
    .await
    .expect("the retained tail must still publish after lag");
    assert_eq!(
        fixture.clock.stats()["activity_lagged_reports"],
        6,
        "overflow must be visible rather than silently pretending all reports persisted"
    );
}

#[test]
fn repeated_activity_saves_and_publishes_its_committed_owner_once() {
    let fixture = Fixture::new();
    assert!(fixture.report("warm provider identity"));
    let retained = fixture.state.lock().unwrap().read_view.load();
    let samples = fixture.clock.capture_phases();
    assert!(fixture.report("single activity commit"));
    let captured = samples.snapshot();
    assert_eq!(
        captured["agent.activity"]["entered.finisher.save"]["count"], 1,
        "{captured}"
    );
    assert_eq!(
        captured["agent.activity"]["entered.publication.install"]["count"], 1,
        "{captured}"
    );
    let app = fixture.state.lock().unwrap();
    let durable = app
        .store
        .as_ref()
        .unwrap()
        .thread_items(&fixture.agent)
        .unwrap();
    assert!(serde_json::to_string(&durable)
        .unwrap()
        .contains("single activity commit"));
    let published = app.read_view.load();
    assert!(serde_json::to_string(
        &published
            .history(&fixture.agent)
            .unwrap()
            .render(&json!({}))
    )
    .unwrap()
    .contains("single activity commit"));
    assert!(
        !serde_json::to_string(&retained.history(&fixture.agent).unwrap().render(&json!({})))
            .unwrap()
            .contains("single activity commit")
    );
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
    let durable = app
        .store
        .as_ref()
        .unwrap()
        .thread_items(&fixture.agent)
        .unwrap();
    assert!(!serde_json::to_string(&durable)
        .unwrap()
        .contains("rejected activity candidate"));
    let published = app.read_view.load();
    assert!(!serde_json::to_string(
        &published
            .history(&fixture.agent)
            .unwrap()
            .render(&json!({}))
    )
    .unwrap()
    .contains("rejected activity candidate"));
    assert!(
        !serde_json::to_string(&app.runs[OWNER].agents.by_id(&fixture.agent).unwrap().thread)
            .unwrap()
            .contains("rejected activity candidate")
    );
}

#[test]
fn matching_resume_name_does_not_bypass_any_session_instance_fence() {
    let fixture = Fixture::new();
    assert!(fixture.report("warm identity"));
    let mut app = fixture.state.lock().unwrap();
    let instance = app
        .session_registry
        .test_tab(&fixture.key)
        .unwrap()
        .session_instance
        .clone()
        .unwrap();
    for field in 0..5 {
        let mut stale = instance.clone();
        match field {
            0 => stale.id.push_str("-stale"),
            1 => stale.entity_id.push_str("-stale"),
            2 => stale.agent_id.push_str("-stale"),
            3 => stale.conversation_id.push_str("-stale"),
            _ => stale.checkout.push_str("-stale"),
        }
        app.note_self_report(
            OWNER,
            &fixture.agent,
            &stale,
            SelfReport {
                named: Some("activity-provider-session".into()),
                model: Some("stale-model".into()),
                effort: None,
            },
        );
        assert_ne!(
            app.runs[OWNER]
                .agents
                .by_id(&fixture.agent)
                .unwrap()
                .active_model
                .as_deref(),
            Some("stale-model")
        );
    }
}

#[test]
fn unchanged_session_name_still_commits_runtime_model_changes() {
    let fixture = Fixture::new();
    assert!(fixture.report("named lineage before model change"));
    let samples = fixture.clock.capture_phases();
    let timer = fixture.clock.frame("agent.deliver");
    {
        let mut app = timer.lock(&fixture.state);
        let instance = app
            .session_registry
            .test_tab(&fixture.key)
            .unwrap()
            .session_instance
            .clone()
            .unwrap();
        app.note_self_report(
            OWNER,
            &fixture.agent,
            &instance,
            SelfReport {
                named: Some("activity-provider-session".into()),
                model: Some("changed-runtime-model".into()),
                effort: Some("high".into()),
            },
        );
    }
    drop(timer);
    assert_eq!(
        samples.snapshot()["agent.deliver"]["entered.finisher.save"]["count"],
        1,
        "the repeated lineage name is free, but the changed model still commits once"
    );
    let app = fixture.state.lock().unwrap();
    let stored = app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == OWNER)
        .unwrap();
    let saved_agent = stored
        .agents
        .iter()
        .find(|agent| agent.id == fixture.agent)
        .unwrap();
    assert_eq!(
        saved_agent.active_model.as_deref(),
        Some("changed-runtime-model")
    );
    assert_eq!(saved_agent.active_effort.as_deref(), Some("high"));
    assert_eq!(
        saved_agent.resume_session_id.as_deref(),
        Some("activity-provider-session")
    );
    let root = app.read_view.load();
    let published = root.get_owned("agent.list", OWNER).unwrap();
    let published_agent = published["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"].as_str() == Some(fixture.agent.as_str()))
        .unwrap();
    assert_eq!(published_agent["active_model"], "changed-runtime-model");
    assert_eq!(published_agent["active_effort"], "high");
}

#[test]
fn unchanged_session_name_cannot_bypass_a_reset_reservation() {
    let fixture = Fixture::new();
    assert!(fixture.report("named lineage before reservation"));
    let mut app = fixture.state.lock().unwrap();
    let instance = app
        .session_registry
        .test_tab(&fixture.key)
        .unwrap()
        .session_instance
        .clone()
        .unwrap();
    // The lineage name is already current, but its addressed roster needs repair.
    app.record_agent_resume_id(OWNER, &fixture.agent, None);
    let before = app.read_view.load();
    let saved_before = app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == OWNER)
        .unwrap();
    app.resetting_conversations.insert(fixture.agent.clone());
    app.note_self_report(
        OWNER,
        &fixture.agent,
        &instance,
        SelfReport {
            named: Some("activity-provider-session".into()),
            model: Some("model-refused-during-reset".into()),
            effort: Some("high".into()),
        },
    );
    assert_eq!(app.recorded_resume_id(OWNER, &fixture.agent), None);
    assert_ne!(
        app.recorded_active_model(OWNER, &fixture.agent).as_deref(),
        Some("model-refused-during-reset")
    );
    let saved_after = app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == OWNER)
        .unwrap();
    assert_eq!(
        serde_json::to_value(saved_before).unwrap(),
        serde_json::to_value(saved_after).unwrap()
    );
    assert!(
        Arc::ptr_eq(&before, &app.read_view.load()),
        "the refused no-op name must publish neither roster repair nor model changes"
    );
}

#[test]
fn shared_conversation_activity_commits_history_without_claiming_alias_publication() {
    let fixture = Fixture::new();
    assert!(fixture.report("canonical activity before sharing"));
    let mut app = fixture.state.lock().unwrap();
    let repo = app.project_at(0).repo_path.clone();
    let alias_owner = adopted_run(
        &mut app,
        &repo,
        fixture._directory.path(),
        "activity-shared-alias",
    );
    let alias_agent = primary_agent_id(&app, &alias_owner);
    let mut active = app.take_run(&alias_owner).unwrap();
    active
        .agents
        .by_id_mut(&alias_agent)
        .unwrap()
        .bind_conversation(&fixture.agent);
    app.finish_run_mutation(alias_owner.clone(), active)
        .unwrap();
    let report = ActivityReport::own_work(AgentActivity::Narration {
        summary: "shared canonical activity".into(),
    });
    assert!(
        !record_activity(&mut app, &fixture.key, &alias_owner, &alias_agent, &report),
        "a canonical history save must leave the alias owner's fallback publication required"
    );
    let durable = app
        .store
        .as_ref()
        .unwrap()
        .thread_items(&fixture.agent)
        .unwrap();
    assert_eq!(
        durable
            .iter()
            .filter(|item| matches!(item,
        crate::thread::ThreadItem::Event(event)
            if event.summary.as_deref() == Some("shared canonical activity")))
            .count(),
        1
    );
    let root = app.read_view.load();
    let published = root.history(&fixture.agent).unwrap().render(&json!({}));
    assert_eq!(
        published["items"],
        serde_json::to_value(&durable).unwrap(),
        "the false alias receipt must not suppress the canonical committed history"
    );
    assert_eq!(
        app.agent_conversation(&alias_owner, Some(&alias_agent))
            .unwrap()
            .id,
        app.agent_conversation(OWNER, Some(&fixture.agent))
            .unwrap()
            .id
    );
}
