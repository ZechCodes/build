use super::project_agent::{project_agent, workspace};
use super::*;
use crate::mcp::DoneServer;

struct NamingWorkspace {
    _home: tempfile::TempDir,
    _state_root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    project: String,
    workspace: String,
    owner: String,
    agent: String,
    root: PathBuf,
    log: SessionLog,
}
impl NamingWorkspace {
    fn new(name: &str) -> Self {
        let state_root = tempfile::tempdir().unwrap();
        let (home, mut app, project) = super::tracker::tracked(state_root.path());
        let workspace = workspace(&mut app, &project, name);
        let result = app.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id":workspace}),
        ));
        assert_eq!(result["ok"], true, "{result}");
        let owner = result["result"]["run_id"].as_str().unwrap().to_string();
        let added = app.handle(req("agent.add", json!({"entity_id":owner})));
        assert_eq!(added["ok"], true, "{added}");
        let agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
        app.set_agent_model_choice(&owner, &agent, ModelChoice::default())
            .unwrap();
        let root = app.workspaces.get(&workspace).unwrap().root.clone();
        let log = SessionLog::default();
        insert_agent_tab(
            &mut app,
            &root,
            &owner,
            &agent,
            DictatedSession::reporting(AgentStatus::Waiting)
                .natively_accepting(ModelChoice::default())
                .recording_into(&log),
        );
        Self {
            _home: home,
            _state_root: state_root,
            state: Arc::new(Mutex::new(app)),
            project,
            workspace,
            owner,
            agent,
            root,
            log,
        }
    }
    fn record(&self) -> Value {
        serde_json::to_value(
            self.state
                .lock()
                .unwrap()
                .workspaces
                .get(&self.workspace)
                .unwrap(),
        )
        .unwrap()
    }
    fn say(&self) -> Result<(String, Spawned), String> {
        deliver(
            &self.state,
            &self.root,
            &self.owner,
            &self.agent,
            &ModelChoice::default(),
            "build",
            ["do the task", "do the task"],
        )
    }
    fn name(&self, name: &str) -> Result<Value, String> {
        let server = DoneServer::for_owner(&self.agent);
        let parsed = server.handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_workspace_name","arguments":{"name":name}}}).to_string());
        let Some(action) = parsed.action else {
            return Err(parsed.reply.unwrap());
        };
        self.state
            .lock()
            .unwrap()
            .on_agent_mcp_action(&self.owner, &self.agent, action)
    }
}

#[test]
fn the_first_turn_in_an_unnamed_workspace_asks_for_its_name_once() {
    let f = NamingWorkspace::new("");
    assert_eq!(f.record()["waiting_for_name"], true);
    assert_eq!(
        f.record()["name_asked"],
        false,
        "no agent was sent a turn at creation"
    );
    f.say().unwrap();
    f.say().unwrap();
    let turns = f.log.turns();
    assert!(turns[0].contains("set_workspace_name"), "{turns:?}");
    assert!(
        turns[0].contains("set_name"),
        "both naming notes compose: {turns:?}"
    );
    assert!(!turns[1].contains("set_workspace_name"), "{turns:?}");
    assert_eq!(f.record()["name_asked"], true);
    let mut app = f.state.lock().unwrap();
    app.workspaces.reload().unwrap();
    assert_eq!(
        serde_json::to_value(app.workspaces.get(&f.workspace).unwrap()).unwrap()["name_asked"],
        true
    );
}

#[test]
fn a_second_agent_is_not_asked_again() {
    let mut f = NamingWorkspace::new("");
    f.say().unwrap();
    {
        let mut app = f.state.lock().unwrap();
        let added = app.handle(req("agent.add", json!({"entity_id":f.owner})));
        f.agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
        app.set_agent_model_choice(&f.owner, &f.agent, ModelChoice::default())
            .unwrap();
        f.log = SessionLog::default();
        insert_agent_tab(
            &mut app,
            &f.root,
            &f.owner,
            &f.agent,
            DictatedSession::reporting(AgentStatus::Waiting)
                .natively_accepting(ModelChoice::default())
                .recording_into(&f.log),
        );
    }
    f.say().unwrap();
    assert!(!f.log.turns()[0].contains("set_workspace_name"));
    assert_eq!(f.record()["name_asked"], true);
}

