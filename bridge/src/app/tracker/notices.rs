//! Telling the agents watching an issue that it moved (spec: Issues →
//! Tracking).
//!
//! What tracking is FOR. A change to a tracked issue puts one message on each
//! tracker's conversation and starts its turn, so an agent collaborating on an
//! issue neither polls for it nor has to stay awake.
//!
//! Three rules hold the whole thing up, and each is a test:
//!
//! 1. **Never to the actor.** An agent woken to be told what it just did would
//!    answer its own message, and two agents tracking each other's issues would
//!    do it forever.
//! 2. **One notice per write**, however many events the write carried. A call
//!    that relabels and moves is one thing that happened to the issue, and two
//!    messages about it would be two interruptions for one change.
//! 3. **Quiet about its own failure.** The change succeeded and is durable
//!    before any of this runs; a conversation that could not be written must
//!    not turn a landed change into a refused one.

use super::IssueWrite;
use crate::app::{AppState, PendingAgentTurn, TurnText, NEW_THREAD_MESSAGES_PROMPT};
use crate::thread::IssueEnvelope;
use crate::tracker::{Actor, Issue, IssueEventKind};

impl AppState {
    /// Tell everyone watching, except whoever did it.
    pub(in crate::app) fn notify_trackers(&mut self, write: &IssueWrite) {
        let told = write.issue.trackers_to_notify(&write.actor);
        if told.is_empty() {
            return;
        }
        let Some(summary) = notice_summary(write) else {
            // A write that changed nothing the timeline records is not news.
            return;
        };
        let envelope = notice_envelope(&write.issue);
        for agent_id in told {
            if let Err(why) = self.deliver_notice(&agent_id, &envelope, &summary) {
                eprintln!(
                    "notify {agent_id} about issue #{}: {why}",
                    write.issue.number
                );
            }
        }
    }

    /// Put one notice on one agent's conversation and start its turn.
    ///
    /// The same two steps the restart notice takes: the message goes on the
    /// thread BEFORE the turn is queued, so a cold session's catch-up packet
    /// already contains it and the agent reads it as the newest thing said.
    fn deliver_notice(
        &mut self,
        agent_id: &str,
        envelope: &IssueEnvelope,
        summary: &str,
    ) -> Result<(), String> {
        let entity_id = self
            .entity_of_agent(agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        let addressed = self.addressed_agent(&serde_json::json!({
            "id": entity_id,
            "agent_id": agent_id,
        }))?;
        let now = crate::store::now_rfc3339();
        let body = summary.to_string();
        let envelope = envelope.clone();
        self.edit_agent_conversation(&entity_id, agent_id, |thread, _| {
            thread.post_user_from_build(body, &now);
            thread.wear_issue(envelope);
            Ok(serde_json::Value::Null)
        })?;
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: addressed.root.clone(),
            owner: addressed.entity_id.clone(),
            agent_id: addressed.agent_id.clone(),
            conversation_id: addressed.conversation_id.clone(),
            model_choice: addressed.model_choice.clone(),
            choice_revision: addressed.choice_revision,
            interrupt: false,
            // The notice is already on the thread, so the turn says what every
            // other unread message says: go and read it.
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "issue_notice",
            wants_catch_up: true,
            survives_refusal: false,
        });
        Ok(())
    }
}

/// The issue as a notice wears it: enough to recognise it and link it, and not
/// the body.
///
/// A dispatched issue carries its whole body because the agent is being handed
/// the work and has to read it. A tracker is being told something moved; it
/// has the issue already, and thirty notices each carrying a body is thirty
/// copies of a thing that has not changed.
fn notice_envelope(issue: &Issue) -> IssueEnvelope {
    IssueEnvelope {
        issue_id: issue.id.clone(),
        number: issue.number,
        title: issue.title.clone(),
        links: issue.links.clone(),
    }
}

/// One line saying what changed and who changed it, with a comment's words
/// under it when the change is a comment.
///
/// Derived from the events the write carried rather than from the verb that
/// made it, so the notice and the timeline cannot disagree about what
/// happened: they are reading the same record.
///
/// `None` is a write the timeline records nothing for — which is not news.
fn notice_summary(write: &IssueWrite) -> Option<String> {
    let who = actor_name(&write.actor);
    let number = write.issue.number;
    if let Some(comment) = write.comments.first() {
        return Some(format!(
            "#{number} {} — {who} commented:\n\n{}",
            write.issue.title,
            comment.body.trim()
        ));
    }
    let what = write
        .events
        .iter()
        .find_map(|event| change_phrase(event.kind, &event.payload))?;
    Some(format!("#{number} {} — {what} by {who}", write.issue.title))
}

