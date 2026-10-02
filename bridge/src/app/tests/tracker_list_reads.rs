//! `tasks.list` over a project with a history (#128): every listed task
//! answers exactly what reading that task alone answers, however the list
//! gathers the timelines and the agent records behind it.

use super::project_agent::workspace;
use super::tracker::{filed, tracked_with_origin};
use super::*;
use crate::tracker::{Actor, Assignee, TaskComment, TaskEvent, TaskEventKind};

const TASKS: usize = 50;

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

/// Who a task's history names, by where the name comes from: prose, an
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

/// A few comments and events per task, stamped out of insertion order.
fn activity(cast: &Cast, task_id: &str, index: usize) -> (Vec<TaskComment>, Vec<TaskEvent>) {
    let at = |minute: usize| format!("2026-09-01T10:{:02}:00Z", (index + minute) % 60);
    let comments = (0..1 + index % 3)
        .rev()
        .map(|n| TaskComment {
            id: crate::tracker::new_comment_id(),
            task_id: task_id.to_string(),
            author: cast.author(index + n),
            body: format!("note {n}"),
            anchor: None,
            reply_to: None,
            opinion: None,
            mentions_user: false,
            notifies_user: false,
            refs: Vec::new(),
            attachments: Vec::new(),
            created_at: at(n * 2),
            author_context: None,
        })
        .collect();
    let events = (0..index % 3)
        .map(|n| {
            TaskEvent::new(
                task_id,
                cast.author(index + n + 1),
                TaskEventKind::Labelled,
                json!({ "added": [format!("l{n}")], "removed": [] }),
                &at(n * 3),
            )
        })
        .collect();
    (comments, events)
}

/// Fifty tasks whose records predate identities, so the list is also the
/// read that fills them in.
fn history(state: &mut AppState, project_id: &str, cast: &Cast) -> Vec<String> {
    (0..TASKS)
        .map(|index| {
            let id = filed(state, project_id, &format!("task {index}"))["id"]
                .as_str()
                .unwrap()
                .to_string();
            let store = state.tracker_store().unwrap();
            let mut task = store.load_tracker_task(&id).unwrap().unwrap();
            task.body = cast.prose(index);
            if index.is_multiple_of(5) {
                task.labels = vec!["sweep".into()];
            }
            if index.is_multiple_of(7) {
                task.assignee = Some(Assignee::Agent {
                    agent_id: cast.departed.clone(),
                });
            }
            task.identities.clear();
            let (comments, events) = activity(cast, &id, index);
            store
                .save_tracker_task_activity(&task, &comments, &events)
                .unwrap();
            id
        })
        .collect()
}

fn stored_records(state: &AppState, ids: &[String]) -> Vec<crate::tracker::Task> {
    let store = state.tracker_store().unwrap();
    ids.iter()
        .map(|id| store.load_tracker_task(id).unwrap().unwrap())
        .collect()
}

/// The list's rows, and what `tasks.get` answers for the same tasks in the
/// same order — the one-task read, which loads its own timeline.
fn listed_and_each_alone(state: &mut AppState, params: Value) -> (Value, Value) {
    let listed = state.handle(req("tasks.list", params));
    assert_eq!(listed["ok"], true, "{listed:?}");
    let rows = listed["result"]["tasks"].clone();
    let alone = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let got = state.handle(req("tasks.get", json!({ "task_id": row["id"] })));
            got["result"]["task"].clone()
        })
        .collect();
    (rows, Value::Array(alone))
}