#[test]
fn the_project_coordinator_is_never_asked_to_name_a_workspace() {
    let f = NamingWorkspace::new("");
    let (owner, agent, root, log) = {
        let mut app = f.state.lock().unwrap();
        let (owner, agent) = project_agent(&mut app, &f.project);
        app.set_agent_model_choice(&owner, &agent, ModelChoice::default())
            .unwrap();
        let root = app.runs[&owner].worktree.path.clone();
        let log = SessionLog::default();
        insert_agent_tab(
            &mut app,
            &root,
            &owner,
            &agent,
            DictatedSession::reporting(AgentStatus::Waiting)
                .natively_accepting(ModelChoice::default())
                .recording_into(&log),
        );
        (owner, agent, root, log)
    };
    deliver(
        &f.state,
        &root,
        &owner,
        &agent,
        &ModelChoice::default(),
        "build",
        ["do the task", "do the task"],
    )
    .unwrap();
    assert!(!log.turns()[0].contains("set_workspace_name"));
    assert_eq!(f.record()["name_asked"], false);
}

struct RejectedWrite;
impl AgentSession for RejectedWrite {
    fn accepts_turn_choice(&self) -> bool {
        true
    }
    fn turn_choice_support(&self, _: &ModelChoice) -> crate::harness::TurnChoiceSupport {
        crate::harness::TurnChoiceSupport::Native
    }
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Err(HarnessError::Unsupported("rejected write".into()))
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        true
    }
    fn end(&self) {}
    fn backdate_last_output(&self, _: Duration) {}
}

#[test]
fn a_rejected_write_keeps_the_workspace_name_ask() {
    let f = NamingWorkspace::new("");
    insert_agent_tab(
        &mut f.state.lock().unwrap(),
        &f.root,
        &f.owner,
        &f.agent,
        RejectedWrite,
    );
    assert!(f.say().is_err());
    assert_eq!(f.record()["waiting_for_name"], true);
    assert_eq!(f.record()["name_asked"], false);
    insert_agent_tab(
        &mut f.state.lock().unwrap(),
        &f.root,
        &f.owner,
        &f.agent,
        DictatedSession::reporting(AgentStatus::Waiting)
            .natively_accepting(ModelChoice::default())
            .recording_into(&f.log),
    );
    f.say().unwrap();
    assert!(f.log.turns()[0].contains("set_workspace_name"));
}

#[test]
fn set_workspace_name_renames_the_callers_workspace_and_clears_the_wait() {
    let f = NamingWorkspace::new("");
    let before = f.record();
    f.name("  Fix   workspace  names ").unwrap();
    let after = f.record();
    assert_eq!(after["name"], "Fix workspace names");
    assert_eq!(after["waiting_for_name"], false);
    assert_eq!(after["root"], before["root"]);
    assert_eq!(after["directories"], before["directories"]);
    let mut app = f.state.lock().unwrap();
    app.workspaces.reload().unwrap();
    assert_eq!(
        app.workspaces.get(&f.workspace).unwrap().name,
        "Fix workspace names"
    );
}

#[test]
fn set_workspace_name_refuses_a_named_workspace() {
    let f = NamingWorkspace::new("Person named this");
    assert!(f
        .name("Overwrite label")
        .unwrap_err()
        .contains("already named"));
    assert_eq!(f.record()["name"], "Person named this");
}

#[test]
fn set_workspace_name_refuses_outside_a_workspace() {
    let f = NamingWorkspace::new("");
    let mut app = f.state.lock().unwrap();
    let (owner, _) = project_agent(&mut app, &f.project);
    let added = app.handle(req("agent.add", json!({"entity_id":owner,"name":"Worker"})));
    let agent = added["result"]["agent"]["id"].as_str().unwrap();
    let action =
        serde_json::from_value(json!({"action":"set_workspace_name","name":"Own workspace"}))
            .expect("the tool action exists");
    assert!(app
        .on_agent_mcp_action(&owner, agent, action)
        .unwrap_err()
        .contains("not in a workspace"));
}

#[test]
fn set_workspace_name_refuses_blank_and_oversized_names() {
    let f = NamingWorkspace::new("");
    for name in ["".to_string(), "  ".into(), "é".repeat(41)] {
        assert!(f.name(&name).is_err());
        assert_eq!(f.record()["waiting_for_name"], true);
    }
    // The authenticated dispatcher must validate too, not just tools/call.
    let mut app = f.state.lock().unwrap();
    let action = serde_json::from_value(json!({"action":"set_workspace_name","name":"  "}))
        .expect("the tool action exists");
    assert!(app.on_agent_mcp_action(&f.owner, &f.agent, action).is_err());
}

