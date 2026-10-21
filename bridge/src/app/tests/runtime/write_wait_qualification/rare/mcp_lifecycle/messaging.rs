use super::*;

pub(super) async fn project_worker(context: &mut ProjectFixture) {
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::AddProjectAgent {
                notify_user: Some(false),
                harness: Some("claude".into()),
                model: None,
                effort: None,
                name: Some("MCP project worker".into()),
                role: None,
                capability: None,
            },
        )
        .await;
    let result = &proof.reply["result"];
    let worker = result["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(result["entity_id"], context.caller.owner);
    assert_eq!(result["agent"]["project_role"], "worker");
    assert_eq!(result["agent"]["created_by"], context.caller.agent);
    assert_eq!(result["agent"]["watched"], false);
    let app = context.fixture.state().lock().unwrap();
    assert_eq!(
        app.project_pin(&context.caller.owner),
        Some(context.caller.agent.as_str())
    );
    assert!(app
        .entity_agents(&context.caller.owner)
        .unwrap()
        .by_id(&worker)
        .is_some());
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .iter()
        .any(|run| run.id == context.caller.owner
            && run.agents.iter().any(|agent| agent.id == worker)));
    drop(app);
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"project_worker":worker,"owner":context.caller.owner,"pin_unchanged":true})).await;
}

pub(super) async fn workspace_worker(context: &mut ProjectFixture, workspace: &str) {
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: workspace.into(),
                notify_user: Some(false),
                harness: Some("claude".into()),
                model: None,
                effort: None,
                name: Some("MCP workspace worker".into()),
                role: None,
                capability: None,
            },
        )
        .await;
    let entity = proof.reply["result"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string();
    let worker = proof.reply["result"]["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(proof.reply["result"]["workspace_id"], workspace);
    assert_eq!(
        proof.reply["result"]["agent"]["created_by"],
        context.caller.agent
    );
    assert_eq!(proof.reply["result"]["agent"]["watched"], false);
    assert_eq!(workspace_row(context, workspace)["entity_id"], entity);
    let app = context.fixture.state().lock().unwrap();
    assert!(app.entity_agents(&entity).unwrap().by_id(&worker).is_some());
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .iter()
        .any(|run| run.id == entity && run.agents.iter().any(|agent| agent.id == worker)));
    drop(app);
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"workspace":workspace,"worker":worker,"owner":entity})).await;

    let warm = WarmTarget::install(context, &entity, &worker);
    message_workspace_agent(context, workspace, &entity, &worker, &warm).await;
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::RemoveWorkspaceAgent {
                workspace_id: workspace.into(),
                agent_id: worker.clone(),
            },
        )
        .await;
    assert_eq!(proof.reply["result"]["agent_id"], worker);
    let roster = read(&context.fixture, "agent.list", json!({"entity_id":entity}));
    assert!(!roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == worker));
    let app = context.fixture.state().lock().unwrap();
    assert!(app.entity_agents(&entity).unwrap().by_id(&worker).is_none());
    assert!(!app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .iter()
        .any(|run| run.id == entity && run.agents.iter().any(|agent| agent.id == worker)));
    drop(app);
    assert!(
        warm.log.ended(),
        "actual remove_workspace_agent must end its warm session"
    );
    assert!(context
        .fixture
        .state()
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&warm.key)
        .is_none());
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"removed_worker":worker,"owner":entity,"workspace_preserved":workspace})).await;
}

