//! What an agent's push says (#200): the agent's name, the first line of the
//! attention-class item that made it news, and the deep link to its
//! conversation. Built under the app lock from what the lock already holds;
//! sealed later, off it (`AppState::spawn_notify`). The words go only into
//! that sealed content — never into a log.

use crate::app::AppState;
use crate::notify::content::{conversation_url, first_line, ConversationPlace, PushContent};
use crate::thread::{AgentOwnerKind, ThreadEventKind, ThreadItem};

impl AppState {
    /// The content for one agent's news on `entity_id`, or `None` when any
    /// part of it cannot be said — the push then goes out generic.
    ///
    /// The entity maps to its place through the owner its messages already
    /// wear (`conversation_owner_ref`): a workspace's conversation owner is
    /// the run whose worktree is that workspace's root; a project's is the run
    /// on the project's scratch root. Either way the project is the one the
    /// entity is registered under.
    pub(in crate::app) fn agent_push_content(
        &self,
        entity_id: &str,
        agent_id: &str,
        attention_sequence: Option<u64>,
    ) -> Option<PushContent> {
        let owner = self.conversation_owner_ref(entity_id)?;
        let project_id = self.projects.project_id_of(entity_id)?;
        let place = match owner.kind {
            AgentOwnerKind::Workspace => ConversationPlace::Workspace {
                project_id,
                workspace_id: &owner.id,
            },
            AgentOwnerKind::Project => ConversationPlace::Project { project_id },
        };
        let url = conversation_url(self.notifier.as_ref()?.device_id(), &place, agent_id);
        let title = self
            .agent_name(entity_id, agent_id)
            .unwrap_or_else(|| owner.name.clone());
        let body = self.attention_line(entity_id, agent_id, attention_sequence?)?;
        PushContent::new(&title, &body, url)
    }

    /// The first line of the item at `sequence` on the agent's conversation.
    fn attention_line(&self, entity_id: &str, agent_id: &str, sequence: u64) -> Option<String> {
        let thread = self.agent_conversation(entity_id, Some(agent_id)).ok()?;
        let item = thread
            .items
            .iter()
            .rev()
            .find(|item| item.sequence() == sequence)?;
        attention_words(item)
    }
}

/// A message's first line; an event's summary, else a phrase for its kind.
fn attention_words(item: &ThreadItem) -> Option<String> {
    match item {
        ThreadItem::Message(message) => first_line(&message.body),
        ThreadItem::Event(event) => event
            .summary
            .as_deref()
            .and_then(first_line)
            .or_else(|| event_phrase(event.event).map(str::to_string)),
    }
}

/// What an attention event means, for one that carries no summary. `None` for
/// a kind with no words here.
fn event_phrase(kind: ThreadEventKind) -> Option<&'static str> {
    Some(match kind {
        ThreadEventKind::Done => "Finished",
        ThreadEventKind::Blocked => "Blocked and needs you",
        ThreadEventKind::ReviewBlocked => "Review is blocked",
        ThreadEventKind::RunFailed => "The run failed",
        ThreadEventKind::StageFailed => "A stage failed",
        ThreadEventKind::RecoveryFailed => "Recovery failed",
        ThreadEventKind::IdleUnreported => "Went quiet without reporting",
        ThreadEventKind::Interrupted => "Stopped before it finished",
        ThreadEventKind::Merged => "Merged",
        ThreadEventKind::Abandoned => "Abandoned",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{ThreadEvent, ThreadItem};

    fn event(kind: ThreadEventKind, summary: Option<&str>) -> ThreadItem {
        let mut thread = crate::thread::Thread::new("t");
        thread.push_event(
            kind,
            summary.map(str::to_string),
            None,
            None,
            "2026-09-27T00:00:00Z",
        );
        let item = thread.items.pop().unwrap();
        assert!(matches!(item, ThreadItem::Event(ThreadEvent { .. })));
        item
    }

    #[test]
    fn an_event_without_a_summary_says_its_kind_in_a_phrase() {
        assert_eq!(
            attention_words(&event(ThreadEventKind::Interrupted, None)).as_deref(),
            Some("Stopped before it finished")
        );
        assert_eq!(
            attention_words(&event(ThreadEventKind::Done, Some("\n Shipped   it\nmore")))
                .as_deref(),
            Some("Shipped it")
        );
        assert_eq!(
            attention_words(&event(ThreadEventKind::SessionStarted, None)),
            None
        );
    }
}