#[test]
fn set_workspace_name_while_busy_leaves_the_wait() {
    let f = NamingWorkspace::new("");
    f.state.lock().unwrap().active_deferred_filesystem_jobs += 1;
    assert!(f
        .name("Retry later")
        .unwrap_err()
        .contains(crate::reclaim::BUSY));
    assert_eq!(f.record()["waiting_for_name"], true);
    f.state.lock().unwrap().active_deferred_filesystem_jobs -= 1;
    f.name("Retry later").unwrap();
}

#[test]
fn a_client_rename_clears_the_wait_and_the_ask() {
    let f = NamingWorkspace::new("");
    let renamed = f.state.lock().unwrap().handle(req(
        "workspace.rename",
        json!({"workspace_id":f.workspace,"name":"Person chose this"}),
    ));
    assert_eq!(renamed["ok"], true, "{renamed}");
    assert_eq!(f.record()["waiting_for_name"], false);
    f.say().unwrap();
    assert!(!f.log.turns()[0].contains("set_workspace_name"));
    assert!(f
        .name("Overwrite label")
        .unwrap_err()
        .contains("already named"));
}

#[test]
fn a_provider_command_leaves_the_workspace_request_for_a_real_turn() {
    let f = NamingWorkspace::new("");
    deliver(
        &f.state,
        &f.root,
        &f.owner,
        &f.agent,
        &ModelChoice::default(),
        "build",
        ["/compact", "/compact"],
    )
    .unwrap();
    assert_eq!(f.log.turns(), vec!["/compact"]);
    assert_eq!(f.record()["name_asked"], false);
    f.say().unwrap();
    assert!(f.log.turns()[1].contains("set_workspace_name"));
}

struct RenameDuringRejectedWrite {
    state: Arc<Mutex<AppState>>,
    workspace: String,
}
impl AgentSession for RenameDuringRejectedWrite {
    fn accepts_turn_choice(&self) -> bool {
        true
    }
    fn turn_choice_support(&self, _: &ModelChoice) -> crate::harness::TurnChoiceSupport {
        crate::harness::TurnChoiceSupport::Native
    }
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        self.state
            .lock()
            .unwrap()
            .workspace_rename(&json!({"workspace_id":self.workspace,"name":"Person wins"}))
            .unwrap();
        Err(HarnessError::Unsupported(
            "rejected after client rename".into(),
        ))
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        true
    }
    fn end(&self) {}
    fn backdate_last_output(&self, _: Duration) {}
}

#[test]
fn a_client_rename_during_a_rejected_write_does_not_restore_the_wait() {
    let f = NamingWorkspace::new("");
    insert_agent_tab(
        &mut f.state.lock().unwrap(),
        &f.root,
        &f.owner,
        &f.agent,
        RenameDuringRejectedWrite {
            state: Arc::clone(&f.state),
            workspace: f.workspace.clone(),
        },
    );
    assert!(f.say().is_err());
    assert_eq!(f.record()["name"], "Person wins");
    assert_eq!(f.record()["waiting_for_name"], false);
    assert_eq!(f.record()["name_asked"], false);
}

#[test]
fn a_first_workspace_turn_waits_for_in_flight_manifest_writers() {
    let f = NamingWorkspace::new("");
    let (turn, timer) = {
        let mut app = f.state.lock().unwrap();
        app.active_deferred_filesystem_jobs += 1;
        let agent = app
            .entity_agents(&f.owner)
            .unwrap()
            .by_id(&f.agent)
            .unwrap();
        let turn = PendingAgentTurn {
            operation_id: None,
            root: f.root.clone(),
            owner: f.owner.clone(),
            agent_id: f.agent.clone(),
            conversation_id: agent.conversation_id().to_string(),
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt: false,
            say: Some(TurnText {
                attachments: Default::default(),
                kind: None,
                cold: "do the task".into(),
                warm: "do the task".into(),
            }),
            phase: "build",
            wants_catch_up: false,
            survives_refusal: false,
        };
        (turn, Arc::clone(&app.frame_clock).frame("test"))
    };
    assert!(matches!(
        crate::app::deliver(&f.state, &turn, &timer).unwrap(),
        DeliveryOutcome::Deferred
    ));
    assert!(f.log.turns().is_empty());
    assert_eq!(f.record()["name_asked"], false);
    f.state.lock().unwrap().active_deferred_filesystem_jobs -= 1;
    assert!(matches!(
        crate::app::deliver(&f.state, &turn, &timer).unwrap(),
        DeliveryOutcome::Delivered(_)
    ));
    assert!(f.log.turns()[0].contains("set_workspace_name"));
}
