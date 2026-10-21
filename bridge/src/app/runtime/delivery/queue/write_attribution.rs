//! Required queue residence is independent of the request's final reply.
use super::*;
use crate::timing::write_wait::{
    current_context, IntervalCategory, ObligationId, WaitGuard, WaitKind, WriteContext,
};

pub(super) struct QueuedWait {
    conversation: String,
    context: WriteContext,
    child: ObligationId,
    _waiting: WaitGuard,
}

pub(super) fn begin(waits: &mut Vec<QueuedWait>, turn: &PendingAgentTurn) {
    for child in &turn.logical_obligations {
        let context = child.context();
        if waits
            .iter()
            .any(|wait| wait.child == child.id() && wait.context.same_registry(&context))
        {
            continue;
        }
        waits.push(QueuedWait {
            conversation: turn.conversation_id.clone(),
            child: child.id(),
            _waiting: context.interval(IntervalCategory::KnownWait(WaitKind::Dispatch)),
            context,
        });
    }
}
pub(super) fn end(waits: &mut Vec<QueuedWait>, turn: &PendingAgentTurn) {
    waits.retain(|wait| {
        !turn
            .logical_obligations
            .iter()
            .any(|child| wait.child == child.id() && wait.context.same_registry(&child.context()))
    });
}
pub(super) fn discard(waits: &mut Vec<QueuedWait>, turn: &mut PendingAgentTurn) {
    end(waits, turn);
    turn.cancel_logical_origins();
}
pub(super) fn take_conversation(
    waits: &mut Vec<QueuedWait>,
    conversation: &str,
) -> Vec<QueuedWait> {
    let (held, remaining) = std::mem::take(waits)
        .into_iter()
        .partition(|wait: &QueuedWait| wait.conversation == conversation);
    *waits = remaining;
    held
}
impl PendingAgentTurn {
    pub(in crate::app) fn capture_logical_origin(&mut self) {
        if self.logical_obligations.is_empty() {
            if let Some(context) = current_context() {
                self.logical_obligations.push(context.child());
            }
        }
    }
    pub(in crate::app) fn absorb_logical_origins(&mut self, other: &mut Self) {
        self.logical_obligations
            .append(&mut other.logical_obligations);
    }
    pub(in crate::app) fn cancel_logical_origins(&mut self) {
        for child in self.logical_obligations.drain(..) {
            child.cancel();
        }
    }
    pub(in crate::app) fn complete_logical_origins(&mut self) {
        for child in self.logical_obligations.drain(..) {
            child.complete();
        }
    }
    pub(in crate::app) fn logical_contexts(&self) -> Vec<WriteContext> {
        self.logical_obligations
            .iter()
            .map(|child| child.context())
            .collect()
    }
}
impl HeldConversationTurns {
    pub(in crate::app) fn cancel(mut self) {
        self.logical_waits.clear();
        for turn in &mut self.queued {
            turn.cancel_logical_origins();
        }
        self.settling.cancel_logical_origins();
    }
}
