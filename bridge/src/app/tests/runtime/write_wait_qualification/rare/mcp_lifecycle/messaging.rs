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

    // The warm message/delivery proof is intentionally absent in this bounded
    // checkpoint. No cold session is opened and no message action is credited.
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
    context.fixture.verify(proof,json!({"caller":context.caller.agent,"removed_worker":worker,"owner":entity,"workspace_preserved":workspace})).await;
}