/// How one event reads in a notice. `None` for the kinds a tracker is not told
/// about: somebody else starting or stopping watching is not a change to the
/// issue.
fn change_phrase(kind: IssueEventKind, payload: &serde_json::Value) -> Option<String> {
    let named = |key: &str| payload.get(key).and_then(serde_json::Value::as_str);
    Some(match kind {
        IssueEventKind::Created => "created".to_string(),
        IssueEventKind::Moved => match named("to") {
            Some(to) => format!("moved to {}", column_name(to)),
            None => "moved".to_string(),
        },
        IssueEventKind::Assigned => match payload.get("assignee").and_then(assignee_name) {
            Some(to) => format!("assigned to {to}"),
            None => "assigned".to_string(),
        },
        IssueEventKind::Unassigned => "unassigned".to_string(),
        IssueEventKind::Labelled => "relabelled".to_string(),
        IssueEventKind::Linked => "linked".to_string(),
        IssueEventKind::Closed => "closed".to_string(),
        IssueEventKind::Reopened => "reopened".to_string(),
        IssueEventKind::Dispatched => "dispatched".to_string(),
        // Who else is watching is not a change to the issue.
        IssueEventKind::Tracked | IssueEventKind::Untracked => return None,
    })
}

/// A column's display name, so a notice reads "moved to In review" rather than
/// naming the slug a client is supposed to render.
fn column_name(slug: &str) -> &str {
    crate::tracker::COLUMNS
        .iter()
        .find(|column| column.id == slug)
        .map(|column| column.name)
        .unwrap_or(slug)
}

fn assignee_name(assignee: &serde_json::Value) -> Option<String> {
    match assignee.get("kind").and_then(serde_json::Value::as_str)? {
        "user" => Some("the user".to_string()),
        "project_agent" => Some("the project's agent".to_string()),
        _ => assignee
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

fn actor_name(actor: &Actor) -> String {
    match actor {
        Actor::User => "the user".to_string(),
        Actor::Agent { agent_id } => agent_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue() -> Issue {
        let mut issue = Issue::drafted("/repo", "Kanban drag", Actor::User, "2026-09-20T15:00:00Z");
        issue.number = 13;
        issue
    }

    fn write_by(actor: Actor) -> IssueWrite {
        IssueWrite {
            issue: issue(),
            comments: Vec::new(),
            events: Vec::new(),
            actor,
        }
    }

    /// A move reads as the column's NAME, not the slug a client renders.
    #[test]
    fn a_move_names_the_column_a_reader_would_recognise() {
        let mut write = write_by(Actor::User);
        write.event(
            &Actor::User,
            IssueEventKind::Moved,
            json!({ "from": "backlog", "to": "in_review" }),
            "2026-09-20T15:01:00Z",
        );
        assert_eq!(
            notice_summary(&write).unwrap(),
            "#13 Kanban drag — moved to In review by the user"
        );
    }

    /// A comment's words ride the notice: the point of hearing about a comment
    /// is reading it.
    #[test]
    fn a_comment_carries_its_body_into_the_notice() {
        let mut write = write_by(Actor::Agent {
            agent_id: "agent-1".into(),
        });
        write.comments.push(crate::tracker::IssueComment {
            id: "ic-1".into(),
            issue_id: write.issue.id.clone(),
            author: Actor::Agent {
                agent_id: "agent-1".into(),
            },
            body: "  Reproduced it.  ".into(),
            refs: Vec::new(),
            created_at: "2026-09-20T15:01:00Z".into(),
        });
        let summary = notice_summary(&write).unwrap();
        assert!(summary.starts_with("#13 Kanban drag — agent-1 commented:"));
        assert!(summary.ends_with("Reproduced it."), "{summary}");
    }

    /// Somebody else starting to watch is not a change to the issue, so it is
    /// not news and no notice goes out for it.
    #[test]
    fn a_tracking_change_alone_is_not_news() {
        let mut write = write_by(Actor::User);
        write.event(
            &Actor::User,
            IssueEventKind::Tracked,
            json!({ "agent_id": "agent-2" }),
            "2026-09-20T15:01:00Z",
        );
        assert_eq!(notice_summary(&write), None);
    }

    /// An assignment names who got it, including the two assignee kinds that
    /// are not an agent id.
    #[test]
    fn an_assignment_names_whoever_got_it() {
        for (assignee, expected) in [
            (json!({ "kind": "user" }), "the user"),
            (json!({ "kind": "project_agent" }), "the project's agent"),
            (json!({ "kind": "agent", "agent_id": "agent-9" }), "agent-9"),
        ] {
            let mut write = write_by(Actor::User);
            write.event(
                &Actor::User,
                IssueEventKind::Assigned,
                json!({ "assignee": assignee }),
                "2026-09-20T15:01:00Z",
            );
            assert_eq!(
                notice_summary(&write).unwrap(),
                format!("#13 Kanban drag — assigned to {expected} by the user")
            );
        }
    }

    /// The envelope names the issue and does not repeat it: a tracker has the
    /// issue already, and thirty notices each carrying a copy is thirty copies
    /// of what did not change.
    #[test]
    fn a_notice_envelope_identifies_the_issue_without_repeating_it() {
        let mut issue = issue();
        issue.body = "a long description".into();
        let envelope = notice_envelope(&issue);
        assert_eq!(envelope.number, 13);
        assert_eq!(envelope.title, "Kanban drag");
        assert!(
            !serde_json::to_string(&envelope)
                .unwrap()
                .contains("a long description"),
            "the body is not carried"
        );
    }
}
