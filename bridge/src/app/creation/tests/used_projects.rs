use super::*;
use crate::harness::{AgentSession, AgentStatus, HarnessError, Turn};
use std::time::Duration;

fn failed_project(f: &Fixture, id: &str) -> Value {
    let record = f.admit(id);
    f.store.writer().claim_mutation(PRINCIPAL, id).unwrap();
    project_step(&f.state, &record).unwrap();
    f.store.writer().finish_creation(PRINCIPAL,id,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace refused","retryable":false}})).unwrap();
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspaces
        .list(Some("proj-2"))
        .is_empty());
    json!({"project_id":"proj-2","creation_operation_id":id})
}

fn assert_refused(f: &Fixture, params: &Value, expected: &str) {
    let mut app = f.state.lock().unwrap();
    let before = std::fs::read(f._root.path().join("config.json")).unwrap();
    assert_eq!(app.project_discard_unused(params).unwrap_err(), expected);
    assert!(app.projects.get("proj-2").is_some());
    assert_eq!(
        std::fs::read(f._root.path().join("config.json")).unwrap(),
        before
    );
}

#[test]
fn discard_unused_refused_with_tasks() {
    let f = Fixture::new();
    let params = failed_project(&f, &operation(200));
    let key = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-2")
        .unwrap()
        .project_key
        .clone();
    let task = f
        .store
        .create_tracker_task(
            crate::tracker::Task::drafted(
                &key,
                "must survive discard",
                crate::tracker::Actor::User,
                &crate::store::now_rfc3339(),
            ),
            &[],
        )
        .unwrap();
    assert_refused(&f, &params, "project has a task");
    assert!(f.store.load_tracker_task(&task.id).unwrap().is_some());
}

struct IdleSession;
impl AgentSession for IdleSession {
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        false
    }
    fn backdate_last_output(&self, _: Duration) {}
    fn end(&self) {}
}

#[test]
fn discard_unused_refused_with_agent_sessions() {
    let f = Fixture::new();
    let params = failed_project(&f, &operation(201));
    let mut app = f.state.lock().unwrap();
    let root = app.projects.get("proj-2").unwrap().home_path.clone();
    let key = crate::app::TabKey::agent(&root, "agent-discard");
    let tab = crate::app::Tab {
        tab_id: "agent:agent-discard".into(),
        root,
        role: crate::app::TabRole::Agent {
            owner: "proj-2".into(),
            agent_id: "agent-discard".into(),
            provider: Default::default(),
        },
        project_source: None,
        created_at: crate::store::now_rfc3339(),
        session: Arc::new(IdleSession),
        session_instance: None,
        screen: None,
        live: true,
        call_sequences: Default::default(),
        last_delivered_at: None,
    };
    app.session_registry.test_insert_tab(key.clone(), tab);
    drop(app);
    assert_refused(&f, &params, "project has an agent session");
    assert!(f.state.lock().unwrap().session_registry.contains(&key));
}

#[test]
fn discard_unused_refused_with_retained_roster() {
    let f = Fixture::new();
    let params = failed_project(&f, &operation(202));
    let key = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-2")
        .unwrap()
        .project_key
        .clone();
    let record: crate::store::PersistedPlan = serde_json::from_value(json!({
        "id":"plan-retained", "goal":"keep this roster", "project_path":key,
        "base_branch":"main", "state":crate::plan::PlanState::Interrupted, "plan_path":".build/plan.md",
        "agents":crate::agent::stored_agents("plan-retained"),
        "created_at":crate::store::now_rfc3339(), "updated_at":crate::store::now_rfc3339()
    })).unwrap();
    f.store.save_task_plan(&record).unwrap();
    // The roster exists only on disk, so no in-memory run/plan refusal masks it.
    assert!(f.state.lock().unwrap().plans.is_empty());
    assert_refused(&f, &params, "project has a retained agent session");
    assert_eq!(f.store.load_all_plan_rosters().unwrap()[0].id, record.id);
}

