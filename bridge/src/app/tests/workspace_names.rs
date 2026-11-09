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
    assert!(
        f.record()["name_asked"].is_null(),
        "the request claim is not persisted"
    );
    let manifest = std::fs::read(f.root.join(crate::workspace::MANIFEST_FILE)).unwrap();
    f.say().unwrap();
    f.say().unwrap();
    let turns = f.log.turns();
    assert!(turns[0].contains("set_workspace_name"), "{turns:?}");
    assert!(
        turns[0].contains("set_name"),
        "both naming notes compose: {turns:?}"
    );
    assert!(!turns[1].contains("set_workspace_name"), "{turns:?}");
    assert!(f.record()["name_asked"].is_null());
    let mut app = f.state.lock().unwrap();
    app.workspaces.reload().unwrap();
    assert!(
        !app.workspaces.claim_name_request(&f.workspace),
        "a same-process reload keeps the memory claim"
    );
    assert_eq!(
        std::fs::read(f.root.join(crate::workspace::MANIFEST_FILE)).unwrap(),
        manifest,
        "asking never rewrites the manifest"
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
    assert!(f.record()["name_asked"].is_null());
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
    assert!(f.record()["name_asked"].is_null());
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
    assert!(f.record()["name_asked"].is_null());
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
    assert!(f.record()["name_asked"].is_null());
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
    assert!(f.record()["name_asked"].is_null());
}

#[test]
fn a_restart_asks_a_still_unnamed_workspace_again() {
    let mut f = NamingWorkspace::new("");
    let config = f._state_root.path().join("config.json");
    {
        let mut app = f.state.lock().unwrap();
        app.config_path = Some(config.clone());
        app.persist();
    }
    f.say().unwrap();
    assert!(f.log.turns()[0].contains("set_workspace_name"));
    let mut restarted = super::project_agent::rooted(f._state_root.path())
        .with_config(&config)
        .unwrap()
        .with_task_store(f._state_root.path().join("store"))
        .unwrap();
    f.log = SessionLog::default();
    insert_agent_tab(
        &mut restarted,
        &f.root,
        &f.owner,
        &f.agent,
        DictatedSession::reporting(AgentStatus::Waiting)
            .natively_accepting(ModelChoice::default())
            .recording_into(&f.log),
    );
    f.state = Arc::new(Mutex::new(restarted));
    assert_eq!(f.record()["waiting_for_name"], true);
    f.say().unwrap();
    f.say().unwrap();
    let turns = f.log.turns();
    assert!(turns[0].contains("set_workspace_name"), "{turns:?}");
    assert!(!turns[1].contains("set_workspace_name"), "{turns:?}");
}

#[test]
fn a_failure_before_the_send_keeps_the_workspace_name_ask() {
    let f = NamingWorkspace::new("");
    let (turn, timer, database) = {
        let mut app = f.state.lock().unwrap();
        app.edit_agent_conversation(&f.owner, &f.agent, |thread, _| {
            thread.post_user(
                "a legacy message before the send",
                None,
                "2026-11-09T12:00:00Z",
            );
            Ok(())
        })
        .unwrap();
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
                cold: "read the message".into(),
                warm: "read the message".into(),
            }),
            phase: "build",
            wants_catch_up: true,
            survives_refusal: false,
        };
        (
            turn,
            Arc::clone(&app.frame_clock).frame("test"),
            f._state_root.path().join("store/build.db"),
        )
    };
    let connection = rusqlite::Connection::open(database).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_workspace_delivery BEFORE UPDATE ON thread_items WHEN json_extract(NEW.item, '$.data.delivery_status') = 'submitted' BEGIN SELECT RAISE(ABORT, 'injected pre-send failure'); END;").unwrap();
    let error = crate::app::deliver(&f.state, &turn, &timer)
        .err()
        .expect("the injected pre-send bookkeeping failure is returned");
    assert!(error.contains("injected pre-send failure"), "{error}");
    assert!(f.log.turns().is_empty());
    assert_eq!(f.record()["waiting_for_name"], true);
    connection
        .execute_batch("DROP TRIGGER refuse_workspace_delivery")
        .unwrap();
    assert!(matches!(
        crate::app::deliver(&f.state, &turn, &timer).unwrap(),
        DeliveryOutcome::Delivered(_)
    ));
    assert!(f.log.turns()[0].contains("set_workspace_name"));
}

