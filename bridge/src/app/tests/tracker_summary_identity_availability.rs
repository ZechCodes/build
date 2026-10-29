//! Availability is a live observation even when a retained identity was saved
//! while its creator or assignee could still be opened.
use super::*;
use crate::tracker::{Actor, Assignee, Task, TaskAgentIdentity};

struct Fixture {
    app: AppState,
    project: String,
    owner: String,
    agents: [String; 2],
    task: Task,
}

fn retained_identity(agent: &str, name: &str) -> TaskAgentIdentity {
    TaskAgentIdentity {
        agent_id: agent.into(),
        name: Some(name.into()),
        project_role: None,
        ordinal: Some(7),
        workspace_id: Some("retained-workspace".into()),
        workspace_name: Some("Retained checkout".into()),
        provider: Some("pi".into()),
        available: true,
    }
}

fn create_task(app: &AppState, project: &str, agents: &[String; 2]) -> Task {
    let path = app.tracker_project_path(project).unwrap();
    let mut task = Task::drafted(
        &path,
        "Retained creator and assignee",
        Actor::Agent {
            agent_id: agents[0].clone(),
        },
        "2026-10-27T00:00:00Z",
    );
    task.assignee = Some(Assignee::Agent {
        agent_id: agents[1].clone(),
    });
    for (agent, name) in agents.iter().zip(["Retained creator", "Retained assignee"]) {
        task.identities
            .insert(agent.clone(), retained_identity(agent, name));
    }
    app.tracker_store()
        .unwrap()
        .create_tracker_task(task, &[])
        .unwrap()
}

fn live_fixture(root: &Path) -> Fixture {
    let (mut app, project) = super::tracker::tracked_with_origin(root);
    let workspace = super::project_agent::workspace(&mut app, &project, "Identity parity");
    let ensured = app.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace}),
    ));
    assert_eq!(ensured["ok"], true, "{ensured}");
    let owner = ensured["result"]["entity_id"].as_str().unwrap().to_string();
    let agents = ["Current creator", "Current assignee"].map(|name| {
        let added = app
            .agent_add(&json!({"entity_id":owner,"provider":"pi","notify_user":false}))
            .unwrap();
        let id = added["agent"]["id"].as_str().unwrap().to_string();
        app.set_agent_name(&owner, &id, name).unwrap();
        id
    });
    let mut task = create_task(&app, &project, &agents);
    for identity in task.identities.values_mut() {
        identity.workspace_id = Some(workspace.clone());
        identity.workspace_name = Some("Identity parity".into());
    }
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&task, &[], &[])
        .unwrap();
    Fixture {
        app,
        project,
        owner,
        agents,
        task,
    }
}

async fn assert_read_parity(app: &AppState, project: &str, task: &Task, available: bool) -> Value {
    let context = app.task_read_context();
    let mut connection = app.tracker_store().unwrap().read_pool().acquire().await;
    let (detail, legacy, summary) = connection
        .read(|tx| {
            Ok::<_, String>((
                context.tasks_get(tx, &json!({"task_id":task.id}))?,
                context.tasks_list(tx, &json!({"project_id":project}))?,
                context.tasks_list_summary(tx, &json!({"project_id":project,"summary":true}))?,
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        summary["tasks"][0]["created_by"],
        detail["task"]["created_by"]
    );
    assert_eq!(summary["tasks"][0]["assignee"], detail["task"]["assignee"]);
    for agent in task.identities.keys() {
        let expected = &detail["task"]["identities"][agent];
        assert_eq!(expected["available"], available, "legacy detail: {detail}");
        assert_eq!(legacy["tasks"][0]["identities"][agent], *expected);
        assert_eq!(
            summary["identities"][agent], *expected,
            "summary: {summary}"
        );
    }
    summary
}

fn assert_retained_descriptions(summary: &Value, task: &Task) {
    for (agent, retained) in &task.identities {
        let mut expected = retained.clone();
        expected.available = false;
        assert_eq!(
            summary["identities"][agent],
            serde_json::to_value(expected).unwrap()
        );
    }
}

#[tokio::test]
async fn summary_retained_creator_and_assignee_without_rosters_are_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = super::tracker::tracked(root.path());
    let agents = [
        "agent-retained-creator".into(),
        "agent-retained-assignee".into(),
    ];
    let task = create_task(&app, &project, &agents);
    let summary = assert_read_parity(&app, &project, &task, false).await;
    assert_retained_descriptions(&summary, &task);
}

#[tokio::test]
async fn summary_committed_roster_without_a_live_owner_is_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = live_fixture(root.path());
    fixture.app.runs.remove(&fixture.owner).unwrap();
    let summary = assert_read_parity(&fixture.app, &fixture.project, &fixture.task, false).await;
    for (agent, name) in fixture
        .agents
        .iter()
        .zip(["Current creator", "Current assignee"])
    {
        assert_eq!(summary["identities"][agent]["name"], name);
        assert_eq!(summary["identities"][agent]["provider"], "pi");
    }
}

#[tokio::test]
async fn summary_deleted_roster_keeps_descriptions_but_clears_availability() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = live_fixture(root.path());
    fixture
        .app
        .tracker_store()
        .unwrap()
        .delete_run(&fixture.owner)
        .unwrap();
    fixture.app.runs.remove(&fixture.owner).unwrap();
    let summary = assert_read_parity(&fixture.app, &fixture.project, &fixture.task, false).await;
    assert_retained_descriptions(&summary, &fixture.task);
}

#[tokio::test]
async fn summary_live_creator_and_assignee_follow_available_and_retired_bindings() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = live_fixture(root.path());
    let summary = assert_read_parity(&fixture.app, &fixture.project, &fixture.task, true).await;
    for (agent, name) in fixture
        .agents
        .iter()
        .zip(["Current creator", "Current assignee"])
    {
        assert_eq!(summary["identities"][agent]["name"], name);
        assert_eq!(summary["identities"][agent]["provider"], "pi");
    }
    for agent in &fixture.agents {
        fixture
            .app
            .session_registry
            .retire_agent_binding(&fixture.owner, agent);
    }
    assert_read_parity(&fixture.app, &fixture.project, &fixture.task, false).await;
}

#[tokio::test]
async fn summary_committed_roster_association_matches_legacy_without_live_owner() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = live_fixture(root.path());
    // A retained association may name an earlier checkout. This must remain a
    // separate parity regression rather than hiding behind availability.
    for identity in fixture.task.identities.values_mut() {
        identity.workspace_id = Some("retained-workspace".into());
        identity.workspace_name = Some("Retained checkout".into());
    }
    fixture
        .app
        .tracker_store()
        .unwrap()
        .save_tracker_task_activity(&fixture.task, &[], &[])
        .unwrap();
    fixture.app.runs.remove(&fixture.owner).unwrap();
    assert_read_parity(&fixture.app, &fixture.project, &fixture.task, false).await;
}
