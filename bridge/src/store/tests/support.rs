use super::*;
use crate::agent::AgentRoster;
use crate::models::ModelChoice;
use crate::operation::{
    DeliveryIntent, OperationPayload, OperationReceipt, OperationStatus, THREAD_POST_METHOD,
};

pub(super) const NOW: &str = "2026-08-21T10:00:00Z";

pub(super) fn queued_operation(operation_id: &str, posted_sequence: u64) -> OperationReceipt {
    let agent_id = crate::agent::derived_agent_id("issue-1");
    OperationReceipt {
        operation_id: operation_id.to_string(),
        method: THREAD_POST_METHOD.to_string(),
        entity_id: "issue-1".to_string(),
        agent_id: agent_id.clone(),
        conversation_id: agent_id.clone(),
        choice_revision: 0,
        posted_sequence,
        message_start_sequence: posted_sequence,
        status: OperationStatus::Queued,
        execution_error: None,
        request_hash: "same-request".to_string(),
        delivery: Some(DeliveryIntent {
            root: "/repo".into(),
            owner_id: "issue-1".to_string(),
            agent_id,
            model_choice: ModelChoice::default(),
            choice_revision: 0,
            interrupt: false,
            payload: Some(OperationPayload {
                start_sequence: posted_sequence,
                end_sequence: posted_sequence,
                messages: Vec::new(),
                prior_context: String::new(),
            }),
        }),
        requested_by: None,
    }
}

pub(super) fn run_record(id: &str, plan_id: Option<&str>, created_at: &str) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: plan_id.map(str::to_string),
        goal: format!("goal for {id}"),
        project_path: "/repo".into(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: format!("build/{id}"),
        worktree_name: id.into(),
        worktree_path: format!("/wt/{id}"),
        base_sha: None,
        stages: Vec::new(),
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        publication_attempt: None,
        provider: Default::default(),
        model: None,
        effort: None,
        agents: AgentRoster::with_first(id, ModelChoice::default(), created_at)
            .agents()
            .to_vec(),
        legacy_thread: Default::default(),
        last_summary: None,
        last_error: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        state_changed_at: None,
    }
}

pub(super) fn plan_record(id: &str) -> PersistedPlan {
    PersistedPlan {
        id: id.into(),
        goal: format!("goal for {id}"),
        project_path: "/repo".into(),
        base_branch: "main".into(),
        state: PlanState::Drafting,
        archived_at: None,
        implementation_intent: Default::default(),
        implementation_activity: Default::default(),
        plan_path: ".build/plan.md".into(),
        stages: Vec::new(),
        provider: Default::default(),
        model: None,
        effort: None,
        agents: AgentRoster::with_first(id, ModelChoice::default(), NOW)
            .agents()
            .to_vec(),
        legacy_thread: Default::default(),
        last_summary: None,
        last_error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        state_changed_at: None,
    }
}

pub(super) fn reload_run(store: &Store, run_id: &str) -> PersistedRun {
    store
        .load_all_runs()
        .expect("runs load")
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap_or_else(|| panic!("{run_id} is missing after a reload"))
}

/// A conversation of `count` messages, saved, with the id of the agent
/// holding it — the shape every paging test starts from.
pub(super) fn store_with_conversation(root: &Path, count: usize) -> (Store, String) {
    let store = Store::new(root).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for n in 0..count {
        record.agents[0]
            .thread
            .post_user(format!("message {n}"), None, NOW);
    }
    store.save_run(&record).expect("the conversation saves");
    (store, record.agents[0].id.clone())
}

pub(super) fn sequences(items: &[ThreadItem]) -> Vec<u64> {
    items.iter().map(ThreadItem::sequence).collect()
}

/// How SQLite says it will answer a statement. The plan does not depend on
/// what the parameters hold, only on how many there are, so every pinning
/// test below binds ones.
pub(super) fn query_plan(connection: &Connection, statement: &str) -> Vec<String> {
    let mut explain = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
        .expect("the statement prepares");
    let placeholders = vec![1_i64; explain.parameter_count()];
    let plan = explain
        .query_map(rusqlite::params_from_iter(placeholders), |row| {
            row.get::<_, String>(3)
        })
        .expect("the plan reads")
        .collect::<Result<_, _>>()
        .expect("the plan reads");
    plan
}

/// The sequences the store believes are activity — read off the column
/// rather than off the items, which is the whole point of hoisting it.
pub(super) fn stored_activity_sequences(store: &Store, agent_id: &str) -> Vec<u64> {
    let connection = store.connection();
    let mut statement = connection
        .prepare(
            "SELECT sequence FROM thread_items \
                 WHERE agent_id = ?1 AND activity = 1 ORDER BY sequence",
        )
        .expect("the predicate prepares");
    let read = statement
        .query_map([agent_id], |row| row.get::<_, i64>(0))
        .expect("the predicate reads")
        .map(|sequence| sequence.expect("a row reads") as u64)
        .collect();
    read
}
