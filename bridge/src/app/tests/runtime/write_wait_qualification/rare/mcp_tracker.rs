//! Tracker variants enter through the authenticated MCP socket, including aliases.
mod basic;
mod reviews;

use super::super::rpc::ReplyProof;
use super::{RareFixture, VerifiedStep};
use crate::app::tests::project_agent::project_agent;
use crate::app::AppState;
use crate::mcp::{BridgeAction, McpSurface};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "qualification-tracker-capability";

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = basic::execute().await;
    steps.extend(reviews::execute().await);
    steps
}

struct TrackerFixture {
    inner: RareFixture,
    agent: String,
}

impl TrackerFixture {
    fn new(family: &'static str, mut app: AppState) -> Self {
        let project = app.project_at(0).id.clone();
        let (_, agent) = project_agent(&mut app, &project);
        app.delivery_queue.clear_queued();
        app.test_install_mcp_token(agent.clone(), TOKEN.into());
        Self {
            inner: RareFixture::new(family, app),
            agent,
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
        self.inner.finish().await
    }
}
