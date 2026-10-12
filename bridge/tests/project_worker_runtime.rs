//! A project's workers at work (#533), end to end.
//!
//! The whole daemon over temporary fixtures (see `project_bridge`): a task
//! handed to a new project worker, retried; the worker starting in the
//! project's sources and holding them in place while it runs; its removal;
//! a restart with the project's folder away and back; and the project's
//! deletion, which ends its agents and keeps its sources.
//!
//! One test, because it sets this binary's PATH and HOME.

mod project_bridge;

use project_bridge::{ids, init_repo, private_path, scratch_of, Bridge};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The project under test: a git home and a plain folder beside it.
struct Project {
    root: PathBuf,
    repo: PathBuf,
    project_id: String,
    owner: String,
    pin: String,
}

fn a_project_with_two_sources(bridge: &Bridge, root: &Path, repo: &Path) -> Project {
    let notes = root.join("notes");
    std::fs::create_dir(&notes).unwrap();
    let project_id = bridge.ok(
        "project.create",
        json!({ "name": "repo", "sources": [{ "path": repo }, { "path": notes }] }),
    )["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner = bridge.ensure(&project_id);
    let pin = bridge.roster(&owner, true)["pinned_agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    Project {
        root: root.to_path_buf(),
        repo: repo.to_path_buf(),
        project_id,
        owner,
        pin,
    }
}

/// A task handed to a new project worker, and the same request again: one
/// worker, one delivery, the same answer. The worker starts in Build's
/// scratch for its scaffold and nowhere in the sources.
fn a_task_reaches_one_new_worker(bridge: &Bridge, project: &Project) -> String {
    let task_id = bridge.ok(
        "tasks.create",
        json!({ "project_id": project.project_id, "title": "Tidy the README" }),
    )["task"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let request = json!({
        "task_id": task_id,
        "assignee": { "kind": "new_project_agent", "agent_name": "Docs pass" },
        "operation_id": "op-docs-pass",
    });
    let first = bridge.ok("tasks.assign", request.clone());
    let again = bridge.ok("tasks.assign", request);
    assert_eq!(again["dispatch"], first["dispatch"], "a retry is the first");
    let worker = first["dispatch"]["agent_id"].as_str().unwrap().to_string();
    assert_eq!(first["dispatch"]["entity_id"], json!(project.owner));
    assert_eq!(first["dispatch"]["workspace_id"], Value::Null);
    let roster = bridge.roster(&project.owner, true);
    assert_eq!(ids(&roster), [project.pin.clone(), worker.clone()]);
    assert_eq!(roster["agents"][1]["project_role"], "worker");
    bridge.wait_for_state(&project.owner, &worker, "live");
    let page = bridge.ok(
        "thread.page",
        json!({ "entity_id": project.owner, "agent_id": worker, "limit": 50 }),
    );
    let handed: Vec<&Value> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["data"]["operation_id"] == "op-docs-pass")
        .collect();
    assert_eq!(handed.len(), 1, "delivered once: {page}");
    assert!(scratch_of(&project.root)
        .join(format!(".build/mcp-{worker}.json"))
        .is_file());
    assert!(!project.repo.join(".build").exists());
    worker
}

/// While the worker runs, no source moves or leaves; once it is removed,
/// one can.
fn a_running_worker_holds_the_sources(bridge: &Bridge, project: &Project, worker: &str) {
    let remove = json!({ "project_id": project.project_id, "source_id": "source-2" });
    let refusal = bridge.refused("project.remove_source", remove.clone(), "conflict");
    assert!(
        refusal["error"].as_str().unwrap().contains("Docs pass"),
        "{refusal}"
    );
    bridge.ok(
        "agent.remove",
        json!({ "entity_id": project.owner, "agent_id": worker }),
    );
    let removed = bridge.ok("project.remove_source", remove);
    assert_eq!(removed["sources"].as_array().unwrap().len(), 1);
}

/// A restart with the project's folder away keeps the project; the folder
/// back, the list finds it under its own id with the same owner and pin.
fn a_folder_away_at_boot_comes_back(project: &Project) {
    let away = project.repo.with_extension("away");
    std::fs::rename(&project.repo, &away).unwrap();
    let bridge = Bridge::boot(&project.root);
    let listed = bridge.ok("project.list", json!({}));
    assert_eq!(listed["projects"], json!([]), "{listed}");
    bridge.ok("settings.set", json!({ "workspace_idle_secs": 86400 }));
    std::fs::rename(&away, &project.repo).unwrap();
    let listed = bridge.ok("project.list", json!({}));
    assert_eq!(
        listed["projects"][0]["project_id"],
        json!(project.project_id)
    );
    assert_eq!(bridge.ensure(&project.project_id), project.owner);
    let roster = bridge.roster(&project.owner, true);
    assert_eq!(roster["pinned_agent_id"], json!(project.pin));
    bridge.shutdown();
}

/// Deleting the project ends its agents and removes what Build kept for it,
/// and leaves the sources — uncommitted work included — where they are.
fn deleting_the_project_keeps_its_sources(bridge: &Bridge, project: &Project) {
    let worker = bridge.ok(
        "agent.add",
        json!({ "entity_id": project.owner, "name": "Last one" }),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    bridge.ok(
        "thread.post",
        json!({ "entity_id": project.owner, "agent_id": worker, "body": "anything?" }),
    );
    bridge.wait_for_state(&project.owner, &worker, "live");
    let edit = project.repo.join("uncommitted.txt");
    std::fs::write(&edit, "mine\n").unwrap();
    let scratch = scratch_of(&project.root);
    bridge.ok(
        "project.delete",
        json!({ "project_id": project.project_id, "confirm": true }),
    );
    assert_eq!(bridge.ok("project.list", json!({}))["projects"], json!([]));
    assert!(!scratch.exists(), "Build's scratch for the project is gone");
    assert!(project.repo.join(".git").is_dir());
    assert_eq!(std::fs::read_to_string(&edit).unwrap(), "mine\n");
}

#[test]
fn project_workers_take_tasks_hold_sources_and_leave_them_on_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("PATH", private_path(&root));
    std::env::set_var("HOME", &home);
    let repo = init_repo(&root);

    let bridge = Bridge::boot(&root);
    let project = a_project_with_two_sources(&bridge, &root, &repo);
    let worker = a_task_reaches_one_new_worker(&bridge, &project);
    a_running_worker_holds_the_sources(&bridge, &project, &worker);
    bridge.shutdown();

    a_folder_away_at_boot_comes_back(&project);

    let bridge = Bridge::boot(&root);
    deleting_the_project_keeps_its_sources(&bridge, &project);
    bridge.shutdown();
}
