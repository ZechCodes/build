//! A real `branch.finish` frame with Git losing both create-only restoration
//! attempts to a competing ref lock. The workspace still goes, while the
//! reply and linked issue retain the measured commit needed for recovery.

use build_bridge::{
    app::AppState,
    carrier::{FrameHandler, SessionSender},
    harness::HarnessContext,
    transport::Frame,
};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
    handler.call(
        SessionSender::detached("branch-recovery"),
        Frame {
            session_id: "branch-recovery".into(),
            message_id: "1".into(),
            frame_type: "rpc".into(),
            sender: "client".into(),
            created_at: "2026-09-25T00:00:00Z".into(),
            payload: json!({ "id": "1", "method": method, "params": params }),
        },
    )
}

fn result(handler: &FrameHandler, method: &str, params: Value) -> Value {
    let answer = call(handler, method, params);
    assert_eq!(answer["ok"], true, "{method}: {answer}");
    answer["result"].clone()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct RecoveryCase {
    root: tempfile::TempDir,
    repo: std::path::PathBuf,
    handler: FrameHandler,
    project: String,
    workspace_id: String,
    branch: String,
    checkout: String,
    measured: String,
    issue: String,
    attempts: std::path::PathBuf,
}

impl RecoveryCase {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@build.ing"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
        let origin = root.path().join("origin.git");
        git(
            root.path(),
            &[
                "clone",
                "--bare",
                repo.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&repo, &["fetch", "origin"]);
        git(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);

        let handler = AppState::new_unrooted_configured(
            root.path().join("worktrees"),
            "main",
            true,
            HarnessContext::resolved(root.path().join("mcp.sock"), root.path().to_path_buf())
                .unwrap(),
        )
        .with_config(root.path().join("config.json"))
        .unwrap()
        .with_task_store(root.path().join("store"))
        .unwrap()
        .into_handler();
        let project = result(&handler, "project.add", json!({ "path": repo }))["project_id"]
            .as_str()
            .unwrap()
            .to_string();
        let workspace = result(
            &handler,
            "workspace.create",
            json!({
                "project_id": project, "name": "recover", "isolation": "worktree"
            }),
        );
        let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
        let branch = workspace["directories"][0]["branch"]
            .as_str()
            .unwrap()
            .to_string();
        let checkout = workspace["directories"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        let measured = git(&repo, &["rev-parse", &format!("refs/heads/{branch}")]);
        git(
            &repo,
            &["config", &format!("branch.{branch}.remote"), "origin"],
        );
        let issue = result(
            &handler,
            "issues.create",
            json!({
                "project_id": project, "title": "recover this branch"
            }),
        )["issue"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        result(
            &handler,
            "issues.link",
            json!({ "issue_id": issue, "workspace_id": workspace_id }),
        );

        let attempts = root.path().join("attempts");
        Self {
            root,
            repo,
            handler,
            project,
            workspace_id,
            branch,
            checkout,
            measured,
            issue,
            attempts,
        }
    }

    fn install_competing_lock(&self) -> String {
        // Only this integration test runs in this process, so the PATH wrapper is
        // isolated from every other test binary. It switches the source checkout
        // onto the branch just before the measured-OID delete, then holds the
        // ref lock across both restoration attempts.
        let bin = self.root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        std::fs::write(&wrapper, r##"#!/bin/sh
if [ "$PWD" = "$BRANCH_RESTORE_REPO" ] && [ "$1" = update-ref ] && [ "$2" = -d ] && [ "$3" = "$BRANCH_RESTORE_REF" ]; then
  /usr/bin/git switch "$BRANCH_RESTORE_BRANCH" || exit $?
  exec /usr/bin/git "$@"
fi
if [ "$PWD" = "$BRANCH_RESTORE_REPO" ] && [ "$1" = update-ref ] && [ "$2" = "$BRANCH_RESTORE_REF" ]; then
  mkdir -p "$(dirname "$BRANCH_RESTORE_LOCK")"
  : > "$BRANCH_RESTORE_LOCK"
  echo attempt >> "$BRANCH_RESTORE_ATTEMPTS"
  exec /usr/bin/git "$@"
fi
exec /usr/bin/git "$@"
"##).unwrap();
        let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&wrapper, permissions).unwrap();
        let old_path = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{old_path}", bin.display()));
        std::env::set_var("BRANCH_RESTORE_REPO", &self.repo);
        std::env::set_var("BRANCH_RESTORE_REF", format!("refs/heads/{}", self.branch));
        std::env::set_var("BRANCH_RESTORE_BRANCH", &self.branch);
        std::env::set_var(
            "BRANCH_RESTORE_LOCK",
            self.repo
                .join(".git/refs/heads")
                .join(format!("{}.lock", self.branch)),
        );
        std::env::set_var("BRANCH_RESTORE_ATTEMPTS", &self.attempts);
        old_path
    }

    fn assert_outcome(&self, finished: &Value) {
        assert_eq!(finished["deleted"], true, "{finished}");
        assert_eq!(finished["branch_deleted"], true, "{finished}");
        let reason = finished["branch_reason"].as_str().unwrap();
        assert!(reason.contains(&self.measured), "{reason}");
        assert!(
            reason.contains(&self.repo.display().to_string()),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("checked out at {}", self.repo.display())),
            "{reason}"
        );
        assert!(
            reason.contains("checkout still names the missing branch"),
            "{reason}"
        );
        self.assert_git_state();
        self.assert_issue_event(reason);
    }

    fn assert_git_state(&self) {
        assert!(!Path::new(&self.checkout).exists());
        assert_eq!(
            git(&self.repo, &["symbolic-ref", "HEAD"]),
            format!("refs/heads/{}", self.branch)
        );
        assert!(!Command::new("/usr/bin/git")
            .current_dir(&self.repo)
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", self.branch)
            ])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(
            std::fs::read_to_string(&self.attempts)
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert_eq!(
            git(
                &self.repo,
                &["config", "--get", &format!("branch.{}.remote", self.branch)]
            ),
            "origin"
        );
        let listed = result(
            &self.handler,
            "workspace.list",
            json!({ "project_id": self.project }),
        );
        assert_eq!(listed["workspaces"], json!([]));
    }

    fn assert_issue_event(&self, reason: &str) {
        let timeline = result(
            &self.handler,
            "issues.get",
            json!({ "issue_id": self.issue }),
        )["timeline"]
            .clone();
        let logged = timeline
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["kind"] == "branch_deleted")
            .unwrap();
        assert_eq!(logged["payload"]["reason"], reason);
        assert_eq!(logged["payload"]["branch"], self.branch);
        assert_eq!(logged["payload"]["workspace_id"], self.workspace_id);
    }
}

#[test]
fn failed_restore_is_reported_on_the_frame_and_linked_issue() {
    let case = RecoveryCase::new();
    let old_path = case.install_competing_lock();
    let finished = result(
        &case.handler,
        "branch.finish",
        json!({
            "project_id": case.project, "branch": case.branch, "action": "delete"
        }),
    );
    std::env::set_var("PATH", old_path);
    case.assert_outcome(&finished);
}
