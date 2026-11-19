//! The project coordinator changes the same saved choice the model picker uses.

use super::*;
use crate::app::tests::project_agent::{project_agent, workspace};
use crate::harness::adk::AdkSession;
use crate::harness::installed::{CliProbe, CliReading, ListedModel, Readings};
use crate::harness::SessionStatusSnapshot;
use crate::mcp::BridgeAction;

struct ModelTool {
    state_root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    app: Arc<Mutex<AppState>>,
    project_id: String,
    caller_owner: String,
    caller: String,
    owner: String,
    worker: String,
    log: SessionLog,
}

fn choice(provider: AgentProvider, model: &str, effort: Option<&str>) -> ModelChoice {
    ModelChoice {
        provider,
        model: Some(model.into()),
        effort: effort.map(str::to_owned),
    }
}

impl ModelTool {
    fn new(initial: ModelChoice) -> Self {
        let state_root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project_id) = crate::app::tests::tracker::tracked(state_root.path());
        let (caller_owner, caller) = project_agent(&mut app, &project_id);
        let workspace_id = workspace(&mut app, &project_id, "model-tool");
        let owner = app.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id": workspace_id}),
        ))["result"]["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let worker = app.handle(req(
            "agent.add",
            json!({"entity_id": owner, "provider": initial.provider,
                   "model": initial.model, "effort": initial.effort}),
        ))["result"]["agent"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            state_root,
            _repo,
            app: app.shared(),
            project_id,
            caller_owner,
            caller,
            owner,
            worker,
            log: SessionLog::default(),
        }
    }

    fn open(&self, session: DictatedSession) -> crate::thread::SessionInstance {
        let mut app = self.app.lock().unwrap();
        let root = AppState::canonical_root(&app.entity_agent_root(&self.owner).unwrap());
        let active_model = session.active_model().or_else(|| {
            self.choice_digest(&app)["model"]
                .as_str()
                .map(str::to_owned)
        });
        insert_agent_tab(
            &mut app,
            &root,
            &self.owner,
            &self.worker,
            session.recording_into(&self.log).interruptible(),
        );
        let instance = self.instance(&app);
        app.record_agent_active_model(&self.owner, &self.worker, active_model);
        instance
    }

    async fn open_adk(&self, initial: &ModelChoice) -> Arc<dyn AgentSession> {
        let spec = HarnessSpec::new("sh").arg("-c").arg(r#"
            printf '%s\n' '{"type":"system","subtype":"init","session_id":"adk-model-tools","model":"claude-opus-5"}'
            IFS= read -r first || exit 0
            printf '%s\n' '{"type":"system","subtype":"task_started","task_id":"monitor","description":"Persistent monitor"}'
            while [ ! -e "$1/close-turn" ]; do sleep 0.02; done
            printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
            while IFS= read -r line; do
                printf '%s\n' "$line" >> "$1/delivered"
                case "$line" in
                    *'"subtype":"set_model"'*)
                        request=${line#*'"request_id":"'}
                        request=${request%%\"*}
                        printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s"}}\n' "$request"
                        ;;
                esac
            done
        "#).arg("adk-model-tools").arg(self.state_root.path().to_str().unwrap());
        let (session, _activity) = AdkSession::spawn(&spec, None, initial).unwrap();
        wait_for(Duration::from_secs(5), || {
            (session.status() == AgentStatus::Waiting).then_some(())
        })
        .await
        .expect("ADK fixture initialized");
        let (key, worker, instance) = {
            let mut app = self.app.lock().unwrap();
            let root = AppState::canonical_root(&app.entity_agent_root(&self.owner).unwrap());
            let key = insert_agent_tab(&mut app, &root, &self.owner, &self.worker, session);
            let worker = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
            (key, worker, self.instance(&app))
        };
        spawn_status_pump(
            &self.app,
            key,
            Arc::clone(&worker),
            Some(instance),
            worker.status_changed(),
        );
        worker
    }

    fn instance(&self, app: &AppState) -> crate::thread::SessionInstance {
        app.agent_conversation(&self.owner, Some(&self.worker))
            .unwrap()
            .open_session_instance(&self.worker)
            .unwrap()
    }

    fn choice_digest(&self, app: &AppState) -> Value {
        app.agent_digests(&self.owner, DigestScope::List)
            .into_iter()
            .find(|agent| agent["id"] == self.worker)
            .unwrap()
    }

    fn set(&self, model: &str, effort: Option<&str>) -> Result<Value, String> {
        self.set_for(&self.worker, model, effort)
    }

    fn set_for(&self, agent_id: &str, model: &str, effort: Option<&str>) -> Result<Value, String> {
        let action: BridgeAction = serde_json::from_value(json!({
            "action":"set_agent_model", "agent_id":agent_id, "model":model, "effort":effort
        }))
        .expect("the socket understands set_agent_model");
        self.app
            .lock()
            .unwrap()
            .agent_action(&self.caller_owner, &self.caller, action)
    }

    fn queue_next_turn(&self) {
        let answer = self.app.lock().unwrap().handle(req(
            "thread.post",
            json!({"entity_id":self.owner, "agent_id":self.worker,
                   "conversation_id": self.worker,
                   "operation_id":"model-tool-next-turn", "body":"Continue this conversation"}),
        ));
        assert_eq!(answer["ok"], true, "{answer}");
    }

    fn drain(&self) {
        DeliveryRunner::drain(&self.app, &a_frame(&self.app));
    }
}

#[test]
fn set_agent_model_between_turns_persists_the_picker_choice_and_uses_the_same_session() {
    let provider = AgentProvider::CodexAppServer;
    let fixture = ModelTool::new(choice(provider, "gpt-6-sol", Some("low")));
    let next = choice(provider, "gpt-6-astra", Some("high"));
    let instance = fixture
        .open(DictatedSession::reporting(AgentStatus::Waiting).natively_accepting(next.clone()));
    let before = fixture.choice_digest(&fixture.app.lock().unwrap());

    let answer = fixture.set("gpt-6-astra", Some("high")).unwrap();

    assert_eq!(answer["deferred"], false);
    let pending = fixture.choice_digest(&fixture.app.lock().unwrap());
    assert_eq!(pending["model"], "gpt-6-astra");
    assert_eq!(pending["effort"], "high");
    assert_eq!(pending["active_model"], before["active_model"]);
    assert_eq!(pending["conversation_id"], before["conversation_id"]);
    assert_eq!(
        pending["choice_revision"].as_u64().unwrap(),
        before["choice_revision"].as_u64().unwrap() + 1
    );
    assert!(fixture.log.turns().is_empty());

    fixture.queue_next_turn();
    fixture.drain();
    assert_eq!(fixture.log.choices(), vec![Some(next)]);
    assert_eq!(fixture.instance(&fixture.app.lock().unwrap()), instance);
}

#[test]
fn set_agent_model_mid_turn_leaves_current_turn_alone_and_applies_on_the_next() {
    let provider = AgentProvider::ClaudeAdk;
    let next = choice(provider, "claude-opus-5", Some("high"));
    let fixture = ModelTool::new(next.clone());
    let (status, updates) =
        tokio::sync::watch::channel(SessionStatusSnapshot::new(AgentStatus::Working));
    let instance = fixture.open(
        DictatedSession::reporting(AgentStatus::Working)
            .watching_status(updates)
            .announcing_model("claude-opus-5-5")
            .applying_model_before_prompt()
            .natively_accepting(next.clone()),
    );

    assert_eq!(
        fixture.choice_digest(&fixture.app.lock().unwrap())["active_model"],
        "claude-opus-5-5"
    );

    // Returning to the launch model also needs the current turn to end.
    let answer = fixture.set("claude-opus-5", None).unwrap();

    assert_eq!(answer["deferred"], true);
    assert_eq!(answer["effort"], "high");
    fixture.queue_next_turn();
    fixture.drain();
    assert!(
        fixture.log.turns().is_empty(),
        "the current turn is still open"
    );
    assert!(!fixture.log.interrupted());

    status
        .send(SessionStatusSnapshot::new(AgentStatus::Waiting))
        .unwrap();
    fixture.drain();
    assert_eq!(fixture.log.choices(), vec![Some(next)]);
    assert_eq!(fixture.instance(&fixture.app.lock().unwrap()), instance);
}

#[tokio::test]
async fn set_agent_model_releases_a_turn_that_ends_before_the_deferred_delivery_requeues() {
    use crate::app::runtime::delivery::runner::settlement_probe::Gate;

    let provider = AgentProvider::ClaudeAdk;
    let fixture = ModelTool::new(choice(provider, "claude-opus-5", None));
    let next = choice(provider, "claude-opus-5-5", None);
    let (status, updates) =
        tokio::sync::watch::channel(SessionStatusSnapshot::new(AgentStatus::Working));
    fixture.open(
        DictatedSession::reporting(AgentStatus::Working)
            .watching_status(updates)
            .applying_model_before_prompt()
            .natively_accepting(next.clone()),
    );
    fixture.set("claude-opus-5-5", None).unwrap();

    let mut gate = Gate::install(&fixture.app, "model-tool-next-turn");
    fixture.queue_next_turn();
    fixture.drain();
    gate.entered();
    assert!(fixture.log.turns().is_empty());
    status
        .send(SessionStatusSnapshot::new(AgentStatus::Waiting))
        .unwrap();
    gate.release();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.log.choices().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn was not retried"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(fixture.log.choices(), vec![Some(next)]);
}

#[tokio::test]
async fn set_agent_model_adk_delivers_when_a_turn_closes_over_a_background_task() {
    let provider = AgentProvider::ClaudeAdk;
    let initial = choice(provider, "claude-opus-5", Some("high"));
    let next = choice(provider, "claude-opus-5-5", Some("high"));
    let fixture = ModelTool::new(initial.clone());
    let session = fixture.open_adk(&initial).await;
    session.send_turn(&Turn::new("start a monitor")).unwrap();

    assert!(session.waits_for_turn_choice(&next));
    assert!(!session.waits_for_turn_choice(&initial));
    assert_eq!(
        fixture.set("claude-opus-5-5", None).unwrap()["deferred"],
        true
    );
    fixture.queue_next_turn();
    fixture.drain();
    wait_for(Duration::from_secs(5), || {
        let app = fixture.app.lock().unwrap();
        let held = app
            .delivery_queue
            .queued()
            .any(|turn| turn.operation_id.as_deref() == Some("model-tool-next-turn"));
        held.then_some(())
    })
    .await
    .expect("the open turn holds the delivery");

    std::fs::write(fixture.state_root.path().join("close-turn"), "").unwrap();
    let delivered = wait_for(Duration::from_secs(5), || {
        let written = std::fs::read_to_string(fixture.state_root.path().join("delivered")).ok()?;
        (written.lines().count() >= 2).then_some(written)
    })
    .await
    .expect("the real status pump wakes delivery even while the monitor remains active");

    let lines: Vec<Value> = delivered
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        lines[0]["request"],
        json!({"subtype":"set_model","model":"claude-opus-5-5"})
    );
    assert_eq!(lines[1]["type"], "user");
    assert!(lines[1]["message"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Continue this conversation"));
    wait_for(Duration::from_secs(5), || {
        (session.active_model().as_deref() == next.model.as_deref()).then_some(())
    })
    .await
    .expect("the real ADK session observes the acknowledged model change");
    assert_eq!(session.status(), AgentStatus::Working);
    session.end();
}

#[tokio::test]
async fn set_agent_model_adk_omitted_effort_can_select_an_effortless_model() {
    let provider = AgentProvider::ClaudeAdk;
    let initial = choice(provider, "claude-opus-5", Some("high"));
    let fixture = ModelTool::new(initial.clone());
    let session = fixture.open_adk(&initial).await;

    let response = fixture.set("claude-haiku-4-5", None).unwrap();

    assert_eq!(response["model"], "claude-haiku-4-5");
    assert!(response["effort"].is_null());
    let next = choice(provider, "claude-haiku-4-5", None);
    assert_eq!(
        session.turn_choice_support(&next),
        crate::harness::TurnChoiceSupport::Native
    );
    session.end();
}

struct CodexOffer;

impl CliProbe for CodexOffer {
    fn read(&self, binary: &str) -> CliReading {
        assert_eq!(binary, "codex");
        CliReading {
            version: None,
            listed: Some(vec![ListedModel {
                id: "gpt-6-sol".into(),
                label: "GPT-6-Sol".into(),
                hidden: false,
                efforts: vec!["low".into()],
            }]),
        }
    }
}

#[test]
fn set_agent_model_refuses_invalid_targets_models_and_efforts() {
    let provider = AgentProvider::CodexAppServer;
    let fixture = ModelTool::new(choice(provider, "gpt-6-sol", Some("low")));
    fixture.open(
        DictatedSession::reporting(AgentStatus::Waiting).natively_accepting(choice(
            provider,
            "gpt-6-sol",
            Some("low"),
        )),
    );
    let outside = {
        let mut app = fixture.app.lock().unwrap();
        app.cli_readings = Readings::answering_inline(&CodexOffer);
        let source = init_repo_named(fixture.state_root.path(), "outside-model-tool");
        let project = crate::app::tests::project_agent::added_project(&mut app, &source);
        assert_ne!(project, fixture.project_id);
        project_agent(&mut app, &project).1
    };

    for (agent, model, effort, error) in [
        ("agent-missing", "gpt-6-sol", None, "unknown agent_id"),
        (&outside, "gpt-6-sol", None, "is not in project"),
        (&fixture.worker, "gpt-6-astra", None, "does not offer it"),
        (&fixture.worker, "claude-opus-5", None, "does not offer it"),
        (
            &fixture.worker,
            "gpt-6-sol",
            Some("high"),
            "does not support effort high",
        ),
    ] {
        let refused = fixture.set_for(agent, model, effort).unwrap_err();
        assert!(refused.contains(error), "{refused}");
    }
}

#[test]
fn set_agent_model_refuses_changes_requiring_a_new_session() {
    for (provider, initial, model) in [
        (AgentProvider::Claude, "claude-opus-5", "claude-opus-5-5"),
        (AgentProvider::Codex, "gpt-6-sol", "gpt-6-astra"),
        (AgentProvider::Pi, "gpt-6-sol", "gpt-6-astra"),
    ] {
        let fixture = ModelTool::new(choice(provider, initial, None));
        let instance = fixture.open(DictatedSession::reporting(AgentStatus::Waiting));
        let before = fixture.choice_digest(&fixture.app.lock().unwrap());
        let error = fixture.set(model, None).unwrap_err();
        assert!(error.contains("cannot change model in place"), "{error}");
        let app = fixture.app.lock().unwrap();
        assert_eq!(fixture.choice_digest(&app), before);
        assert_eq!(fixture.instance(&app), instance);
    }
}

#[test]
fn set_agent_model_refuses_a_different_effort_on_claude_adk() {
    let provider = AgentProvider::ClaudeAdk;
    let fixture = ModelTool::new(choice(provider, "claude-opus-5", Some("high")));
    let instance = fixture.open(
        DictatedSession::reporting(AgentStatus::Waiting).natively_accepting(choice(
            provider,
            "claude-opus-5-5",
            Some("high"),
        )),
    );
    let before = fixture.choice_digest(&fixture.app.lock().unwrap());

    let error = fixture.set("claude-opus-5-5", Some("low")).unwrap_err();

    assert!(error.contains("keeps its launch effort"), "{error}");
    let app = fixture.app.lock().unwrap();
    assert_eq!(fixture.choice_digest(&app), before);
    assert_eq!(fixture.instance(&app), instance);
    assert!(fixture.log.turns().is_empty());
}

#[test]
fn set_agent_model_refuses_a_session_that_is_no_longer_running() {
    let provider = AgentProvider::CodexAppServer;
    let fixture = ModelTool::new(choice(provider, "gpt-6-sol", Some("low")));
    fixture.open(
        DictatedSession::reporting(AgentStatus::Ended { code: Some(0) })
            .natively_accepting(choice(provider, "gpt-6-astra", Some("low"))),
    );

    let error = fixture.set("gpt-6-astra", None).unwrap_err();

    assert!(error.contains("no running session"), "{error}");
}
