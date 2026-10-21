//! A queued MCP compaction remains owned through its final published detail.
mod status_origins;
use super::*;
use crate::app::runtime::pumps::status_timing::MeasuredStatusPump;
use crate::app::tests::runtime::compaction::{CompactingAgent, RUN};
use crate::app::tests::*;
use crate::harness::{AgentActivity, TurnContext};
use crate::mcp::BridgeAction;

const TOKEN: &str = "qualification-compaction-owned-capability";

async fn observe(
    agent: &CompactingAgent,
    mut pump: MeasuredStatusPump,
    status: AgentStatus,
) -> MeasuredStatusPump {
    let next = agent.status.borrow().transition(status).unwrap();
    agent.status.send_replace(next.clone());
    tokio::task::spawn_blocking(move || {
        pump.observe(&next);
        pump
    })
    .await
    .unwrap()
}

/// A status observation schedules the compaction on the blocking pool. Its
/// return is not the session's receipt: wait for the real send and for the
/// sender's following guarded commit before supplying completion callbacks.
async fn sent_compaction(agent: &CompactingAgent) -> Option<Vec<String>> {
    let deadline = Instant::now() + WAIT_BUDGET;
    loop {
        let turns = agent.log.turns();
        let context_cleared = agent
            .state
            .lock()
            .unwrap()
            .entity_agents(RUN)
            .unwrap()
            .by_id(&agent.agent_id)
            .unwrap()
            .last_context_tokens
            .is_none();
        // send_compaction clears context and installs SentCompaction under the
        // same guard, after the finite session has accepted the command.
        if turns == ["/compact"] && context_cleared {
            return Some(turns);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::task::yield_now().await;
    }
}

async fn retained_compaction(project_caller: bool) -> Vec<VerifiedStep> {
    let agent = CompactingAgent::without_background_services().with_context(200_000);
    let caller = {
        let mut app = agent.state.lock().unwrap();
        let caller = if project_caller {
            let project = app.projects.project_id_of(RUN).unwrap().to_owned();
            crate::app::tests::project_agent::project_agent(&mut app, &project).1
        } else {
            agent.agent_id.clone()
        };
        app.test_install_mcp_token(caller.clone(), TOKEN.into());
        caller
    };
    let mut fixture = RareFixture::shared("mcp-compaction", Arc::clone(&agent.state));
    let key = TabKey::agent(&AppState::canonical_root(&agent.root), &agent.agent_id);
    let (session, instance) = {
        let app = agent.state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (
            Arc::clone(&tab.session),
            tab.session_instance.clone().unwrap(),
        )
    };
    let pump = MeasuredStatusPump::new(&agent.state, key, &session, instance);
    let pump = observe(&agent, pump, AgentStatus::Working).await;
    let action = if project_caller {
        BridgeAction::CompactAgent {
            agent_id: agent.agent_id.clone(),
            instructions: "keep qualification evidence".into(),
        }
    } else {
        BridgeAction::CompactSelf {
            instructions: "keep qualification evidence".into(),
        }
    };
    let proof = fixture.call_mcp(&caller, TOKEN, action).await;
    let sent_before_release = agent.log.turns();
    let retained_after_root = fixture.origin_active(&proof).await;
    let pump = observe(&agent, pump, AgentStatus::Waiting).await;
    let sent = sent_compaction(&agent).await;

    // Finite external callback input drives the actual activity/context
    // recording boundaries. The registry is never completed by the fixture.
    if sent.is_some() {
        let mut app = agent.state.lock().unwrap();
        app.record_agent_activity(
            RUN,
            &agent.agent_id,
            &AgentActivity::Compaction { completed: false },
            None,
        );
        app.record_agent_turn_context(
            RUN,
            &agent.agent_id,
            TurnContext {
                context_tokens: 17_000,
                cache_read_tokens: 0,
            },
        );
    }
    let row = {
        let mut app = agent.state.lock().unwrap();
        crate::app::tests::project_agent::items(&mut app, RUN, &agent.agent_id)
            .into_iter()
            .find(|row| row["data"]["event"] == "compaction")
    };
    session.end();
    drop(pump);
    let turns = sent.expect("the real compaction send and ledger commit completed");
    let row = row.expect("the actual completed compaction published a row");
    assert!(sent_before_release.is_empty());
    assert_eq!(turns, ["/compact".to_owned()]);
    assert_eq!(row["data"]["compaction"]["context_after"], 17_000);
    fixture
        .verify(proof, json!({"sent":turns,"published_compaction":row}))
        .await;
    let steps = fixture.finish().await;
    assert!(
        retained_after_root,
        "accepted compaction lost its required descendant after root completion"
    );
    steps
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = retained_compaction(false).await;
    steps.extend(retained_compaction(true).await);
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_mcp_compact_self_retains_origin_through_stamped_publication() {
    retained_compaction(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_mcp_compact_agent_retains_origin_through_stamped_publication() {
    retained_compaction(true).await;
}
