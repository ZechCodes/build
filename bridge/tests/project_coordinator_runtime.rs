//! A project's pinned coordinator and its workers (#532), end to end.
//!
//! The whole daemon over temporary fixtures: real frames through the real
//! [`FrameHandler`], a real task store and config under a temporary directory,
//! a real git project, real deliveries starting real agent sessions, and
//! restarts that boot a new bridge over the same store. Nothing is stubbed but
//! the harness, which is the daemon's own QA agent — a fake child that drains
//! its prompts — so no provider CLI is ever looked for: the test's PATH holds
//! only the few tools the bridge and its fixtures run.
//!
//! One test, because it sets this binary's PATH and HOME.

mod project_bridge;

use build_bridge::store::Store;
use project_bridge::{ids, init_repo, private_path, purposes, scratch_of, Bridge};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Rewrite the stored owner as a bridge from before pins wrote it: no pin,
/// and no purpose on any agent.
fn store_as_legacy(root: &Path, owner: &str) {
    let store = Store::new(root.join("store")).unwrap();
    let mut record = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == owner)
        .unwrap();
    record.pinned_agent_id = None;
    for agent in &mut record.agents {
        agent.project_role = None;
    }
    store.save_run(&record).unwrap();
}

/// The project under test and the agents each phase leaves on it.
struct Project {
    root: PathBuf,
    repo: PathBuf,
    project_id: String,
    owner: String,
    pin: String,
}

impl Project {
    fn agent_list(&self, bridge: &Bridge, include_project_workers: bool) -> Vec<String> {
        ids(&bridge.roster(&self.owner, include_project_workers))
    }

    fn only_the_pin(&self) -> Vec<String> {
        vec![self.pin.clone()]
    }
}

