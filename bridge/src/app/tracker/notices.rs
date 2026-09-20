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
use crate::thread::{IssueEnvelope, IssueNotice};
use crate::tracker::{Actor, Issue, IssueEventKind};

impl AppState {
    /// Tell everyone watching, except whoever did it.
    pub(in crate::app) fn notify_trackers(&mut self, write: &IssueWrite) {
        let told = write.issue.trackers_to_notify(&write.actor);
        if told.is_empty() {
            return;
        }
        let Some(notice) = notice_of(write) else {
            // A write that changed nothing the timeline records is not news.
            return;
        };
        let body = notice_body(
            &notice,
            &write.issue,
            &self.actor_label(&write.actor),
            write.comments.first().map(|comment| comment.body.as_str()),
        );
        let envelope = notice_envelope(&write.issue);
        for agent_id in told {
            if let Err(why) = self.deliver_notice(&agent_id, &envelope, &notice, &body) {
                eprintln!(
                    "notify {agent_id} about issue #{}: {why}",
                    write.issue.number
                );
            }
        }
    }

    /// Who a notice says did it, in the words the conversation uses.
    ///
    /// The same naming an assignment notice uses: an agent is named by the
    /// workspace it works in, because the reader wants to know which of its
    /// colleagues moved the card and an id is something to go and look up.
    fn actor_label(&self, actor: &Actor) -> String {
        let Actor::Agent { agent_id } = actor else {
            return "The user".to_string();
        };
        let Some(entity_id) = self.entity_of_agent(agent_id) else {
            return format!("Agent {agent_id}");
        };
        let identity = self.agent_identity(&entity_id, agent_id);
        match identity.owner {
            Some(owner) if owner.kind == crate::thread::AgentOwnerKind::Project => {
                format!("The {} project's agent", owner.name)
            }
            Some(owner) => format!("The {} agent", owner.name),
            None => format!("Agent {agent_id}"),
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
        notice: &IssueNotice,
        body: &str,
    ) -> Result<(), String> {
        let entity_id = self
            .entity_of_agent(agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        let addressed = self.addressed_agent(&serde_json::json!({
            "id": entity_id,
            "agent_id": agent_id,
        }))?;
        let now = crate::store::now_rfc3339();
        let body = body.to_string();
        let envelope = envelope.clone();
        let notice = notice.clone();
        self.edit_agent_conversation(&entity_id, agent_id, |thread, _| {
            thread.post_user_from_build(body, &now);
            // Both: the envelope says WHICH issue, the notice says what
            // happened to it, and one line that links the right thing needs
            // the two together.
            thread.wear_issue(envelope);
            thread.wear_issue_notice(notice);
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

/// What changed, as a client reads it: the actor, the action and whatever
/// detail that action has.
///
/// Derived from the events the write carried rather than from the verb that
/// made it, so the notice and the timeline cannot disagree about what
/// happened: they are reading the same record. `None` is a write the timeline
/// records nothing for — which is not news.
fn notice_of(write: &IssueWrite) -> Option<IssueNotice> {
    let plain = |action: &str| IssueNotice {
        actor: write.actor.clone(),
        action: action.to_string(),
        comment_id: None,
        from: None,
        to: None,
        assignee: None,
    };
    if let Some(comment) = write.comments.first() {
        return Some(IssueNotice {
            comment_id: Some(comment.id.clone()),
            ..plain("commented")
        });
    }
    let named = |payload: &serde_json::Value, key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    write.events.iter().find_map(|event| match event.kind {
        IssueEventKind::Created => Some(plain("created")),
        IssueEventKind::Moved => Some(IssueNotice {
            from: named(&event.payload, "from"),
            to: named(&event.payload, "to"),
            ..plain("moved")
        }),
        IssueEventKind::Assigned => Some(IssueNotice {
            assignee: event
                .payload
                .get("assignee")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            ..plain("assigned")
        }),
        IssueEventKind::Unassigned => Some(plain("unassigned")),
        IssueEventKind::Labelled => Some(plain("edited")),
        IssueEventKind::Linked => Some(plain("linked")),
        IssueEventKind::Closed => Some(plain("closed")),
        IssueEventKind::Reopened => Some(plain("reopened")),
        IssueEventKind::Dispatched => Some(plain("assigned")),
        // Who else is watching is not a change to the issue.
        IssueEventKind::Tracked | IssueEventKind::Untracked => None,
    })
}

/// The same thing in one line of prose, for a harness — which gets the body or
/// nothing — and as the fallback for a client that has not learned
/// `issue_notice` yet.
///
/// Reads as "X did Y on #N Title", with a comment's words under it: the point
/// of hearing about a comment is reading it, and a notice that made the reader
/// go and fetch it would have cost them the trip it exists to save.
fn notice_body(notice: &IssueNotice, issue: &Issue, who: &str, comment: Option<&str>) -> String {
    let issue_named = format!("#{} {}", issue.number, issue.title);
    let line = match notice.action.as_str() {
        "commented" => format!("{who} commented on {issue_named}"),
        "moved" => match notice.to.as_deref() {
            Some(to) => format!("{who} moved {issue_named} to {}", column_name(to)),
            None => format!("{who} moved {issue_named}"),
        },
        "assigned" => match notice.assignee.as_ref().map(assignee_name) {
            Some(to) => format!("{who} assigned {issue_named} to {to}"),
            None => format!("{who} assigned {issue_named}"),
        },
        "unassigned" => format!("{who} unassigned {issue_named}"),
        "created" => format!("{who} created {issue_named}"),
        "closed" => format!("{who} closed {issue_named}"),
        "reopened" => format!("{who} reopened {issue_named}"),
        "linked" => format!("{who} linked {issue_named}"),
        _ => format!("{who} edited {issue_named}"),
    };
    match comment.map(str::trim).filter(|body| !body.is_empty()) {
        Some(body) => format!("{line}\n\n{body}"),
        None => line,
    }
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

fn assignee_name(assignee: &crate::tracker::Assignee) -> String {
    match assignee {
        crate::tracker::Assignee::User => "the user".to_string(),
        crate::tracker::Assignee::ProjectAgent => "the project's agent".to_string(),
        crate::tracker::Assignee::Agent { agent_id } => agent_id.clone(),
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

    fn body_of(write: &IssueWrite, who: &str) -> String {
        let notice = notice_of(write).unwrap();
        notice_body(
            &notice,
            &write.issue,
            who,
            write.comments.first().map(|comment| comment.body.as_str()),
        )
    }

    /// A move carries both columns as slugs for a client to render, and reads
    /// as the column's NAME in the line a harness gets.
    #[test]
    fn a_move_carries_both_columns_and_names_the_one_it_landed_in() {
        let mut write = write_by(Actor::User);
        write.event(
            &Actor::User,
            IssueEventKind::Moved,
            json!({ "from": "backlog", "to": "in_review" }),
            "2026-09-20T15:01:00Z",
        );
        let notice = notice_of(&write).unwrap();
        assert_eq!(notice.action, "moved");
        assert_eq!(notice.from.as_deref(), Some("backlog"));
        assert_eq!(notice.to.as_deref(), Some("in_review"));
        assert_eq!(notice.actor, Actor::User);
        assert_eq!(
            body_of(&write, "The user"),
            "The user moved #13 Kanban drag to In review"
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
        let notice = notice_of(&write).unwrap();
        assert_eq!(notice.action, "commented");
        assert_eq!(
            notice.comment_id.as_deref(),
            Some("ic-1"),
            "so a client links the comment and not the issue"
        );
        let body = body_of(&write, "The wire-facade agent");
        assert!(
            body.starts_with("The wire-facade agent commented on #13 Kanban drag"),
            "{body}"
        );
        assert!(body.ends_with("Reproduced it."), "{body}");
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
        assert!(notice_of(&write).is_none());
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
            let notice = notice_of(&write).unwrap();
            assert_eq!(notice.action, "assigned");
            assert_eq!(
                notice.assignee,
                Some(serde_json::from_value(assignee).unwrap()),
                "a client draws who got it without parsing the line"
            );
            assert_eq!(
                body_of(&write, "The user"),
                format!("The user assigned #13 Kanban drag to {expected}")
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
