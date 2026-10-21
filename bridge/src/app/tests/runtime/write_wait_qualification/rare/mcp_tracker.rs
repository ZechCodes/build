//! Tracker variants enter through the authenticated MCP socket, including aliases.
mod basic;
mod reviews;

use super::super::rpc::ReplyProof;
use super::finite_provider::FiniteProvider;
use super::{RareFixture, VerifiedStep};
use crate::app::tests::project_agent::project_agent;
use crate::app::tests::support::{
    dictated_agent_tab, wait_for_deliveries, DictatedSession, SessionLog,
};
use crate::app::{AppState, TabKey, TabRole};
use crate::harness::{AgentSession, AgentStatus, SessionStatusSnapshot};
use crate::mcp::{BridgeAction, McpSurface};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

const TOKEN: &str = "qualification-tracker-capability";

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = basic::execute().await;
    steps.extend(reviews::execute().await);
    steps
}

struct TrackerFixture {
    inner: RareFixture,
    agent: String,
    caller_key: TabKey,
    provider_scope: FiniteProvider,
    warm_log: SessionLog,
    warm_caller: WarmCaller,
}

struct WarmCaller {
    session: Arc<dyn AgentSession>,
    _status: watch::Sender<SessionStatusSnapshot>,
}

impl Drop for WarmCaller {
    fn drop(&mut self) {
        self.session.end();
    }
}

impl TrackerFixture {
    fn new(family: &'static str, mut app: AppState) -> Self {
        let project = app.project_at(0).id.clone();
        let (owner, agent) = project_agent(&mut app, &project);
        let checkout = app.runs[&owner].worktree.path.clone();
        let choice = app.runs[&owner]
            .agents
            .by_id(&agent)
            .unwrap()
            .choice
            .clone();
        let provider_scope = FiniteProvider::install(&checkout);
        provider_scope.require_enrolled();
        let warm_log = provider_scope.log.clone();
        let (status, watched_status) =
            watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let session = DictatedSession::reporting(AgentStatus::Waiting)
            .recording_into(&warm_log)
            .watching_status(watched_status)
            .natively_accepting(choice.clone())
            .interruptible();
        let mut tab = dictated_agent_tab(&checkout, &owner, &agent, session);
        tab.role = TabRole::Agent {
            owner: owner.clone(),
            agent_id: agent.clone(),
            provider: choice.provider,
        };
        tab.session_instance = app.record_agent_session_start(
            &owner,
            &agent,
            &checkout,
            &choice,
            "qualification-warm-caller",
        );
        assert!(app.session_instance_uses_choice(tab.session_instance.as_ref().unwrap(), &choice));
        let caller_key = TabKey::agent(&AppState::canonical_root(&checkout), &agent);
        let warm_caller = WarmCaller {
            session: Arc::clone(&tab.session),
            _status: status,
        };
        app.session_registry
            .test_insert_tab(caller_key.clone(), tab);
        app.delivery_queue.clear_queued();
        app.test_install_mcp_token(agent.clone(), TOKEN.into());
        Self {
            inner: RareFixture::new(family, app),
            agent,
            caller_key,
            provider_scope,
            warm_log,
            warm_caller,
        }
    }

    fn state(&self) -> &Arc<Mutex<AppState>> {
        self.inner.state()
    }

    fn actor(&self) -> Value {
        json!({"kind":"agent","agent_id":self.agent})
    }

    async fn call(&self, name: &str, arguments: Value) -> ReplyProof {
        self.inner
            .call_mcp_tool(&self.agent, TOKEN, McpSurface::Project, name, arguments)
            .await
    }

    async fn verify(&mut self, proof: ReplyProof, effect: Value) {
        self.inner.verify(proof, effect).await;
    }

    async fn wrapped_read_refusal(&self, task: &str) {
        let action = BridgeAction::TrackerWithWaitingOn {
            wrapped: Box::new(BridgeAction::TrackerGetTask {
                task_id: task.into(),
            }),
            waiting_on: Vec::new(),
        };
        let proof = self
            .inner
            .call_mcp_response(&self.agent, TOKEN, action)
            .await;
        assert_eq!(
            proof.reply["error"],
            "waiting_on only accompanies create_task or link_task"
        );
        self.inner.verify_refusal(proof).await;
    }

    async fn finish(self) -> Vec<VerifiedStep> {
        wait_for_deliveries(self.state()).await;
        {
            let app = self.state().lock().unwrap();
            assert!(
                app.session_registry.token_matches(&self.agent, TOKEN),
                "the stable caller capability must survive real background notices"
            );
            let tab = app.session_registry.test_tab(&self.caller_key).unwrap();
            assert!(tab.live && !matches!(tab.session.status(), AgentStatus::Ended { .. }));
        }
        assert_eq!(
            self.provider_scope.opens.load(Ordering::SeqCst),
            0,
            "the prepared warm caller must not be replaced by a cold spawn"
        );
        eprintln!(
            "mcp_tracker_warm_notice_turns={}",
            self.warm_log.turns().len()
        );
        self.warm_caller.session.end();
        assert!(self.warm_log.ended());
        self.inner.finish().await
    }
}
