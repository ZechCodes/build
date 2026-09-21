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
        let called = self.agent_display_name(&write.actor);
        let Some(notice) = notice_of(write, called.clone()) else {
            // A write that changed nothing the timeline records is not news.
            return;
        };
        let who = self.actor_said_as(&write.actor);
        let envelope = notice_envelope(&write.issue);
        let holder = write
            .issue
            .assignee
            .as_ref()
            .and_then(crate::tracker::Assignee::agent_id)
            .map(str::to_string);
        for agent_id in told {
            // The line differs for the one agent that HOLDS the issue: a
            // comment on your own issue is a question, and a comment on one
            // you are watching is news. Everyone else gets the same words.
            let body = notice_body(
                &notice,
                &write.issue,
                &who,
                holder.as_deref() == Some(&agent_id),
            );
            if let Err(why) = self.deliver_notice(&agent_id, &envelope, &notice, &body) {
                eprintln!(
                    "notify {agent_id} about issue #{}: {why}",
                    write.issue.number
                );
            }
        }
    }

    /// What an acting agent is CALLED, when it has been named. `None` for the
    /// user and for an agent nobody has named.
    fn agent_display_name(&self, actor: &Actor) -> Option<String> {
        let Actor::Agent { agent_id } = actor else {
            return None;
        };
        let entity_id = self.entity_of_agent(agent_id)?;
        self.entity_agents(&entity_id)
            .ok()?
            .by_id(agent_id)
            .and_then(|agent| agent.name.clone())
    }

    /// Who a notice says did it, in the words the conversation uses, and
    /// phrased to sit mid-sentence: "…by the user", "…from Rail scroll".
    ///
    /// An agent is named the way its conversation is named — by the name it
    /// was given, else the workspace it works in — because the reader wants to
    /// know which of its colleagues it was and an id is something to go and
    /// look up.
    fn actor_said_as(&self, actor: &Actor) -> String {
        let Actor::Agent { agent_id } = actor else {
            return "the user".to_string();
        };
        if let Some(name) = self.agent_display_name(actor) {
            return name;
        }
        let Some(entity_id) = self.entity_of_agent(agent_id) else {
            return format!("agent {agent_id}");
        };
        match self.agent_identity(&entity_id, agent_id).owner {
            Some(owner) if owner.kind == crate::thread::AgentOwnerKind::Project => {
                format!("the {} project's agent", owner.name)
            }
            Some(owner) => format!("the {} agent", owner.name),
            None => format!("agent {agent_id}"),
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
fn notice_of(write: &IssueWrite, actor_name: Option<String>) -> Option<IssueNotice> {
    let plain = |action: &str| IssueNotice {
        actor: crate::thread::NoticeActor {
            who: write.actor.clone(),
            name: actor_name.clone(),
        },
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

/// The notice, as one line and nothing else.
///
/// A notification, not the thing itself (Zech, 2026-09-20: "Least context
/// necessary so if the agent is watching for a status change not a comment it
/// knows to ignore"). The comment's words are NOT here: an agent watching an
/// issue for a column move paid for every comment anybody wrote on it, and the
/// one that cares reads it with `read_comment` for the same cost it used to
/// pay whether it cared or not.
///
/// No title either. The number is what the issue is called between agents, and
/// the envelope carries the title for a client that draws a card.
///
/// `holds_it` is the one agent this issue is assigned to. A comment on your
/// own issue is a question and the line says where to answer it; the same
/// comment to a watcher is news.
fn notice_body(notice: &IssueNotice, issue: &Issue, who: &str, holds_it: bool) -> String {
    let number = issue.number;
    match notice.action.as_str() {
        "commented" => {
            let comment = notice.comment_id.as_deref().unwrap_or("");
            if holds_it {
                format!(
                    "New comment {comment} on #{number} from {who} — read_comment for their \
                     message, answer on the issue with comment_issue."
                )
            } else {
                format!(
                    "New comment {comment} on #{number} from {who} — read_comment for their \
                     message."
                )
            }
        }
        "moved" => match notice.to.as_deref() {
            Some(to) => format!("#{number} moved to {} by {who}.", column_name(to)),
            None => format!("#{number} moved by {who}."),
        },
        "assigned" if holds_it => format!("#{number} assigned to you by {who}."),
        "assigned" => match notice.assignee.as_ref().map(assignee_name) {
            Some(to) => format!("#{number} assigned to {to} by {who}."),
            None => format!("#{number} assigned by {who}."),
        },
        "unassigned" => format!("#{number} unassigned by {who}."),
        "created" => format!("#{number} created by {who}."),
        "closed" => format!("#{number} closed by {who}."),
        "reopened" => format!("#{number} reopened by {who}."),
        "linked" => format!("#{number} linked by {who}."),
        _ => format!("#{number} edited by {who}."),
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

    /// The line a watcher gets.
    fn body_of(write: &IssueWrite, who: &str) -> String {
        let notice = notice_of(write, None).unwrap();
        notice_body(&notice, &write.issue, who, false)
    }

    /// And the line the agent HOLDING it gets.
    fn body_for_holder(write: &IssueWrite, who: &str) -> String {
        let notice = notice_of(write, None).unwrap();
        notice_body(&notice, &write.issue, who, true)
    }

    /// A move carries both columns as slugs for a client to render, and reads
    /// as one line naming the column it landed in.
    #[test]
    fn a_move_is_one_line_naming_the_column_it_landed_in() {
        let mut write = write_by(Actor::User);
        write.event(
            &Actor::User,
            IssueEventKind::Moved,
            json!({ "from": "backlog", "to": "in_review" }),
            "2026-09-20T15:01:00Z",
        );
        let notice = notice_of(&write, None).unwrap();
        assert_eq!(notice.action, "moved");
        assert_eq!(notice.from.as_deref(), Some("backlog"));
        assert_eq!(notice.to.as_deref(), Some("in_review"));
        assert_eq!(
            body_of(&write, "the user"),
            "#13 moved to In review by the user."
        );
    }

    /// A comment notice carries the comment's ID and NOT its words.
    ///
    /// The whole point of #61: an agent watching an issue for a column move
    /// paid for every comment anybody wrote on it. Now it pays for a line, and
    /// reads the words with `read_comment` only if it decides it cares.
    #[test]
    fn a_comment_notice_names_the_comment_and_carries_none_of_it() {
        let mut write = write_by(Actor::Agent {
            agent_id: "agent-1".into(),
        });
        write.comments.push(crate::tracker::IssueComment {
            id: "ic-1".into(),
            issue_id: write.issue.id.clone(),
            author: Actor::Agent {
                agent_id: "agent-1".into(),
            },
            body: "Reproduced it on the compose stack.".into(),
            refs: Vec::new(),
            created_at: "2026-09-20T15:01:00Z".into(),
        });
        let notice = notice_of(&write, None).unwrap();
        assert_eq!(notice.action, "commented");
        assert_eq!(notice.comment_id.as_deref(), Some("ic-1"));

        let watching = body_of(&write, "Rail scroll");
        assert_eq!(
            watching,
            "New comment ic-1 on #13 from Rail scroll — read_comment for their message."
        );
        assert!(
            !watching.contains("Reproduced it"),
            "the comment's words are not in the notice: {watching}"
        );

        // The agent that HOLDS the issue is being asked something, and the
        // line says where to answer.
        let holding = body_for_holder(&write, "the user");
        assert!(
            holding.starts_with("New comment ic-1 on #13 from the user"),
            "{holding}"
        );
        assert!(
            holding.ends_with("answer on the issue with comment_issue."),
            "{holding}"
        );
        assert!(!holding.contains("Reproduced it"), "{holding}");
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
        assert!(notice_of(&write, None).is_none());
    }

    /// An assignment names who got it — and tells the one who got it that they
    /// got it, which is the fact they most need off that line.
    #[test]
    fn an_assignment_names_whoever_got_it_and_says_when_it_is_you() {
        for (assignee, expected) in [
            (json!({ "kind": "user" }), "the user"),
            (json!({ "kind": "project_agent" }), "the project's agent"),
            (json!({ "kind": "agent", "agent_id": "agent-9" }), "agent-9"),
        ] {
            let mut write = write_by(Actor::User);
            write.issue.assignee = serde_json::from_value(assignee.clone()).ok();
            write.event(
                &Actor::User,
                IssueEventKind::Assigned,
                json!({ "assignee": assignee }),
                "2026-09-20T15:01:00Z",
            );
            assert_eq!(
                body_of(&write, "the user"),
                format!("#13 assigned to {expected} by the user.")
            );
            assert_eq!(
                body_for_holder(&write, "the user"),
                "#13 assigned to you by the user."
            );
        }
    }

    /// Every other kind is one line, and none of them carries a title: the
    /// number is what an issue is called between agents, and the envelope
    /// carries the title for a client drawing a card.
    #[test]
    fn every_other_kind_is_one_line_with_no_title() {
        for (kind, expected) in [
            (IssueEventKind::Closed, "#13 closed by the user."),
            (IssueEventKind::Reopened, "#13 reopened by the user."),
            (IssueEventKind::Linked, "#13 linked by the user."),
            (IssueEventKind::Labelled, "#13 edited by the user."),
            (IssueEventKind::Unassigned, "#13 unassigned by the user."),
            (IssueEventKind::Created, "#13 created by the user."),
        ] {
            let mut write = write_by(Actor::User);
            write.event(&Actor::User, kind, json!({}), "2026-09-20T15:01:00Z");
            let said = body_of(&write, "the user");
            assert_eq!(said, expected, "{kind:?}");
            assert!(
                !said.contains("Kanban drag"),
                "no title on the line: {said}"
            );
            assert_eq!(said.lines().count(), 1, "one line: {said}");
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
