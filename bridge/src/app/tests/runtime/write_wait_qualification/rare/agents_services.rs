//! Actual agent, conversation and isolated local-service RPC recipes.

use super::{RareFixture, VerifiedStep};
use crate::app::tests::*;
use std::collections::BTreeSet;

mod inventory;
mod local;
mod spawn;
mod terminal;
mod updater;
mod provider;
mod conversation;

struct EndFixtureSessions(Arc<Mutex<AppState>>);

impl Drop for EndFixtureSessions {
    fn drop(&mut self) {
        let sessions: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .session_registry
            .test_tabs()
            .map(|(_, tab)| Arc::clone(&tab.session))
            .collect();
        for session in sessions {
            session.end();
        }
    }
}

const METHODS: &[&str] = &[
    "agent.add",
    "agent.choose",
    "agent.interrupt",
    "agent.remove",
    "agent.start",
    "bridge.check_update",
    "bridge.install_update",
    "capture.answer",
    "capture.cancel",
    "capture.create",
    "capture.reroute",
    "conversation.reset",
    "conversation.settings",
    "conversation.unwatch",
    "conversation.watch",
    "entity.dismiss",
    "entity.mute",
    "entity.seen",
    "harnesses.refresh",
    "push.registerKey",
    "push.revokeKey",
    "settings.set",
    "term.close",
    "term.create",
    "thread.attach",
    "thread.post",
    "user.present",
];

pub(super) async fn execute() -> Vec<VerifiedStep> {
    Vec::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_service_recipes_execute_twenty_seven_successful_mutations() {
    let steps = execute().await;
    let observed: BTreeSet<_> = steps.iter().map(|step| step.method.as_str()).collect();
    assert_eq!(observed, METHODS.iter().copied().collect());
}