#[test]
fn a_long_list_answers_what_each_task_answers_alone() {
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
    assert_eq!(listed_ids, newest_first, "newest first, every task once");
    assert_eq!(rows, alone, "each row is what reading the task alone says");
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
            .filter(|task| !task.identities.is_empty())
            .count()
            >= TASKS / 2,
        "the list fills identities into the records it read"
    );
    assert_eq!(
        stored_records(&state, &ids),
        filled,
        "reading each task alone found nothing the list had not already filled"
    );

    let (again, _) = listed_and_each_alone(&mut state, json!({ "project_id": project_id }));
    assert_eq!(again, rows, "a second list answers the same");

    let (labelled, labelled_alone) = listed_and_each_alone(
        &mut state,
        json!({ "project_id": project_id, "label": "sweep" }),
    );
    assert_eq!(labelled.as_array().unwrap().len(), TASKS / 5);
    assert_eq!(labelled, labelled_alone);
}

/// Naming an agent no live roster holds reads the stored runs and plans. The
/// list read them once per call (#128), but then cloned every stored run's
/// roster — each agent's conversation tail — for every such agent it named:
/// 450 ms held per `tasks.list` on a real store (#131). An agent is found in
/// the stored records without copying a conversation. Count those reads so
/// host load cannot change whether this regression is caught.
#[test]
fn naming_many_departed_agents_copies_no_conversation() {
    const DEPARTED: usize = 8;
    const GHOSTS: usize = 1200;
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let departed: Vec<String> = (0..DEPARTED)
        .map(|n| departed_with_a_long_conversation(&mut state, &project_id, n))
        .collect();
    for batch in 0..GHOSTS / 8 {
        let id = filed(&mut state, &project_id, &format!("ghosts {batch}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let store = state.tracker_store().unwrap();
        let mut task = store.load_tracker_task(&id).unwrap().unwrap();
        task.body = (0..8)
            .map(|n| format!("@agent:agent-ghost-{} ", batch * 8 + n))
            .collect::<String>()
            + &format!("and @agent:{}", departed[batch % DEPARTED]);
        task.identities.clear();
        store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    }

    let store = state.tracker_store().unwrap();
    let roster_reads =
        store.load_all_run_rosters().unwrap().len() + store.load_all_plan_rosters().unwrap().len();
    assert_eq!(roster_reads, DEPARTED, "the fixture has eight stored runs");
    let before = crate::store::Store::agent_read_counts();
    let listed = state.handle(req("tasks.list", json!({ "project_id": project_id })));
    let after = crate::store::Store::agent_read_counts();

    assert_eq!(listed["ok"], true, "{listed:?}");
    let rows = listed["result"]["tasks"].as_array().unwrap();
    assert_eq!(rows.len(), GHOSTS / 8);
    assert!(rows.iter().enumerate().all(|(index, row)| {
        let batch = GHOSTS / 8 - 1 - index;
        row["identities"][&departed[batch % DEPARTED]]["name"] == "Historian"
    }));
    assert_eq!(
        after.0 - before.0,
        roster_reads,
        "tasks.list reads each stored roster once"
    );
    assert_eq!(
        after.1 - before.1,
        0,
        "tasks.list does not load departed agents' conversations"
    );

    state.tracker_store().unwrap().load_all_runs().unwrap();
    let control = crate::store::Store::agent_read_counts();
    assert_eq!(
        control.1 - after.1,
        DEPARTED,
        "the counter detects a full conversation read for each stored run"
    );
}

/// An agent named "Historian" whose run has left the live roster, with a
/// conversation tail as long as the store keeps.
fn departed_with_a_long_conversation(state: &mut AppState, project_id: &str, n: usize) -> String {
    let (entity_id, agent_id) = agent_on_checkout(state, project_id, &format!("history-{n}"));
    state
        .set_agent_name(&entity_id, &agent_id, "Historian")
        .unwrap();
    let mut active = state.runs.remove(&entity_id).unwrap();
    let thread = &mut active.agents.by_id_mut(&agent_id).unwrap().thread;
    for item in 0..250 {
        thread.push_event(
            crate::thread::ThreadEventKind::Narration,
            Some(format!(
                "narration {item}: {}",
                "a long-winded agent. ".repeat(8)
            )),
            None,
            None,
            now_rfc3339(),
        );
    }
    state.persist_run_record(&entity_id, &active).unwrap();
    agent_id
}
