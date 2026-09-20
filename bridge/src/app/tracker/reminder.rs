//! What an agent still holds when it says it is done (spec: Issues → The
//! Complete reminder).
//!
//! An agent reports Complete and walks away from three open issues assigned to
//! it. Nobody is told, the issues sit in In progress forever, and whoever
//! assigned them finds out by going to look. So on Complete — and only on
//! Complete — Build says what is still open and hands the turn back.
//!
//! Not on Blocked and not on Waiting. Both of those are the agent saying it
//! cannot finish, which is already an answer about the work; adding a list of
//! what it has not finished would be telling it what it just told us.
//!
//! Said once per set, not once per Complete. The reminder is delivered as a
//! TURN, so an agent that answers it reports Complete again — which is another
//! reminder, which is another answer. That loop ran five times on #27 in three
//! minutes before the agent worked out what was happening and stopped
//! replying. So the same list is not sent twice: a Complete holding what the
//! agent was last told about says nothing, and a set that has changed is news
//! again.
//!
//! The way out is to finish them, say something on them, or hand them back,
//! and all three are one tool call.

use crate::app::{AppState, PendingAgentTurn, TurnText, NEW_THREAD_MESSAGES_PROMPT};
use crate::store::IssueFilter;
use crate::tracker::{Issue, IssueState, STILL_TO_FINISH};

impl AppState {
    /// An agent reported Complete: tell it what it still holds.
    ///
    /// Quiet about its own failure, for the reason every other automatic
    /// write here is: the report is the agent's and the turn is over, and a
    /// conversation that could not be written must not turn a finished piece
    /// of work into a failed one.
    pub(in crate::app) fn remind_of_open_issues(&mut self, entity_id: &str, agent_id: &str) {
        let held = self.open_issues_held_by(entity_id, agent_id);
        if held.is_empty() {
            // Nothing held is also nothing to remember: an agent that finishes
            // everything and is later handed one more should hear about it.
            self.reminded_holdings.remove(agent_id);
            return;
        }
        let holding: Vec<String> = held.iter().map(|issue| issue.id.clone()).collect();
        if self.reminded_holdings.get(agent_id) == Some(&holding) {
            return;
        }
        self.reminded_holdings.insert(agent_id.to_string(), holding);
        if let Err(why) = self.deliver_reminder(entity_id, agent_id, &held) {
            eprintln!("remind {agent_id} of its open issues: {why}");
        }
    }

    /// The issues assigned to this agent that are still ITS to finish.
    ///
    /// [`STILL_TO_FINISH`] is the test rather than "open and not closed": an
    /// issue in In review has been handed on — the agent said so by reporting
    /// Complete — and one parked in Done is finished with whether or not
    /// anybody closed it. Naming either would make the reminder noise, and a
    /// reminder that is noise is one an agent learns to answer without
    /// reading.
    fn open_issues_held_by(&mut self, entity_id: &str, agent_id: &str) -> Vec<Issue> {
        let Some(project_id) = self.projects.project_id_of(entity_id).map(str::to_string) else {
            return Vec::new();
        };
        let Ok(project_path) = self.tracker_project_path(&project_id) else {
            return Vec::new();
        };
        let Ok(open) = self.tracker_store().and_then(|store| {
            store
                .list_tracker_issues(
                    &project_path,
                    IssueFilter {
                        state: Some(IssueState::Open),
                        status: None,
                    },
                )
                .map_err(|error| error.to_string())
        }) else {
            return Vec::new();
        };
        open.into_iter()
            .filter(|issue| {
                STILL_TO_FINISH.contains(&issue.status.as_str())
                    && issue
                        .assignee
                        .as_ref()
                        .and_then(crate::tracker::Assignee::agent_id)
                        == Some(agent_id)
            })
            .collect()
    }

    fn deliver_reminder(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        held: &[Issue],
    ) -> Result<(), String> {
        let addressed = self.addressed_agent(&serde_json::json!({
            "id": entity_id,
            "agent_id": agent_id,
        }))?;
        let body = reminder_body(held);
        let now = crate::store::now_rfc3339();
        self.edit_agent_conversation(entity_id, agent_id, |thread, _| {
            thread.post_user_from_build(body, &now);
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
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "issue_reminder",
            wants_catch_up: true,
            survives_refusal: false,
        });
        Ok(())
    }
}

/// What the reminder says.
///
/// It names every issue rather than counting them, because "you still hold 3
/// issues" makes the agent go and look and the looking is the part Build can
/// do. And it says the three ways out, because an agent told only that
/// something is unfinished will pick one of them at random — most often
/// reporting Complete again.
fn reminder_body(held: &[Issue]) -> String {
    let listed: Vec<String> = held
        .iter()
        .map(|issue| {
            format!(
                "- #{} {} ({})",
                issue.number,
                issue.title,
                column_name(&issue.status)
            )
        })
        .collect();
    let count = held.len();
    let these = if count == 1 { "issue" } else { "issues" };
    format!(
        "You reported Complete, but {count} {these} assigned to you {} still open. This \
         message is from Build, not from the user — nobody is waiting on an answer to it.\n\n{}\n\n\
         Each one needs finishing, or a comment saying where it got to, or — if you cannot do it \
         — handing back with a comment saying why, so whoever assigned it knows. Move an issue to \
         In review when it is ready to be looked at, and close it only when it is done with.",
        if count == 1 { "is" } else { "are" },
        listed.join("\n")
    )
}

fn column_name(slug: &str) -> &str {
    crate::tracker::COLUMNS
        .iter()
        .find(|column| column.id == slug)
        .map(|column| column.name)
        .unwrap_or(slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Actor;

    fn issue(number: u64, title: &str, status: &str) -> Issue {
        let mut issue = Issue::drafted("/repo", title, Actor::User, "2026-09-20T15:00:00Z");
        issue.number = number;
        issue.status = status.to_string();
        issue
    }

    /// It names every issue, with the column each is in, and says the three
    /// ways out.
    #[test]
    fn the_reminder_names_each_issue_and_what_to_do_about_it() {
        let body = reminder_body(&[
            issue(13, "Issue tracking", "in_progress"),
            issue(15, "The Complete reminder", "in_review"),
        ]);
        assert!(
            body.contains("2 issues assigned to you are still open"),
            "{body}"
        );
        assert!(
            body.contains("- #13 Issue tracking (In progress)"),
            "{body}"
        );
        assert!(
            body.contains("- #15 The Complete reminder (In review)"),
            "{body}"
        );
        assert!(body.contains("from Build, not from the user"), "{body}");
        assert!(body.contains("handing back"), "{body}");
    }

    /// One issue reads as one issue, not "1 issues".
    #[test]
    fn one_issue_reads_as_one() {
        let body = reminder_body(&[issue(13, "Issue tracking", "in_progress")]);
        assert!(
            body.contains("1 issue assigned to you is still open"),
            "one reads as one: {body}"
        );
    }
}