/// A fresh project is born with exactly one pin, and ensuring again keeps it.
fn a_fresh_project(bridge: &Bridge, root: &Path, repo: &Path) -> Project {
    let project_id = bridge.ok(
        "project.create",
        json!({ "name": "repo", "sources": [{ "path": repo }] }),
    )["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner = bridge.ensure(&project_id);
    assert_eq!(bridge.ensure(&project_id), owner, "ensure is idempotent");
    let fresh = bridge.roster(&owner, true);
    assert_eq!(purposes(&fresh), ["coordinator"], "{fresh}");
    let pin = fresh["pinned_agent_id"].as_str().unwrap().to_string();
    let project = Project {
        root: root.to_path_buf(),
        repo: repo.to_path_buf(),
        project_id,
        owner,
        pin,
    };
    assert_eq!(ids(&fresh), project.only_the_pin());
    project
}

/// Workers are added below the pin; nobody can mint a coordinator.
fn workers_below_the_pin(bridge: &Bridge, project: &Project) -> (String, String) {
    let owner = &project.owner;
    let added = |params: Value| {
        bridge.ok("agent.add", params)["agent"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let worker = added(json!({ "entity_id": owner, "name": "Docs pass" }));
    let second = added(json!({ "entity_id": owner, "project_role": "worker" }));
    bridge.refused(
        "agent.add",
        json!({ "entity_id": owner, "project_role": "coordinator" }),
        "invalid_params",
    );
    let whole = bridge.roster(owner, true);
    assert_eq!(
        ids(&whole),
        [project.pin.clone(), worker.clone(), second.clone()]
    );
    assert_eq!(purposes(&whole), ["coordinator", "worker", "worker"]);
    (worker, second)
}

/// A client that predates workers still sees one project agent.
fn old_clients_see_the_pin_alone(bridge: &Bridge, project: &Project) {
    assert_eq!(project.agent_list(bridge, false), project.only_the_pin());
    let board = bridge.ok("board.list", json!({}));
    let row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == json!(project.owner))
        .unwrap()
        .clone();
    assert_eq!(ids(&row), project.only_the_pin(), "{row}");
}

/// Each agent gets its own child, scaffold and MCP config, all in Build's
/// scratch rather than the sources.
fn each_agent_its_own_session(bridge: &Bridge, project: &Project, worker: &str) {
    let owner = &project.owner;
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": worker, "body": "tidy the README" }),
    );
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "body": "what is in flight?" }),
    );
    bridge.wait_for_state(owner, worker, "live");
    bridge.wait_for_state(owner, &project.pin, "live");
    let scratch = scratch_of(&project.root);
    for agent in [project.pin.as_str(), worker] {
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(scratch.join(format!(".build/mcp-{agent}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            config["mcpServers"]["build"]["args"],
            json!(["mcp", "--task", agent]),
            "{agent}"
        );
    }
    assert!(
        !project.repo.join(".build").exists(),
        "no agent scaffold in the sources"
    );
}

/// The pin cannot be taken away; a worker can, and only it goes.
fn the_pin_stays_and_a_worker_goes(bridge: &Bridge, project: &Project, worker: &str) {
    let owner = &project.owner;
    let refusal = bridge.refused(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": project.pin }),
        "conflict",
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("Delete the project"),
        "{refusal}"
    );
    bridge.refused("run.abandon", json!({ "run_id": owner }), "conflict");
    let after_removal = bridge.ok(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": worker }),
    );
    assert_eq!(
        ids(&after_removal),
        project.only_the_pin(),
        "the legacy projection"
    );
    assert!(!project
        .agent_list(bridge, true)
        .contains(&worker.to_string()));
    bridge.wait_for_state(owner, &project.pin, "live");
}

/// Clearing the pin's conversation keeps its identity.
fn a_reset_keeps_the_pin(bridge: &Bridge, project: &Project) {
    let owner = &project.owner;
    let thread_id = bridge.roster(owner, true)["agents"][0]["thread_id"]
        .as_str()
        .unwrap()
        .to_string();
    bridge.ok(
        "conversation.reset",
        json!({
            "project_id": project.project_id, "entity_id": owner, "agent_id": project.pin,
            "conversation_id": project.pin, "expected_thread_id": thread_id,
        }),
    );
    let reset = bridge.roster(owner, true);
    assert_eq!(reset["pinned_agent_id"], json!(project.pin));
    assert_ne!(reset["agents"][0]["thread_id"], json!(thread_id));
}

/// A store from before pins migrates to the same pin, on every boot.
fn a_legacy_store_migrates_to_the_same_pin(project: &Project, worker: &str) {
    store_as_legacy(&project.root, &project.owner);
    for boot in 0..2 {
        let bridge = Bridge::boot(&project.root);
        assert_eq!(
            bridge.ensure(&project.project_id),
            project.owner,
            "boot {boot}"
        );
        let migrated = bridge.roster(&project.owner, true);
        assert_eq!(
            migrated["pinned_agent_id"],
            json!(project.pin),
            "boot {boot}"
        );
        assert_eq!(
            ids(&migrated),
            [project.pin.clone(), worker.to_string()],
            "boot {boot}"
        );
        assert_eq!(
            purposes(&migrated),
            ["coordinator", "worker"],
            "boot {boot}"
        );
        bridge.shutdown();
    }
}

#[test]
fn a_project_keeps_one_pinned_coordinator_through_workers_sessions_and_restarts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("PATH", private_path(&root));
    std::env::set_var("HOME", &home);
    let repo = init_repo(&root);

    let bridge = Bridge::boot(&root);
    let project = a_fresh_project(&bridge, &root, &repo);
    let (worker, second) = workers_below_the_pin(&bridge, &project);
    old_clients_see_the_pin_alone(&bridge, &project);
    each_agent_its_own_session(&bridge, &project, &worker);
    the_pin_stays_and_a_worker_goes(&bridge, &project, &worker);
    a_reset_keeps_the_pin(&bridge, &project);
    bridge.shutdown();

    a_legacy_store_migrates_to_the_same_pin(&project, &second);

    // A migrated worker still starts, and old clients still see the pin alone.
    let bridge = Bridge::boot(&root);
    bridge.ok(
        "thread.post",
        json!({ "entity_id": project.owner, "agent_id": second, "body": "still here?" }),
    );
    bridge.wait_for_state(&project.owner, &second, "live");
    assert_eq!(project.agent_list(&bridge, false), project.only_the_pin());
    bridge.shutdown();
}
