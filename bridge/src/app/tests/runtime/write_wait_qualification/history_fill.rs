//! Native deterministic history fill within the single declared total.
use crate::app::AppState;
use std::collections::{BTreeMap, BTreeSet};

fn histories(app: &AppState) -> BTreeMap<String, u64> {
    let mut result = BTreeMap::new();
    for run in app.runs.values() {
        for agent in run.agents.iter() {
            let id = agent.conversation_id().to_string();
            let count = agent.thread.total_item_count();
            if let Some(old) = result.insert(id, count) {
                assert_eq!(old, count, "canonical aliases disagree");
            }
        }
    }
    for plan in app.plans.values() {
        for agent in plan.agents.iter() {
            let id = agent.conversation_id().to_string();
            let count = agent.thread.total_item_count();
            if let Some(old) = result.insert(id, count) {
                assert_eq!(old, count, "canonical aliases disagree");
            }
        }
    }
    result
}

fn append(thread: &mut crate::thread::Thread, count: usize, canonical: &str) {
    for index in 0..count {
        let body = format!(
            "Qualification retained history {canonical}/{index:05}: inspect saved work, retain the exact task and conversation identity, and report committed results through the bounded native read paths."
        );
        if index % 2 == 0 {
            thread.post_user(body, None, "2026-10-21T12:00:00Z");
        } else {
            thread.post_agent(body, None, "2026-10-21T12:00:01Z");
        }
    }
    // Existing history is already consumed by the pinned warm fake sessions.
    // This marks historical inbox messages Seen and prevents seed delivery debt.
    thread.read_unread("2026-10-21T12:00:02Z");
    assert!(!thread.has_unread());
}

pub(super) fn fill(app: &mut AppState, target: usize) -> BTreeMap<String, u64> {
    let before = histories(app);
    let initial = before.values().sum::<u64>() as usize;
    assert!(
        initial <= target,
        "read/session seed exceeds the pinned total; no clipping"
    );
    assert!(!before.is_empty());
    let remaining = target - initial;
    let additions = before
        .keys()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                remaining / before.len() + usize::from(index < remaining % before.len()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();
    let run_ids = app.runs.keys().cloned().collect::<Vec<_>>();
    for id in run_ids {
        let mut active = app.runs.remove(&id).unwrap();
        for agent in active.agents.iter_mut() {
            let canonical = agent.conversation_id().to_string();
            if visited.insert(canonical.clone()) {
                append(&mut agent.thread, additions[&canonical], &canonical);
            }
        }
        app.persist_run_record(&id, &active).unwrap();
        app.runs.insert(id, active);
    }
    let plan_ids = app.plans.keys().cloned().collect::<Vec<_>>();
    for id in plan_ids {
        let mut active = app.plans.remove(&id).unwrap();
        for agent in active.agents.iter_mut() {
            let canonical = agent.conversation_id().to_string();
            if visited.insert(canonical.clone()) {
                append(&mut agent.thread, additions[&canonical], &canonical);
            }
        }
        app.persist_plan_record(&id, &active).unwrap();
        app.plans.insert(id, active);
    }
    assert_eq!(visited.len(), additions.len());
    let after = histories(app);
    assert_eq!(after.values().sum::<u64>(), target as u64);
    after
}
