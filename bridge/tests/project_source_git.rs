//! The original-source Git contract (#529), end to end: real frames through
//! the real [`FrameHandler`], real repositories and a plain folder in a
//! temporary directory, real filesystem watchers and the real push flusher.
//!
//! One project holds three original sources — its primary repository, a
//! second repository whose paths and status are the primary's, and a plain
//! folder — and a browser session drives what project Changes will: it reads
//! the greeting, subscribes to each source, edits, stages and commits one
//! source without touching the others, and keeps a save captured before the
//! source moved from reaching the folder it moved to.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::carrier::{FrameHandler, OutboundEnvelope, SessionSender};
use build_bridge::changes;
use build_bridge::transport::Frame;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

/// One git command in `dir` with the fixture's identity and none of the
/// machine's global or system config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@build.ing")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@build.ing")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The repository's own config for what Build's git runs: an identity and no
/// signing, so a product commit never reaches for a real key.
fn configure(repo: &Path) {
    git(repo, &["config", "--local", "user.email", "test@build.ing"]);
    git(repo, &["config", "--local", "user.name", "Test"]);
    git(repo, &["config", "--local", "commit.gpgsign", "false"]);
}

fn primary_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("primary");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    configure(&repo);
    std::fs::write(repo.join("README.md"), "# project\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    std::fs::canonicalize(repo).unwrap()
}

/// A clone at the same commit with no remote: identical paths and status.
fn clone_of(dir: &Path, from: &Path, name: &str) -> PathBuf {
    git(dir, &["clone", "-q", from.to_str().unwrap(), name]);
    let clone = std::fs::canonicalize(dir.join(name)).unwrap();
    git(&clone, &["remote", "remove", "origin"]);
    configure(&clone);
    clone
}

/// The browser: one observable session against the real handler.
struct Browser {
    handler: FrameHandler,
    sender: SessionSender,
    pushes: UnboundedReceiver<OutboundEnvelope>,
    key: String,
    next_id: u64,
    heard: Vec<Value>,
}

impl Browser {
    fn new(handler: FrameHandler) -> Self {
        let (sender, pushes, key) = SessionSender::observable("browser");
        Self {
            handler,
            sender,
            pushes,
            key,
            next_id: 0,
            heard: Vec::new(),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let frame = Frame {
            session_id: "browser".into(),
            message_id: format!("m-{}", self.next_id),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: "t".into(),
            payload: json!({ "id": self.next_id.to_string(), "method": method, "params": params }),
        };
        self.handler.call(self.sender.clone(), frame)
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params.clone());
        assert_eq!(reply["ok"], true, "{method} {params}: {reply:?}");
        reply["result"].clone()
    }

    fn refused(&mut self, method: &str, params: Value) -> String {
        let reply = self.call(method, params.clone());
        assert_eq!(reply["ok"], false, "{method} {params}: {reply:?}");
        reply["error_code"].as_str().unwrap().to_string()
    }

    /// Every item one subscription has been sent so far.
    fn items(&self, subscription_id: &str) -> Vec<Value> {
        self.heard
            .iter()
            .filter(|push| push["type"] == "changes" && push["subscription_id"] == subscription_id)
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .collect()
    }

    /// Listen until `ready` holds over everything heard, or fail.
    async fn hear_until(&mut self, what: &str, ready: impl Fn(&Browser) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while !ready(self) {
            let pushed = tokio::time::timeout_at(deadline, self.pushes.recv())
                .await
                .unwrap_or_else(|_| panic!("never heard {what}: {:?}", self.heard))
                .expect("the session stays open");
            self.heard
                .push(SessionSender::decrypt_push(&self.key, &pushed));
        }
    }

    /// Listen for a while and keep what arrives, for a claim about silence.
    async fn hear_for(&mut self, quiet: Duration) {
        let until = tokio::time::Instant::now() + quiet;
        while let Ok(Some(pushed)) = tokio::time::timeout_at(until, self.pushes.recv()).await {
            self.heard
                .push(SessionSender::decrypt_push(&self.key, &pushed));
        }
    }
}

fn names_path(item: &Value, path: &str) -> bool {
    item["files"]["paths"]
        .as_array()
        .is_some_and(|paths| paths.iter().any(|named| named == path))
}