/// An already-live, lineage-matched session; no provider process is opened.
struct WarmTarget {
    state: Arc<Mutex<AppState>>,
    key: TabKey,
    lineage: crate::thread::SessionInstance,
    log: SessionLog,
    cold_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl WarmTarget {
    fn install(context: &ProjectFixture, owner: &str, worker: &str) -> Self {
        let log = SessionLog::default();
        let mut app = context.fixture.state().lock().unwrap();
        let root = AppState::canonical_root(&app.runs[owner].worktree.path);
        assert!(root.starts_with(&context.root));
        let choice = app
            .entity_agents(owner)
            .unwrap()
            .by_id(worker)
            .unwrap()
            .choice
            .clone();
        assert_eq!(choice.provider, AgentProvider::Claude);
        let key = insert_agent_tab(
            &mut app,
            &root,
            owner,
            worker,
            DictatedSession::reporting(AgentStatus::Waiting)
                .recording_into(&log)
                .natively_accepting(choice.clone()),
        );
        let tab = app.session_registry.test_tab_mut(&key).unwrap();
        tab.role = TabRole::Agent {
            owner: owner.into(),
            agent_id: worker.into(),
            provider: choice.provider,
        };
        let lineage = tab
            .session_instance
            .clone()
            .expect("warm session has actual lineage");
        assert_eq!(lineage.entity_id, owner);
        assert_eq!(lineage.agent_id, worker);
        assert_eq!(lineage.checkout, root.display().to_string());
        assert!(tab.session_is_live());
        assert_eq!(app.session_registry.test_counts().claims, 0);
        let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
        let saved = saved.iter().find(|run| run.id == owner).unwrap();
        let agent = saved
            .agents
            .iter()
            .find(|agent| agent.id == worker)
            .unwrap();
        let session = agent
            .thread
            .sessions
            .iter()
            .find(|session| session.id == lineage.id)
            .unwrap();
        assert_eq!(session.provider, choice.provider.wire_id());
        assert_eq!(session.model, choice.model);
        assert_eq!(session.effort, choice.effort);
        assert_eq!(session.checkout.as_deref(), Some(root.to_str().unwrap()));
        drop(app);
        let cold_attempts = forbid_cold_open(context);
        Self {
            state: Arc::clone(context.fixture.state()),
            key,
            lineage,
            log,
            cold_attempts,
        }
    }
}

impl Drop for WarmTarget {
    fn drop(&mut self) {
        // This fallback owns only an in-memory dictated session. End it even
        // on an assertion failure; successful removal has already retired it.
        let session = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_registry
            .test_tab(&self.key)
            .filter(|tab| {
                tab.session_instance
                    .as_ref()
                    .is_some_and(|lineage| lineage.id == self.lineage.id)
            })
            .map(|tab| Arc::clone(&tab.session));
        if let Some(session) = session {
            session.end();
        }
    }
}

fn forbid_cold_open(context: &ProjectFixture) -> Arc<std::sync::atomic::AtomicUsize> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&attempts);
    let refused = Agent::WarmBuilder(Arc::new(move |_, _, _| {
        called.fetch_add(1, Ordering::SeqCst);
        Err(crate::harness::HarnessError::Unsupported(
            "the MCP warm-session recipe forbids opening a cold provider".into(),
        ))
    }));
    let mut app = context.fixture.state().lock().unwrap();
    let worktrees = app.project_worktrees_root(&context.project);
    let project_index = app
        .projects
        .iter()
        .position(|project| project.id == context.project)
        .unwrap();
    app.agent = refused.clone();
    app.project_at_mut(project_index).orch = Orchestrator::new(
        context.repo.clone(),
        worktrees,
        refused,
        Templates::default(),
        test_bridge_exe(),
    );
    attempts
}

