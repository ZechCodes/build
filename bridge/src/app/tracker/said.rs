//! An agent saying, in its own conversation, what it just did to a task
//! (spec: Tasks → An agent says what it did).
//!
//! Separate from [`super::notices`] and deliberately so. A notice is Build
//! telling somebody ELSE what happened, marked `from_build`, and nobody is
//! waiting on an answer to it. This is the AGENT's own sentence about its own
//! work, in its own conversation: role `agent`, no mark, so a reader sees
//! "Commented on #13" among the agent's other words rather than as something
//! the system interjected.
//!
//! It is a message and not an activity row, which is what makes it survive
//! every detail mode. What an agent did to the board is a thing it did, not a
//! tool call it made on the way to doing something else.

use super::TaskWrite;
use crate::app::AppState;
use crate::thread::TaskAction;
use crate::tracker::{Actor, TaskEventKind};

impl AppState {
    /// Post one message per write into the acting agent's own conversation.
    ///
    /// Only when an agent is acting: a human moving a card on the board is
    /// already looking at the board, and a conversation nobody is reading is
    /// not the place to tell them what they just did on screen. The check is
    /// on the ACTOR rather than on the verb, because the same verb serves
    /// both.
    ///
    /// Quiet about its own failure, for the reason every automatic write here
    /// is: the change landed and is durable before this runs.
    pub(in crate::app) fn say_what_the_agent_did(&mut self, write: &TaskWrite) {
        let Actor::Agent { agent_id } = &write.actor else {
            return;
        };
        let agent_id = agent_id.clone();
        let Some(action) = task_action(write) else {
            return;
        };
        if let Err(why) = self.post_task_action(&agent_id, action) {
            eprintln!("say what {agent_id} did to #{}: {why}", write.task.number);
        }
    }