#[test]
fn an_unnamed_workspace_turn_is_not_deferred_while_another_workspace_is_busy() {
    let f = NamingWorkspace::new("");
    {
        let mut app = f.state.lock().unwrap();
        app.with_inline_deferred(|app| {
            app.workspace_create(
                &json!({"project_id":f.project,"name":"Other work","isolation":"worktree"}),
            )
        })
        .unwrap();
        assert!(
            app.deferred_work.is_some(),
            "another workspace has a real pending filesystem job"
        );
    }
    f.say().unwrap();
    assert!(f.log.turns()[0].contains("set_workspace_name"));
    // Completing the other job reloads manifests. It must retain this process's
    // memory claim so a follow-up still does not ask again.
    let mut work = f.state.lock().unwrap().take_deferred().unwrap();
    loop {
        let done = work.run();
        match f
            .state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.create", &Value::Null, done)
        {
            DeferredNext::Answered(answer) => {
                answer.unwrap();
                break;
            }
            DeferredNext::Again(next) => work = next,
        }
    }
    f.say().unwrap();
    assert!(!f.log.turns()[1].contains("set_workspace_name"));
}

#[test]
fn an_unnamed_workspace_label_uses_eight_id_characters() {
    let f = NamingWorkspace::new("");
    assert_eq!(
        f.record()["name"],
        format!("Workspace {}", &f.workspace[..8])
    );
}

#[test]
fn a_workspace_name_claim_uses_memory_without_filesystem_probes() {
    let f = NamingWorkspace::new("");
    let manifest = std::fs::read(f.root.join(crate::workspace::MANIFEST_FILE)).unwrap();
    let mut app = f.state.lock().unwrap();
    let (claimed, probes) = crate::app::workspaces::count_path_canonicalizations(|| {
        app.claim_workspace_name_request(&f.owner, &f.agent)
    });
    assert_eq!(claimed, Some(f.workspace.clone()));
    assert_eq!(probes, 0, "claiming must use retained workspace roots");
    app.workspaces.release_name_request(&f.workspace);
    assert_eq!(
        std::fs::read(f.root.join(crate::workspace::MANIFEST_FILE)).unwrap(),
        manifest
    );
}

#[test]
fn the_naming_tool_names_the_workspace_that_was_asked() {
    let f = NamingWorkspace::new("");
    let named = {
        let mut app = f.state.lock().unwrap();
        let named = app.workspaces.adopt_root(
            &f.project,
            "named-adopted-row".into(),
            "A named adopted row".into(),
            f.root.clone(),
            "shared-root".into(),
            false,
        );
        assert_eq!(
            app.workspaces.list(Some(&f.project))[0].id,
            named.id,
            "the named row sorts first"
        );
        assert_eq!(
            app.workspace_of_conversation(&f.owner),
            Some(named.id.clone())
        );
        assert_eq!(
            app.claim_workspace_name_request(&f.owner, &f.agent),
            Some(f.workspace.clone()),
            "the unnamed managed row is asked"
        );
        named
    };
    f.name("The actual workspace").unwrap();
    let app = f.state.lock().unwrap();
    let renamed = app.workspaces.get(&f.workspace).unwrap();
    assert_eq!(renamed.name, "The actual workspace");
    assert!(!renamed.waiting_for_name);
    assert_eq!(
        app.workspaces.get(&named.id).unwrap(),
        &named,
        "the earlier named row is unchanged"
    );
}