async fn message_workspace_agent(
    context: &mut ProjectFixture,
    workspace: &str,
    owner: &str,
    worker: &str,
    warm: &WarmTarget,
) {
    const BODY: &str = "MCP lifecycle warm delivery";
    let proof = context
        .fixture
        .call_mcp_tool(
            &context.caller.agent,
            &context.caller.token,
            McpSurface::Project,
            "message_workspace_agent",
            json!({"workspace_id":workspace,"agent_id":worker,"body":BODY}),
        )
        .await;
    assert_eq!(proof.reply["result"]["workspace_id"], workspace);
    assert_eq!(proof.reply["result"]["entity_id"], owner);
    assert_eq!(proof.reply["result"]["agent_id"], worker);
    let operation = proof.reply["result"]["operation_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_message_delivery(context, &operation, &warm.log).await;
    let evidence = message_evidence(context, owner, worker, &operation, warm, BODY);
    context.fixture.verify(proof, evidence).await;
}

async fn wait_for_message_delivery(context: &ProjectFixture, operation: &str, log: &SessionLog) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let terminal = context
            .fixture
            .state()
            .lock()
            .unwrap()
            .operation_receipt(operation)
            .unwrap()
            .is_some_and(|receipt| receipt.status == crate::operation::OperationStatus::Delivered);
        if terminal && !log.turns().is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual warm message delivery did not settle"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

fn message_evidence(
    context: &ProjectFixture,
    owner: &str,
    worker: &str,
    operation: &str,
    warm: &WarmTarget,
    body: &str,
) -> Value {
    use crate::operation::OperationStatus;
    use crate::thread::{MessageDeliveryStatus, ThreadItem};
    use std::sync::atomic::Ordering;
    let app = context.fixture.state().lock().unwrap();
    let receipt = app.operation_receipt(operation).unwrap().unwrap();
    assert_eq!(receipt.status, OperationStatus::Delivered);
    assert!(
        receipt.execution_error.is_none(),
        "a refused handoff is not delivered coverage"
    );
    assert_eq!(receipt.entity_id, owner);
    assert_eq!(receipt.agent_id, worker);
    assert_eq!(receipt.conversation_id, warm.lineage.conversation_id);
    let requester = receipt.requested_by.as_ref().unwrap();
    assert_eq!(requester.entity_id, context.caller.owner);
    assert_eq!(requester.agent_id, context.caller.agent);
    assert_eq!(
        requester.conversation_id,
        app.entity_agents(&context.caller.owner)
            .unwrap()
            .by_id(&context.caller.agent)
            .unwrap()
            .conversation_id()
    );
    let durable = app.store.as_ref().unwrap().thread_items(worker).unwrap();
    let sent = durable
        .iter()
        .find_map(|item| match item {
            ThreadItem::Message(message) if message.operation_id.as_deref() == Some(operation) => {
                Some(message)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(sent.body, body);
    assert_eq!(sent.from_agent.as_ref().unwrap().id, context.caller.agent);
    assert_eq!(sent.delivery_status, Some(MessageDeliveryStatus::Sent));
    let tab = app.session_registry.test_tab(&warm.key).unwrap();
    assert_eq!(tab.session_instance.as_ref().unwrap().id, warm.lineage.id);
    assert!(tab.last_delivered_at.is_some());
    assert_eq!(app.session_registry.test_counts().claims, 0);
    let receipt = receipt.wire_value();
    drop(app);
    assert_eq!(
        warm.cold_attempts.load(Ordering::SeqCst),
        0,
        "warm delivery tried to open a provider"
    );
    assert_eq!(
        warm.log.turns().len(),
        1,
        "exactly one real send_turn is required"
    );
    assert!(warm.log.turns()[0].contains(body));
    json!({"caller":context.caller.agent,"workspace_owner":owner,"worker":worker,
        "operation_id":operation,"operation":receipt,"body":body,
        "session_instance":warm.lineage.id,"recorded_send_turns":1,"cold_provider_opens":0,
        "durable_delivery_status":"sent"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_mcp_workspace_alias_delivers_to_existing_warm_session() {
    let mut context = ProjectFixture::new();
    let workspace = super::workspaces::create(&mut context).await;
    workspace_worker(&mut context, &workspace).await;
    super::workspaces::delete(&mut context, &workspace).await;
    let steps = context.fixture.finish().await;
    let messages: Vec<_> = steps
        .iter()
        .filter(|step| step.method == "mcp.control:message_workspace_agent")
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].mcp_action.as_ref().unwrap()["action"],
        "message_workspace_agent"
    );
    assert_eq!(messages[0].effect["recorded_send_turns"], 1);
    assert_eq!(messages[0].effect["cold_provider_opens"], 0);
}