    fn post_task_action(&mut self, agent_id: &str, action: TaskAction) -> Result<(), String> {
        let entity_id = self
            .entity_of_agent(agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        let body = format!(
            "{} #{} {}",
            label(&action.action),
            action.number,
            action.title
        );
        let now = crate::store::now_rfc3339();
        self.edit_agent_conversation(&entity_id, agent_id, |thread, _| {
            thread.post_agent_task_action(body, action, &now);
            Ok(serde_json::Value::Null)
        })?;
        // No queued turn: the agent is already in the turn that did this, and
        // waking it to read its own sentence would be waking it to hear itself.
        Ok(())
    }
}

/// What one write says the agent did — one action for one write, however many
/// events it carried.
///
/// There is no `created_and_assigned`: a create and an assignment are two
/// writes (the number is minted inside the create's own transaction), and the
/// agent surface's `create_task` takes no assignee at all. A slug no path can
/// produce is a branch every client would carry for nothing.
fn task_action(write: &TaskWrite) -> Option<TaskAction> {
    let action = action_slug(write)?;
    Some(TaskAction {
        action: action.to_string(),
        task_id: write.task.id.clone(),
        number: write.task.number,
        title: write.task.title.clone(),
        comment_id: (action == "commented_on")
            .then(|| write.comments.first().map(|comment| comment.id.clone()))
            .flatten(),
        // Read off the TASK rather than off the event, because the task is
        // what the assignment settled on: a creating kind resolves to the
        // agent it made, and the event that named it was written before that
        // agent existed.
        assignee: (action == "assigned")
            .then(|| write.task.assignee.clone())
            .flatten(),
        to: (action == "moved").then(|| moved_to(write)).flatten(),
    })
}

/// The column a move went to, off the event that recorded it.
fn moved_to(write: &TaskWrite) -> Option<String> {
    write
        .events
        .iter()
        .find(|event| event.kind == TaskEventKind::Moved)
        .and_then(|event| event.payload.get("to"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn action_slug(write: &TaskWrite) -> Option<&'static str> {
    let kinds: Vec<TaskEventKind> = write.events.iter().map(|event| event.kind).collect();
    let has = |kind: TaskEventKind| kinds.contains(&kind);
    if !write.comments.is_empty() {
        return Some("commented_on");
    }
    Some(match () {
        _ if has(TaskEventKind::Created) => "created",
        // Two words, not one: a task handed to somebody and a task handed
        // back are opposite things, and a line that called both "assigned"
        // said the wrong one half the time.
        _ if has(TaskEventKind::Assigned) => "assigned",
        _ if has(TaskEventKind::Unassigned) => "unassigned",
        _ if has(TaskEventKind::Closed) => "closed",
        _ if has(TaskEventKind::Reopened) => "reopened",
        _ if has(TaskEventKind::Moved) => "moved",
        _ if has(TaskEventKind::Linked) => "linked",
        _ if has(TaskEventKind::Labelled) => "updated",
        // Tracking is not something a reader of the agent's conversation needs
        // a line about: the agent asked to hear about a task, which is about
        // what it will read rather than about what it did.
        _ => return None,
    })
}

/// How the body reads. The slug is the wire's; this is the bridge's own one
/// line, and a client that renders its own label ignores it.
fn label(action: &str) -> &'static str {
    match action {
        "created" => "Created",
        "assigned" => "Assigned",
        "unassigned" => "Unassigned",
        "moved" => "Moved",
        "closed" => "Closed",
        "reopened" => "Reopened",
        "commented_on" => "Commented on",
        "linked" => "Linked",
        _ => "Updated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{Task, TaskComment, TaskEvent};
    use serde_json::json;

    fn write_with(events: Vec<TaskEventKind>, comment: bool) -> TaskWrite {
        let mut task = Task::drafted(
            "/repo",
            "Kanban drag",
            Actor::Agent {
                agent_id: "agent-1".into(),
            },
            "2026-09-20T15:00:00Z",
        );
        task.number = 13;
        let mut write = TaskWrite {
            comments: Vec::new(),
            events: Vec::new(),
            actor: Actor::Agent {
                agent_id: "agent-1".into(),
            },
            task,
        };
        for kind in events {
            write.events.push(TaskEvent::new(
                &write.task.id,
                write.actor.clone(),
                kind,
                json!({}),
                "2026-09-20T15:01:00Z",
            ));
        }
        if comment {
            write.comments.push(TaskComment {
                id: "tc-9".into(),
                task_id: write.task.id.clone(),
                author: write.actor.clone(),
                body: "said".into(),
                refs: Vec::new(),
                attachments: Vec::new(),
                created_at: "2026-09-20T15:01:00Z".into(),
                author_context: None,
                anchor: None,
                reply_to: None,
                opinion: None,
                mentions_user: false,
                notifies_user: false,
            });
        }
        write
    }

    /// One write is one action, however many events it carried.
    #[test]
    fn one_write_is_one_action_however_many_events_it_carried() {
        let write = write_with(
            vec![
                TaskEventKind::Created,
                TaskEventKind::Tracked,
                TaskEventKind::Dispatched,
            ],
            false,
        );
        let action = task_action(&write).unwrap();
        assert_eq!(action.action, "created");
        assert_eq!(label(&action.action), "Created");
        assert_eq!(action.number, 13);
        assert_eq!(action.comment_id, None);
    }

    /// A comment carries the id that deep-links it.
    #[test]
    fn a_comment_carries_the_id_that_links_it() {
        let action = task_action(&write_with(Vec::new(), true)).unwrap();
        assert_eq!(action.action, "commented_on");
        assert_eq!(action.comment_id.as_deref(), Some("tc-9"));
    }

    /// A move names the column it went to, so the agent's own line can say
    /// "Moved #13 to “In review”" (#323). Nothing else carries one.
    #[test]
    fn a_move_carries_the_column_it_went_to() {
        let mut write = write_with(vec![TaskEventKind::Moved], false);
        write.events[0].payload = json!({ "from": "ready", "to": "in_review" });
        let action = task_action(&write).unwrap();
        assert_eq!(action.action, "moved");
        assert_eq!(action.to.as_deref(), Some("in_review"));
        let closed = task_action(&write_with(vec![TaskEventKind::Closed], false)).unwrap();
        assert_eq!(closed.to, None);
    }

    /// Tracking is about what the agent will READ, not what it did, so it says
    /// nothing in the agent's own conversation.
    #[test]
    fn tracking_alone_says_nothing() {
        assert!(task_action(&write_with(vec![TaskEventKind::Tracked], false)).is_none());
        assert!(task_action(&write_with(Vec::new(), false)).is_none());
    }

    /// Each remaining kind reads as itself.
    #[test]
    fn every_other_kind_names_what_it_was() {
        for (kind, slug) in [
            (TaskEventKind::Moved, "moved"),
            (TaskEventKind::Closed, "closed"),
            (TaskEventKind::Reopened, "reopened"),
            (TaskEventKind::Linked, "linked"),
            (TaskEventKind::Labelled, "updated"),
            (TaskEventKind::Unassigned, "unassigned"),
        ] {
            let action = task_action(&write_with(vec![kind], false)).unwrap();
            assert_eq!(action.action, slug, "{kind:?}");
        }
    }
}
