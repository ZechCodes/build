//! One boot of the whole daemon over temporary fixtures, for the tests that
//! drive a project's agents end to end: real frames through the real
//! [`FrameHandler`], a real task store and config, a real git project, and
//! the daemon's own QA agent as the only harness, on a PATH that holds no
//! provider CLI.
//!
//! Each test binary compiles its own copy and uses the part it needs, so
//! what another binary uses is not dead here.
#![allow(dead_code)]

use build_bridge::app::AppState;
use build_bridge::carrier::{FrameHandler, SessionSender};
use build_bridge::harness::HarnessContext;
use build_bridge::transport::Frame;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a fake child may take to come up on a loaded machine.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// A PATH holding only what the bridge and the fixtures run, so no provider
/// CLI on this machine is probed or started, and no systemd unit is made.
pub fn private_path(root: &Path) -> PathBuf {
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

pub fn git(dir: &Path, args: &[&str]) {
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

pub fn init_repo(root: &Path) -> PathBuf {
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
pub struct Bridge {
    runtime: tokio::runtime::Runtime,
    handler: FrameHandler,
}

impl Bridge {
    pub fn boot(root: &Path) -> Bridge {
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

    pub fn call(&self, method: &str, params: Value) -> Value {
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

    /// A real frame in flight beside the test's next operation. The runtime
    /// stays owned by this fixture until the caller has joined the thread.
    pub fn call_on_thread(&self, method: &str, params: Value) -> std::thread::JoinHandle<Value> {
        let handler = self.handler.clone();
        let runtime = self.runtime.handle().clone();
        let method = method.to_string();
        std::thread::spawn(move || {
            let _in_runtime = runtime.enter();
            handler.call(
                SessionSender::detached("concurrent-client"),
                Frame {
                    session_id: "concurrent-client".into(),
                    message_id: "concurrent-frame".into(),
                    frame_type: "rpc".into(),
                    sender: "client".into(),
                    created_at: "2026-10-13T00:00:00Z".into(),
                    payload: json!({ "id": "concurrent-frame", "method": method, "params": params }),
                },
            )
        })
    }

    pub fn ok(&self, method: &str, params: Value) -> Value {
        let answer = self.call(method, params);
        assert_eq!(answer["ok"], true, "{method}: {answer}");
        answer["result"].clone()
    }

    pub fn refused(&self, method: &str, params: Value, code: &str) -> Value {
        let answer = self.call(method, params);
        assert_eq!(answer["ok"], false, "{method}: {answer}");
        assert_eq!(answer["error_code"], code, "{method}: {answer}");
        answer
    }

    pub fn roster(&self, owner: &str, include_project_workers: bool) -> Value {
        self.ok(
            "agent.list",
            json!({ "entity_id": owner, "include_project_workers": include_project_workers }),
        )
    }

    pub fn ensure(&self, project_id: &str) -> String {
        self.ok(
            "project.ensure_conversation",
            json!({ "project_id": project_id }),
        )["run_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Wait for one agent's digest to say `state`.
    pub fn wait_for_state(&self, owner: &str, agent_id: &str, state: &str) {
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

    pub fn shutdown(self) {
        drop(self.handler);
        self.runtime.shutdown_timeout(Duration::from_secs(5));
    }
}

pub fn ids(roster: &Value) -> Vec<String> {
    roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap().to_string())
        .collect()
}

/// A session can be live before its initial delivery ticket settles. Only
/// that preflight refusal is transient here; deletion retires the live agent.
pub fn delete_after_delivery_settles(bridge: &Bridge, project: &str) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = bridge.call(
            "project.delete",
            json!({"project_id": project, "confirm": true}),
        );
        if answer["ok"] == true {
            return;
        }
        assert_eq!(answer["error_code"], "conflict", "{answer}");
        assert_eq!(
            answer["error"], "Stop running agents before deleting the project",
            "{answer}"
        );
        assert!(
            Instant::now() < deadline,
            "initial delivery never settled: {answer}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn purposes(roster: &Value) -> Vec<String> {
    roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["project_role"].as_str().unwrap().to_string())
        .collect()
}

/// The one scratch directory Build keeps a project's conversation in.
pub fn scratch_of(root: &Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("project-scratch"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}
