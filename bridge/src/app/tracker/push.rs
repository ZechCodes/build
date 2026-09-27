//! Browser push for task news (#191).
//!
//! A task adds to the unread counter exactly as its row counts it
//! ([`super::inbox::unread_since_mark`], #183): news that is not the user's own
//! doing, on a task the user watches, while it is neither Done nor closed.
//! A write that adds to it fires one content-free notify — the task's opaque
//! id and the generic `task` kind, never its title or a comment's words.

use super::inbox::event_counts_as_unread;
use super::TaskWrite;
use crate::app::AppState;
use crate::tracker::Actor;

impl AppState {
    /// Push once for a write that adds to the counter, inside the task's
    /// debounce window. Runs after the write is durable, and a push that
    /// cannot be sent only logs.
    pub(in crate::app) fn push_task_news(&mut self, write: &TaskWrite) {
        if !write.adds_to_the_unread_counter() {
            return;
        }
        if self
            .notify_throttle
            .should_notify(&write.task.id, crate::notify::unix_seconds())
        {
            self.spawn_notify(write.task.id.clone(), crate::notify::TASK_KIND);
        }
    }
}

impl TaskWrite {
    /// Whether anything this write put on the timeline is unread news to a
    /// user watching the task.
    fn adds_to_the_unread_counter(&self) -> bool {
        self.task.watched && !self.task.is_finished() && self.carries_news_from_somebody_else()
    }

    fn carries_news_from_somebody_else(&self) -> bool {
        let not_the_user = |actor: &Actor| !matches!(actor, Actor::User);
        self.comments
            .iter()
            .any(|comment| not_the_user(&comment.author))
            || self
                .events
                .iter()
                .any(|event| not_the_user(&event.actor) && event_counts_as_unread(event))
    }
}