/// The project's source rows, as `project.list` carries them.
fn sources_of(browser: &mut Browser, project_id: &str) -> Vec<Value> {
    let listed = browser.ok("project.list", json!({}));
    listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == project_id)
        .unwrap()["sources"]
        .as_array()
        .unwrap()
        .clone()
}

/// A Git verb's scope for one source row: its id and its path as captured.
fn git_scope(project_id: &str, row: &Value) -> Value {
    json!({ "project_id": project_id, "source_id": row["id"], "source_path": row["path"] })
}

fn with(scope: &Value, extra: Value) -> Value {
    let mut params = scope.clone();
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

fn subscription(id: &str, project_id: &str, row: &Value, kinds: Value) -> Value {
    json!({
        "subscription_id": id,
        "scope": {
            "kind": "project_source", "project_id": project_id,
            "source_id": row["id"], "source_path": row["path"],
        },
        "kinds": kinds,
    })
}

fn daemon(dir: &Path, primary: &Path) -> Arc<Mutex<AppState>> {
    let context =
        build_bridge::harness::HarnessContext::resolved(dir.join("mcp.sock"), dir.join("home"))
            .expect("a private harness context");
    AppState::new_configured(primary, dir.join("worktrees"), "main", true, context)
        .with_task_store(dir.join("home").join("state"))
        .expect("a task store")
        .shared()
}

#[tokio::test]
async fn original_project_sources_drive_git_and_pushes_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("home")).unwrap();
    let primary = primary_repo(&root);
    let second = clone_of(&root, &primary, "second");
    let plain = root.join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.md"), "plain\n").unwrap();

    let state = daemon(&root, &primary);
    let mut browser = Browser::new(AppState::handler(Arc::clone(&state)));
    let greeting = browser.ok("session.hello", json!({ "changes": "subscriptions" }));
    assert_eq!(greeting["api_version"], "3.21.0");
    for capability in ["git.projectSources", "changes.projectSources"] {
        assert!(
            greeting["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(capability)),
            "{capability}"
        );
    }

    let listed = browser.ok("project.list", json!({}));
    let project_id = listed["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    for (path, name) in [(&second, "second"), (&plain, "plain")] {
        browser.ok(
            "project.add_source",
            json!({ "project_id": project_id, "path": path, "name": name }),
        );
    }
    let rows = sources_of(&mut browser, &project_id);
    assert_eq!(rows.len(), 3, "{rows:?}");
    let (first_row, second_row, plain_row) = (rows[0].clone(), rows[1].clone(), rows[2].clone());
    assert_eq!(second_row["is_git"], true);
    assert_eq!(plain_row["is_git"], false);

    // Subscribe as project Changes and Files will, plus an older client's
    // `all`, which must hear nothing it did not hear before.
    browser.ok(
        "changes.subscribe",
        subscription(
            "s-second",
            &project_id,
            &second_row,
            json!(["git", "files"]),
        ),
    );
    browser.ok(
        "changes.subscribe",
        subscription("s-plain", &project_id, &plain_row, json!(["files"])),
    );
    browser.ok(
        "changes.subscribe",
        json!({ "subscription_id": "s-all", "scope": { "kind": "all" }, "kinds": ["git", "files"] }),
    );
    assert_eq!(
        browser.refused(
            "changes.subscribe",
            subscription("s-refused", &project_id, &plain_row, json!(["git"])),
        ),
        "invalid_params",
        "a plain folder has no git to subscribe to"
    );

    // Identical edits in two repositories: separate keys, separate pushes.
    for repo in [&primary, &second] {
        std::fs::write(repo.join("same.txt"), "same\n").unwrap();
    }
    std::fs::write(plain.join("plain.txt"), "plain\n").unwrap();
    let (first, second_scope) = (
        git_scope(&project_id, &first_row),
        git_scope(&project_id, &second_row),
    );
    let first_status = browser.ok("git.status", first.clone());
    let second_status = browser.ok("git.status", second_scope.clone());
    assert_ne!(first_status["status_key"], second_status["status_key"]);
    assert_eq!(second_status["path"], second.display().to_string());
    browser
        .hear_until("the second source's file, git and the plain file", |b| {
            let second = b.items("s-second");
            second.iter().any(|item| names_path(item, "same.txt"))
                && second
                    .iter()
                    .any(|item| item["git"]["status_key"] == second_status["status_key"])
                && b.items("s-plain")
                    .iter()
                    .any(|item| names_path(item, "plain.txt"))
        })
        .await;
    let identity = |row: &Value| json!({ "project_id": project_id, "source_id": row["id"], "source_path": row["path"] });
    for (subscription_id, row) in [("s-second", &second_row), ("s-plain", &plain_row)] {
        for item in browser.items(subscription_id) {
            assert_eq!(item["project_source"], identity(row), "{item:?}");
        }
    }
    assert!(browser
        .items("s-plain")
        .iter()
        .all(|item| item.get("git").is_none() && !names_path(item, "same.txt")));

    // Stage and commit the second source alone, the shared pane's commit-all.
    browser.ok(
        "git.stage",
        with(&second_scope, json!({ "paths": ["same.txt"] })),
    );
    let committed = browser.ok(
        "git.commit",
        with(&second_scope, json!({ "message": "second only" })),
    );
    assert_eq!(committed["subject"], "second only");
    assert_eq!(git(&second, &["log", "-1", "--format=%s"]), "second only");
    assert_eq!(git(&primary, &["log", "-1", "--format=%s"]), "initial");
    assert!(git(&primary, &["status", "--porcelain"]).contains("same.txt"));
    let history = browser.ok("git.log", second_scope.clone());
    assert_eq!(history["commits"][0]["subject"], "second only");
    assert_eq!(
        browser.refused("git.status", git_scope(&project_id, &plain_row)),
        "invalid_params"
    );

    // Move the second source in settings. A save captured before the move is
    // refused instead of reaching the new folder; the new path works.
    let moved = clone_of(&root, &second, "moved");
    std::fs::write(moved.join("draft.txt"), "draft\n").unwrap();
    std::fs::write(second.join("draft.txt"), "draft\n").unwrap();
    browser.ok(
        "project.update_source",
        json!({ "project_id": project_id, "source_id": second_row["id"], "path": moved }),
    );
    for (method, extra) in [
        ("git.stage", json!({ "paths": ["draft.txt"] })),
        ("git.commit", json!({ "message": "redirected" })),
    ] {
        assert_eq!(
            browser.refused(method, with(&second_scope, extra)),
            "conflict",
            "{method} with the folder captured before the move"
        );
    }
    assert_eq!(git(&moved, &["log", "-1", "--format=%s"]), "second only");
    assert!(git(&moved, &["status", "--porcelain"]).contains("draft.txt"));
    let moved_row = sources_of(&mut browser, &project_id)[1].clone();
    assert_eq!(moved_row["path"], moved.display().to_string());
    let moved_status = browser.ok("git.status", git_scope(&project_id, &moved_row));
    assert_eq!(moved_status["path"], moved.display().to_string());
    assert_eq!(
        browser.refused(
            "changes.subscribe",
            subscription("s-stale", &project_id, &second_row, json!(["files"])),
        ),
        "conflict"
    );

    // The old subscription goes quiet; one naming the new folder hears it.
    browser.ok(
        "changes.subscribe",
        subscription("s-moved", &project_id, &moved_row, json!(["files"])),
    );
    // Past the settle window, so what the old folder said before it moved
    // has been delivered.
    browser.hear_for(Duration::from_millis(1500)).await;
    let before = browser.items("s-second").len();
    std::fs::write(second.join("old.txt"), "old\n").unwrap();
    std::fs::write(moved.join("new.txt"), "new\n").unwrap();
    browser
        .hear_until("the moved folder's file", |b| {
            b.items("s-moved")
                .iter()
                .any(|item| names_path(item, "new.txt"))
        })
        .await;
    browser.hear_for(Duration::from_millis(1500)).await;
    let old_items = browser.items("s-second");
    assert_eq!(old_items.len(), before, "{:?}", browser.heard);
    assert!(old_items.iter().all(|item| !names_path(item, "old.txt")));

    assert!(
        browser.items("s-all").iter().all(|item| {
            item.get("project_source").is_none()
                && !item["entity_id"]
                    .as_str()
                    .is_some_and(changes::is_project_source_entity)
        }),
        "an older client's `all` hears no source item: {:?}",
        browser.heard
    );
    drop(state);
}
