//! Four conversation writer variants over actual authenticated MCP sockets.
use super::{RareFixture, VerifiedStep};
use crate::app::tests::*;
use crate::mcp::BridgeAction;
use crate::operation::OperationStatus;
use std::time::{Duration, Instant};

const TOKEN: &str = "qualification-common-mcp-capability";

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let mut app = qa_state(&repository, directory.path());
    app.default_harness = AgentProvider::Claude;
    let owner = adopted_run(&mut app, &repository, directory.path(), "rare-common-mcp");
    let caller = primary_agent_id(&app, &owner);
    let added = app.handle(req(
        "agent.add",
        json!({"entity_id":owner,"name":"Receiver"}),
    ));
    assert_eq!(added["ok"], true);
    let recipient = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
    let root = app.runs[&owner].worktree.path.clone();
    let choice = app
        .entity_agents(&owner)
        .unwrap()
        .by_id(&recipient)
        .unwrap()
        .choice
        .clone();
    let log = SessionLog::default();
    let mut tab = dictated_agent_tab(
        &root,
        &owner,
        &recipient,
        DictatedSession::reporting(AgentStatus::Waiting)
            .recording_into(&log)
            .natively_accepting(choice.clone()),
    );
    tab.session_instance = app.record_agent_session_start(
        &owner,
        &recipient,
        &root,
        &choice,
        "qualification-existing-recipient",
    );
    assert!(tab.session_instance.is_some());
    let session = Arc::clone(&tab.session);
    app.session_registry.test_insert_tab(
        TabKey::agent(&AppState::canonical_root(&root), &recipient),
        tab,
    );
    app.session_locator_factory =
        Arc::new(|_, _| panic!("MCP common recipe must use its existing finite session"));
    app.delivery_queue.clear_queued();
    app.test_install_mcp_token(caller.clone(), TOKEN.into());
    let mut fixture = RareFixture::new("mcp-common-conversation", app);

    let topic = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::SetTopic {
                topic: "Qualify write waits".into(),
            },
        )
        .await;
    let saved = persisted_agent(&fixture, &owner, &caller);
    assert_eq!(saved.topic.as_deref(), Some("Qualify write waits"));
    fixture
        .verify(
            topic,
            json!({"caller":caller,"persisted_topic":saved.topic}),
        )
        .await;

    let name = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::SetName {
                name: "Qualifier".into(),
            },
        )
        .await;
    let saved = persisted_agent(&fixture, &owner, &caller);
    assert_eq!(saved.name.as_deref(), Some("Qualifier"));
    fixture
        .verify(name, json!({"caller":caller,"persisted_name":saved.name}))
        .await;

    let posted = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::PostThreadMessage {
                still_working: true,
                body: "Qualified progress evidence".into(),
                options: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await;
    let message = persisted_message(&fixture, &owner, &caller, "Qualified progress evidence");
    assert_eq!(
        message["data"]["role"],
        json!(crate::thread::MessageRole::Agent)
    );
    fixture
        .verify(posted, json!({"caller":caller,"persisted_message":message}))
        .await;

    let sent = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::MessageAgent {
                agent_id: recipient.clone(),
                body: "Inspect the finite qualification evidence".into(),
                attachments: Vec::new(),
            },
        )
        .await;
    let operation = sent.reply["result"]["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let delivered = {
            let app = fixture.state().lock().unwrap();
            app.operation_receipt(&operation)
                .unwrap()
                .is_some_and(|receipt| receipt.status == OperationStatus::Delivered)
        };
        if delivered
            && log
                .turns()
                .iter()
                .any(|turn| turn.contains("Inspect the finite qualification evidence"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual message was not delivered to finite session"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let message = persisted_message(
        &fixture,
        &owner,
        &recipient,
        "Inspect the finite qualification evidence",
    );
    assert_eq!(message["data"]["role"], "user");
    assert_eq!(message["data"]["from_agent"]["id"], caller);
    let requested_by = fixture
        .state()
        .lock()
        .unwrap()
        .operation_receipt(&operation)
        .unwrap()
        .unwrap()
        .requested_by
        .unwrap();
    assert_eq!(requested_by.agent_id, caller);
    session.end();
    assert!(log.ended());
    fixture.verify(sent,json!({"recipient":recipient,"operation":operation,"persisted_message":message,"actual_turns":log.turns()})).await;
    fixture.finish().await
}

fn persisted_agent(fixture: &RareFixture, owner: &str, agent: &str) -> crate::agent::Agent {
    fixture
        .state()
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == owner)
        .unwrap()
        .agents
        .into_iter()
        .find(|row| row.id == agent)
        .unwrap()
}

fn persisted_message(fixture: &RareFixture, owner: &str, agent: &str, body: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    let conversation = app
        .entity_agents(owner)
        .unwrap()
        .by_id(agent)
        .unwrap()
        .conversation_id();
    app.store
        .as_ref()
        .unwrap()
        .thread_items(conversation)
        .unwrap()
        .into_iter()
        .map(|item| serde_json::to_value(item).unwrap())
        .find(|item| item["type"] == "message" && item["data"]["body"] == body)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_common_mcp_writers_persist_and_deliver_four_effects() {
    crate::git_fixture::environment::isolated_git_test!();
    let steps = execute().await;
    let actions: std::collections::BTreeSet<_> = steps
        .iter()
        .map(|step| {
            step.mcp_action.as_ref().unwrap()["action"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        actions,
        [
            "set_topic",
            "set_name",
            "post_thread_message",
            "message_agent"
        ]
        .into_iter()
        .collect()
    );
}
