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

use build_bridge::app::AppState;
use build_bridge::carrier::{FrameHandler, SessionSender};
use build_bridge::harness::HarnessContext;
use build_bridge::store::Store;
use build_bridge::transport::Frame;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a fake child may take to come up on a loaded machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// A PATH holding only what the bridge and the fixtures run, so no provider
/// CLI on this machine is probed or started, and no systemd unit is made.
fn private_path(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["sh", "git", "cat"] {
        let found = ["/usr/bin", "/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(tool))
            .find(|path| path.exists())
            .unwrap_or_else(|| panic!("no {tool} on this machine"));
        std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
    }
    bin
}

fn git(dir: &Path, args: &[&str]) {
    // Nothing from the machine's own git config: a `commit.gpgsign` there
    // would sign this fixture with a real key.
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@build.ing"]);
    git(&repo, &["config", "--local", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README.md"), "the project\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "initial"]);
    std::fs::canonicalize(repo).unwrap()
}

/// One boot of the daemon over `root`'s store.
struct Bridge {
    runtime: tokio::runtime::Runtime,
    handler: FrameHandler,
}

impl Bridge {
    fn boot(root: &Path) -> Bridge {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handler = {
            let _in_runtime = runtime.enter();
            AppState::new_unrooted_configured(
                root.join("worktrees"),
                "main",
                true,
                HarnessContext::resolved(root.join("mcp.sock"), root.to_path_buf()).unwrap(),
            )
            .with_config(root.join("config.json"))
            .unwrap()
            .with_task_store(root.join("store"))
            .unwrap()
            .into_handler()
        };
        Bridge { runtime, handler }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        let _in_runtime = self.runtime.enter();
        self.handler.call(
            SessionSender::detached("project-coordinator"),
            Frame {
                session_id: "project-coordinator".into(),
                message_id: "1".into(),
                frame_type: "rpc".into(),
                sender: "client".into(),
                created_at: "2026-10-12T00:00:00Z".into(),
                payload: json!({ "id": "1", "method": method, "params": params }),
            },
        )
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        let answer = self.call(method, params);
        assert_eq!(answer["ok"], true, "{method}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, method: &str, params: Value, code: &str) -> Value {
        let answer = self.call(method, params);
        assert_eq!(answer["ok"], false, "{method}: {answer}");
        assert_eq!(answer["error_code"], code, "{method}: {answer}");
        answer
    }

    fn roster(&self, owner: &str, include_project_workers: bool) -> Value {
        self.ok(
            "agent.list",
            json!({ "entity_id": owner, "include_project_workers": include_project_workers }),
        )
    }

    fn ensure(&self, project_id: &str) -> String {
        self.ok(
            "project.ensure_conversation",
            json!({ "project_id": project_id }),
        )["run_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Wait for one agent's digest to say `state`.
    fn wait_for_state(&self, owner: &str, agent_id: &str, state: &str) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let roster = self.roster(owner, true);
            let digest = roster["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|agent| agent["id"] == agent_id)
                .cloned();
            if digest.as_ref().is_some_and(|agent| agent["state"] == state) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{agent_id} never became {state}: {roster}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn shutdown(self) {
        drop(self.handler);
        self.runtime.shutdown_timeout(Duration::from_secs(5));
    }
}

fn ids(roster: &Value) -> Vec<String> {
    roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap().to_string())
        .collect()
}

fn purposes(roster: &Value) -> Vec<String> {
    roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["project_role"].as_str().unwrap().to_string())
        .collect()
}

/// The one scratch directory Build keeps a project's conversation in.
fn scratch_of(root: &Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("project-scratch"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}

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

#[test]
fn a_project_keeps_one_pinned_coordinator_through_workers_sessions_and_restarts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("PATH", private_path(&root));
    std::env::set_var("HOME", &home);
    let repo = init_repo(&root);

    // ---- a fresh project is born with exactly one pin -----------------------
    let bridge = Bridge::boot(&root);
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
    assert_eq!(ids(&fresh), [pin.clone()]);

    // ---- workers are added below it; nobody can mint a coordinator ----------
    let worker = bridge.ok(
        "agent.add",
        json!({ "entity_id": owner, "name": "Docs pass" }),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let second = bridge.ok(
        "agent.add",
        json!({ "entity_id": owner, "project_role": "worker" }),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    bridge.refused(
        "agent.add",
        json!({ "entity_id": owner, "project_role": "coordinator" }),
        "invalid_params",
    );
    let whole = bridge.roster(&owner, true);
    assert_eq!(ids(&whole), [pin.clone(), worker.clone(), second.clone()]);
    assert_eq!(purposes(&whole), ["coordinator", "worker", "worker"]);

    // ---- a client that predates workers still sees one project agent --------
    assert_eq!(ids(&bridge.roster(&owner, false)), [pin.clone()]);
    let board = bridge.ok("board.list", json!({}));
    let row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == json!(owner))
        .unwrap()
        .clone();
    assert_eq!(ids(&row), [pin.clone()], "{row}");

    // ---- sessions: each agent its own child, scaffold and MCP config --------
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": worker, "body": "tidy the README" }),
    );
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "body": "what is in flight?" }),
    );
    bridge.wait_for_state(&owner, &worker, "live");
    bridge.wait_for_state(&owner, &pin, "live");
    let scratch = scratch_of(&root);
    for agent in [&pin, &worker] {
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
        !repo.join(".build").exists(),
        "no agent scaffold in the sources"
    );

    // ---- the pin cannot be taken away; a worker can, and only it goes -------
    let refusal = bridge.refused(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": pin }),
        "conflict",
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("Delete the project"),
        "{refusal}"
    );
    bridge.refused("run.abandon", json!({ "run_id": owner }), "conflict");
    let after_removal = bridge.ok(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": worker }),
    );
    assert_eq!(ids(&after_removal), [pin.clone()], "the legacy projection");
    assert_eq!(ids(&bridge.roster(&owner, true)), [pin.clone(), second.clone()]);
    bridge.wait_for_state(&owner, &pin, "live");

    // ---- clearing the pin's conversation keeps its identity ----------------
    let thread_id = bridge.roster(&owner, true)["agents"][0]["thread_id"]
        .as_str()
        .unwrap()
        .to_string();
    bridge.ok(
        "conversation.reset",
        json!({
            "project_id": project_id, "entity_id": owner, "agent_id": pin,
            "conversation_id": pin, "expected_thread_id": thread_id,
        }),
    );
    let reset = bridge.roster(&owner, true);
    assert_eq!(reset["pinned_agent_id"], json!(pin));
    assert_ne!(reset["agents"][0]["thread_id"], json!(thread_id));
    bridge.shutdown();

    // ---- a store from before pins migrates to the same pin, twice -----------
    store_as_legacy(&root, &owner);
    for boot in 0..2 {
        let bridge = Bridge::boot(&root);
        assert_eq!(bridge.ensure(&project_id), owner, "boot {boot}");
        let migrated = bridge.roster(&owner, true);
        assert_eq!(migrated["pinned_agent_id"], json!(pin), "boot {boot}");
        assert_eq!(ids(&migrated), [pin.clone(), second.clone()], "boot {boot}");
        assert_eq!(purposes(&migrated), ["coordinator", "worker"], "boot {boot}");
        bridge.shutdown();
    }

    // ---- and a migrated worker still starts as a worker ---------------------
    let bridge = Bridge::boot(&root);
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": second, "body": "still here?" }),
    );
    bridge.wait_for_state(&owner, &second, "live");
    assert_eq!(ids(&bridge.roster(&owner, false)), [pin.clone()]);
    bridge.shutdown();
}
