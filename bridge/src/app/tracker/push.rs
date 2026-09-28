//! Browser push for task news (#191, #200).
//!
//! A task adds to the unread counter exactly as its row counts it
//! ([`super::inbox::unread_since_mark`], #183): news that is not the user's own
//! doing, on a task the user watches, while it is neither Done nor closed.
//! A write that adds to it fires one notify. The api sees only the task's
//! opaque id and the generic `task` kind; the words the browser shows — the
//! task's number and title, and who said what — go only inside content sealed
//! to each notification key (`notify::seal`), and never into a log.

use super::inbox::event_counts_as_unread;
use super::TaskWrite;
use crate::app::AppState;
use crate::notify::content::{first_line, task_url, PushContent};
use crate::tracker::{Actor, TaskEvent, TaskEventKind};

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
            let content = self.task_push_content(write).map(Into::into);
            self.spawn_notify(write.task.id.clone(), crate::notify::TASK_KIND, content);
        }
    }

    /// `#<number> <title>`, then who said what. `None` when the write's news
    /// has no words, so the push goes out generic.
    fn task_push_content(&self, write: &TaskWrite) -> Option<PushContent> {
        let title = format!("#{} {}", write.task.number, write.task.title);
        let body = self.task_news_line(write)?;
        PushContent::new(&title, &body, task_url(&write.task.id))
    }

    /// The newest comment from somebody else, as `<name>: <first line>`;
    /// without one, the newest news event from somebody else, in a phrase.
    fn task_news_line(&self, write: &TaskWrite) -> Option<String> {
        let comment_line = write
            .comments
            .iter()
            .rev()
            .filter(|comment| !matches!(comment.author, Actor::User))
            .find_map(|comment| {
                let said = first_line(&comment.body)?;
                Some(format!(
                    "{}: {said}",
                    self.push_actor_name(write, &comment.author)
                ))
            });
        comment_line.or_else(|| {
            let event = write.events.iter().rev().find(|event| {
                !matches!(event.actor, Actor::User) && event_counts_as_unread(event)
            })?;
            let phrase = news_phrase(event)?;
            Some(format!(
                "{} {phrase}",
                self.push_actor_name(write, &event.actor)
            ))
        })
    }

    /// Who acted, by the name the reader knows them by: the agent's own name,
    /// else the one the task remembers for it, else its workspace's.
    fn push_actor_name(&self, write: &TaskWrite, actor: &Actor) -> String {
        let Actor::Agent { agent_id } = actor else {
            return match actor {
                Actor::User => "You".to_string(),
                _ => "Build".to_string(),
            };
        };
        let remembered = write.task.identities.get(agent_id);
        self.agent_display_name(actor)
            .or_else(|| remembered.and_then(|identity| identity.name.clone()))
            .or_else(|| remembered.and_then(|identity| identity.workspace_name.clone()))
            .unwrap_or_else(|| "An agent".to_string())
    }
}

/// What a news event did, as the words after its actor's name. `None` for an
/// event with no words here, which sends no content.
pub(in crate::app) fn news_phrase(event: &TaskEvent) -> Option<String> {
    let phrase = match event.kind {
        TaskEventKind::Moved => return Some(moved_phrase(event)),
        TaskEventKind::Assigned if assigned_to_the_user(event) => "assigned it to you",
        TaskEventKind::Assigned => "assigned it",
        TaskEventKind::Unassigned => "unassigned it",
        TaskEventKind::Closed => "closed it",
        TaskEventKind::Reopened => "reopened it",
        TaskEventKind::Created => "filed it for you",
        _ => return None,
    };
    Some(phrase.to_string())
}

fn moved_phrase(event: &TaskEvent) -> String {
    let column = event
        .payload
        .get("to")
        .and_then(serde_json::Value::as_str)
        .and_then(|to| {
            crate::tracker::COLUMNS
                .iter()
                .find(|column| column.id == to)
        });
    match column {
        Some(column) => format!("moved it to {}", column.name),
        None => "moved it".to_string(),
    }
}

fn assigned_to_the_user(event: &TaskEvent) -> bool {
    event.payload["assignee"]["kind"] == "user"
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
