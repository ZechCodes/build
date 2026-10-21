use super::*;
use crate::app::tests::tracker::{filed, tracked};
use crate::app::tests::*;
use crate::store::TaskFilter;
use crate::timing::write_wait::WriteWaitRegistry;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_read_seed_returns_saved_wire_values_without_adding_tasks_or_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "Reserved qualification read witness")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dependent = filed(&mut app, &project, "Reserved qualification dependent")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace =
        crate::app::tests::project_agent::workspace(&mut app, &project, "qualification-read-seed");
    let selected = app.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace}),
    ));
    assert_eq!(selected["ok"], true, "{selected}");
    let owner = selected["result"]["entity_id"].as_str().unwrap().to_owned();
    let added = app.handle(req("agent.add", json!({"entity_id":owner})));
    assert_eq!(added["ok"], true, "{added}");
    let agent = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
    let project_path = app.tracker_project_path(&project).unwrap();
    let before_tasks = app
        .require_store()
        .unwrap()
        .list_tracker_tasks(&project_path, TaskFilter::default())
        .unwrap()
        .len();
    let before_workspaces = app.workspaces.list(None).len();
    let inputs = SeededReadInputs::seed(
        &mut app,
        SeedContext {
            project: &project,
            workspace: &workspace,
            owner: &owner,
            agent: &agent,
            task: &task,
            dependent: &dependent,
        },
    );
    assert_eq!(
        app.require_store()
            .unwrap()
            .list_tracker_tasks(&project_path, TaskFilter::default())
            .unwrap()
            .len(),
        before_tasks
    );
    assert_eq!(app.workspaces.list(None).len(), before_workspaces);
    assert_eq!(inputs.ledger.added_tracker_tasks, 0);
    assert_eq!(inputs.ledger.added_registry_workspaces, 0);
    assert_eq!(inputs.ledger.legacy_plans, 2);
    assert_eq!(inputs.ledger.project_owners, 1);
    assert_eq!(inputs.ledger.passive_routers, 1);
    // The common history filler must include this measured native delta.
    assert!(inputs.ledger.added_thread_items < 1000);
    assert_eq!(inputs.core.project, project);
    assert_eq!(inputs.core.workspace, workspace);
    assert_eq!(inputs.core.owner, owner);
    assert_eq!(inputs.core.agent, agent);
    assert_eq!(inputs.core.task, task);
    assert_eq!(inputs.core.dependent, dependent);
    assert_eq!(
        app.project_pin(&inputs.project_owner),
        Some(inputs.project_agent.as_str())
    );
    assert!(app
        .session_registry
        .token_matches(&inputs.router, &inputs.router_token));
    assert_eq!(
        app.require_store()
            .unwrap()
            .load_all_captures()
            .unwrap()
            .iter()
            .filter(|c| c.id == inputs.capture)
            .count(),
        1
    );
    app.initialize_read_context();
    app.initialize_snapshot_projection();
    // The passive fixture does not run an unrelated routing/delivery worker.
    let state = Arc::new(Mutex::new(app));
    let rpc = super::super::rpc::RpcHarness::new(state, WriteWaitRegistry::new());
    let params = vec![
        ("tasks.get", json!({"task_id":task})),
        (
            "tasks.waiting_on.blocks",
            json!({"task_id":task,"limit":20}),
        ),
        ("tasks.review.get", json!({"task_id":task})),
        (
            "tasks.review.diff",
            json!({"task_id":task,"snapshot_id":inputs.core.snapshot,"directory_id":inputs.core.directory,"mode":"blob","path":"README.md","range":{"offset":0,"bytes":4096}}),
        ),
        (
            "tasks.attachment",
            json!({"task_id":task,"path":inputs.core.task_attachment,"offset":0,"length":4096}),
        ),
        (
            "thread.attachment",
            json!({"entity_id":owner,"agent_id":agent,"path":inputs.core.thread_attachment,"offset":0,"length":4096}),
        ),
        ("task.doc", json!({"task_id":inputs.ordinary_plan})),
        (
            "task.stage_doc",
            json!({"task_id":inputs.staged_plan,"stage_id":inputs.stage}),
        ),
        ("capture.get", json!({"capture_id":inputs.capture})),
    ];
    let mut replies = Vec::new();
    for (index, (method, params)) in params.into_iter().enumerate() {
        let id = format!("qual/read-seed/{index}");
        replies.push(rpc.request(index % 4, &id, method, params).await);
    }
    rpc.finish().await;
    for proof in &replies {
        assert_eq!(proof.reply["id"], proof.original_id);
        assert_eq!(proof.reply["ok"], true, "{}: {}", proof.method, proof.reply);
        assert!(
            !proof.reply["result"].is_null(),
            "{} returned a placeholder",
            proof.method
        );
        assert_ne!(proof.reply["result"]["pending"], true);
    }
    let task_value = &replies[0].reply["result"];
    assert_eq!(task_value["task"]["id"], task);
    assert!(task_value["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == inputs.core.comment));
    assert!(replies[1].reply["result"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == dependent));
    assert!(replies[2].reply["result"]["review"]["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == inputs.core.snapshot));
    for (index, expected) in [
        (3, &inputs.core.blob_bytes),
        (4, &inputs.core.attachment_bytes),
        (5, &inputs.core.attachment_bytes),
    ] {
        assert_eq!(
            &b64decode(
                replies[index].reply["result"]["content_b64"]
                    .as_str()
                    .unwrap()
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        replies[6].reply["result"]["contents"],
        "# Ordinary qualification document\n"
    );
    assert_eq!(
        replies[7].reply["result"]["contents"],
        "# Staged qualification document\n"
    );
    assert_eq!(replies[8].reply["result"]["id"], inputs.capture);
    assert_eq!(
        replies[8].reply["result"]["text"],
        "Qualification immutable router read witness"
    );
}
