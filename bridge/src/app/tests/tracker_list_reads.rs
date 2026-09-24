//! `issues.list` over a project with a history (#128): every listed issue
//! answers exactly what reading that issue alone answers, however the list
//! gathers the timelines and the agent records behind it.

use super::project_agent::workspace;
use super::tracker::{filed, tracked_with_origin};
use super::*;
use crate::tracker::{Actor, Assignee, IssueComment, IssueEvent, IssueEventKind};

const ISSUES: usize = 50;

/// An agent on a checkout of its own, named so its identity has something to
/// carry.
fn agent_on_checkout(state: &mut AppState, project_id: &str, name: &str) -> (String, String) {
    let ws = workspace(state, project_id, name);
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    ));
    let entity_id = ensured["result"]["entity_id"].as_str().unwrap().to_string();
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "provider": "pi" }),
    ));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    state.set_agent_name(&entity_id, &agent_id, name).unwrap();
    (entity_id, agent_id)
}

/// Who an issue's history names, by where the name comes from: prose, an
/// assignment, a comment's author, an event's actor.
struct Cast {
    live: String,
    departed: String,
}

impl Cast {
    fn prose(&self, index: usize) -> String {
        match index % 4 {
            0 => format!("Ask @agent:{}.", self.live),
            1 => format!("Ask @agent:{}.", self.departed),
            2 => "Ask @agent:agent-ghost.".into(),
            _ => String::new(),
        }
    }

    fn author(&self, index: usize) -> Actor {
        let agent_id = if index.is_multiple_of(3) {
            &self.departed
        } else {
            &self.live
        };
        Actor::Agent {
            agent_id: agent_id.clone(),
        }
    }
}

/// A few comments and events per issue, stamped out of insertion order.
fn activity(cast: &Cast, issue_id: &str, index: usize) -> (Vec<IssueComment>, Vec<IssueEvent>) {
    let at = |minute: usize| format!("2026-09-01T10:{:02}:00Z", (index + minute) % 60);
    let comments = (0..1 + index % 3)
        .rev()
        .map(|n| IssueComment {
            id: crate::tracker::new_comment_id(),
            issue_id: issue_id.to_string(),
            author: cast.author(index + n),
            body: format!("note {n}"),
            mentions_user: false,
            refs: Vec::new(),
            attachments: Vec::new(),
            created_at: at(n * 2),
            author_context: None,
        })
        .collect();
    let events = (0..index % 3)
        .map(|n| {
            IssueEvent::new(
                issue_id,
                cast.author(index + n + 1),
                IssueEventKind::Labelled,
                json!({ "added": [format!("l{n}")], "removed": [] }),
                &at(n * 3),
            )
        })
        .collect();
    (comments, events)
}

/// Fifty issues whose records predate identities, so the list is also the
/// read that fills them in.
fn history(state: &mut AppState, project_id: &str, cast: &Cast) -> Vec<String> {
    (0..ISSUES)
        .map(|index| {
            let id = filed(state, project_id, &format!("issue {index}"))["id"]
                .as_str()
                .unwrap()
                .to_string();
            let store = state.tracker_store().unwrap();
            let mut issue = store.load_tracker_issue(&id).unwrap().unwrap();
            issue.body = cast.prose(index);
            if index.is_multiple_of(5) {
                issue.labels = vec!["sweep".into()];
            }
            if index.is_multiple_of(7) {
                issue.assignee = Some(Assignee::Agent {
                    agent_id: cast.departed.clone(),
                });
            }
            issue.identities.clear();
            let (comments, events) = activity(cast, &id, index);
            store
                .save_tracker_issue_activity(&issue, &comments, &events)
                .unwrap();
            id
        })
        .collect()
}

fn stored_records(state: &AppState, ids: &[String]) -> Vec<crate::tracker::Issue> {
    let store = state.tracker_store().unwrap();
    ids.iter()
        .map(|id| store.load_tracker_issue(id).unwrap().unwrap())
        .collect()
}

/// The list's rows, and what `issues.get` answers for the same issues in the
/// same order — the one-issue read, which loads its own timeline.
fn listed_and_each_alone(state: &mut AppState, params: Value) -> (Value, Value) {
    let listed = state.handle(req("issues.list", params));
    assert_eq!(listed["ok"], true, "{listed:?}");
    let rows = listed["result"]["issues"].clone();
    let alone = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let got = state.handle(req("issues.get", json!({ "issue_id": row["id"] })));
            got["result"]["issue"].clone()
        })
        .collect();
    (rows, Value::Array(alone))
}

#[test]
fn a_long_list_answers_what_each_issue_answers_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let (_, live) = agent_on_checkout(&mut state, &project_id, "Keeper");
    let (departed_entity, departed) = agent_on_checkout(&mut state, &project_id, "Historian");
    // Only the store still knows this one: its run left the live roster.
    state.runs.remove(&departed_entity);
    let cast = Cast { live, departed };
    let ids = history(&mut state, &project_id, &cast);

    let (rows, alone) = listed_and_each_alone(&mut state, json!({ "project_id": project_id }));
    let filled = stored_records(&state, &ids);

    let mut newest_first = ids.clone();
    newest_first.reverse();
    let listed_ids: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed_ids, newest_first, "newest first, every issue once");
    assert_eq!(rows, alone, "each row is what reading the issue alone says");
    let departed_name = rows.as_array().unwrap().iter().find_map(|row| {
        row["identities"][&cast.departed]["name"]
            .as_str()
            .map(str::to_string)
    });
    assert_eq!(departed_name.as_deref(), Some("Historian"));
    assert!(rows.as_array().unwrap().iter().any(|row| {
        row["identities"]["agent-ghost"]["available"] == false
            && row["identities"]["agent-ghost"]["name"].is_null()
    }));
    assert!(
        filled
            .iter()
            .filter(|issue| !issue.identities.is_empty())
            .count()
            >= ISSUES / 2,
        "the list fills identities into the records it read"
    );
    assert_eq!(
        stored_records(&state, &ids),
        filled,
        "reading each issue alone found nothing the list had not already filled"
    );

    let (again, _) = listed_and_each_alone(&mut state, json!({ "project_id": project_id }));
    assert_eq!(again, rows, "a second list answers the same");

    let (labelled, labelled_alone) = listed_and_each_alone(
        &mut state,
        json!({ "project_id": project_id, "label": "sweep" }),
    );
    assert_eq!(labelled.as_array().unwrap().len(), ISSUES / 5);
    assert_eq!(labelled, labelled_alone);
}
