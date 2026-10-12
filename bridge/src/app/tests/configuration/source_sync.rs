//! Keeping each source's base branch in step with its remote (#267): the
//! setting and its default, the service's passes, `project.sync_source`, and
//! the sync a workspace cut runs first. Build's own Git on an original source
//! takes its turn with those syncs (#530).

use super::*;
use crate::app::projects::{SYNC_BASE_FOR_EXISTING_SOURCES, SYNC_BASE_FOR_NEW_SOURCES};
use crate::app::tests::workflow::lifecycle_offlock::frame_on_a_thread;
use crate::app::SyncPass;
use crate::source_sync::{sync_base, Fetch, SyncLock, SyncOutcome};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    state: AppState,
    project_id: String,
    code_upstream: PathBuf,
    code: PathBuf,
    docs: PathBuf,
}

/// A base checkout cloned from `<name>-upstream`, as a source stands on one.
fn clone_of_upstream(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let upstream = init_repo_named(root, &format!("{name}-upstream"));
    let base = root.join(name);
    git_in(
        root,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            base.to_str().unwrap(),
        ],
    );
    crate::git_fixture::configure_repo(&base);
    (upstream, base)
}

fn advance(repo: &Path, file: &str) {
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", file]);
}

fn head(repo: &Path, name: &str) -> String {
    let output = crate::git_fixture::git_command(repo, &["rev-parse", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn configured(root: &Path, config: &Path) -> AppState {
    AppState::new_unrooted(root.join("worktrees"), "main", true, "/tmp/test-mcp.sock")
        .with_config(config)
        .unwrap()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (code_upstream, code) = clone_of_upstream(&root, "code");
    let (_, docs) = clone_of_upstream(&root, "docs");
    let config = root.join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let mut state = configured(&root, &config);
    let opened = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": code}, {"path": docs}]}),
    ));
    assert_eq!(opened["ok"], true, "{opened:?}");
    let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
    Fixture {
        _dir: dir,
        root,
        config,
        state,
        project_id,
        code_upstream,
        code,
        docs,
    }
}

impl Fixture {
    fn row_source(&self, id: &str) -> Value {
        let project = self.state.projects.get(&self.project_id).unwrap();
        let row = self.state.project_json(project);
        row["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == id)
            .unwrap()
            .clone()
    }

    fn stored_sources(&self) -> Vec<Value> {
        let config: Value = serde_json::from_slice(&std::fs::read(&self.config).unwrap()).unwrap();
        config["projects"][0]["sources"].as_array().unwrap().clone()
    }

    /// One pass of the service, on the state as it stands.
    fn pass(self, pass: SyncPass) -> Self {
        let Fixture {
            _dir,
            root,
            config,
            state,
            project_id,
            code_upstream,
            code,
            docs,
        } = self;
        let shared = Arc::new(Mutex::new(state));
        AppState::sync_sources(&shared, pass, 1_000);
        let state = Arc::try_unwrap(shared).ok().unwrap().into_inner().unwrap();
        Fixture {
            _dir,
            root,
            config,
            state,
            project_id,
            code_upstream,
            code,
            docs,
        }
    }

    fn update(&mut self, source_id: &str, change: Value) -> Value {
        let mut params = json!({"project_id": self.project_id, "source_id": source_id});
        params
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        self.state.handle(req("project.update_source", params))
    }
}

/// The user's call (Sep 29 2026): a source added before the setting existed
/// syncs, the same as a new one. Nothing is written for it until someone
/// chooses, so the default stays one constant.
#[test]
fn a_source_that_predates_the_setting_syncs() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (_, code) = clone_of_upstream(&root, "code");
    let config = root.join("config.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "projects": [{
                "id": "proj-1",
                "path": code,
                "base_branch": "main",
                "sources": [{
                    "id": "source-1", "name": "code", "mount": "code",
                    "path": code, "is_git": true, "base_branch": "main"
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let state = configured(&root, &config);
    let project = state.projects.get("proj-1").unwrap();
    // The decision itself, pinned: flipping it is a one-line change here.
    const { assert!(SYNC_BASE_FOR_EXISTING_SOURCES) };
    assert_eq!(project.sources[0].sync_base, None);
    assert!(project.sources[0].syncs_base());
    assert_eq!(state.project_json(project)["sources"][0]["sync_base"], true);

    state.persist();
    let written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert!(
        written["projects"][0]["sources"][0]
            .get("sync_base")
            .is_none(),
        "{written}"
    );
}

#[test]
fn a_new_source_syncs_and_says_so() {
    let fixture = fixture();

    assert_eq!(fixture.row_source("source-1")["sync_base"], true);
    assert_eq!(fixture.row_source("source-1")["sync"], Value::Null);
    assert!(fixture
        .stored_sources()
        .iter()
        .all(|source| source["sync_base"] == SYNC_BASE_FOR_NEW_SOURCES));
}

#[test]
fn the_timer_fast_forwards_every_source_that_syncs_and_the_row_says_how() {
    let fixture = fixture();
    advance(&fixture.code_upstream, "news.txt");

    let fixture = fixture.pass(SyncPass::Due);

    assert_eq!(
        head(&fixture.code, "main"),
        head(&fixture.code_upstream, "main")
    );
    assert!(fixture.code.join("news.txt").exists());
    let sync = &fixture.row_source("source-1")["sync"];
    assert_eq!(sync["state"], "synced", "{sync}");
    assert_eq!(sync["commits"], 1);
    assert_eq!(sync["last_synced_ms"], 1_000);
    assert_eq!(fixture.row_source("source-2")["sync"]["state"], "synced");
}

#[test]
fn turning_sync_off_is_kept_and_the_timer_then_leaves_the_source_alone() {
    let mut fixture = fixture();
    let before = head(&fixture.code, "main");

    let turned = fixture.update("source-1", json!({"sync_base": false}));
    assert_eq!(turned["ok"], true, "{turned:?}");
    assert_eq!(turned["result"]["sources"][0]["sync_base"], false);
    assert_eq!(fixture.stored_sources()[0]["sync_base"], false);
    let restarted = configured(&fixture.root, &fixture.config);
    assert!(!restarted.projects.get(&fixture.project_id).unwrap().sources[0].syncs_base());

    advance(&fixture.code_upstream, "news.txt");
    let fixture = fixture.pass(SyncPass::Due);

    assert_eq!(head(&fixture.code, "main"), before);
    assert_eq!(fixture.row_source("source-1")["sync"], Value::Null);
}

#[test]
fn turning_sync_on_asks_for_a_sync_now() {
    let mut fixture = fixture();
    fixture.update("source-1", json!({"sync_base": false}));
    advance(&fixture.code_upstream, "news.txt");

    let turned = fixture.update("source-1", json!({"sync_base": true}));
    assert_eq!(turned["ok"], true, "{turned:?}");
    let fixture = fixture.pass(SyncPass::Requested);

    assert_eq!(
        head(&fixture.code, "main"),
        head(&fixture.code_upstream, "main")
    );
    assert_eq!(
        fixture.row_source("source-2")["sync"],
        Value::Null,
        "only the one asked for"
    );
}

#[test]
fn one_source_failing_does_not_stop_the_others() {
    let fixture = fixture();
    let nowhere = fixture.root.join("gone.git");
    git_in(
        &fixture.docs,
        &["remote", "set-url", "origin", nowhere.to_str().unwrap()],
    );
    advance(&fixture.code_upstream, "news.txt");

    let fixture = fixture.pass(SyncPass::Due);

    assert_eq!(fixture.row_source("source-2")["sync"]["state"], "failed");
    assert_eq!(fixture.row_source("source-1")["sync"]["state"], "synced");
    assert_eq!(
        head(&fixture.code, "main"),
        head(&fixture.code_upstream, "main")
    );
}

/// A remote that wanted a person is not asked again every five minutes: the
/// timer leaves it until somebody syncs it by hand.
#[test]
fn a_source_that_needed_you_waits_for_sync_now() {
    let mut fixture = fixture();
    advance(&fixture.code_upstream, "news.txt");
    let before = head(&fixture.code, "main");
    fixture
        .state
        .projects
        .source_mut(&fixture.project_id, "source-1")
        .unwrap()
        .sync_status = Some(
        serde_json::from_value(json!({
            "state": "failed", "reason": "origin did not answer within 30 s.",
            "ahead": 0, "behind": 0, "commits": 0, "needs_you": true,
            "last_attempt_ms": 1, "last_synced_ms": null, "last_fetched_ms": null
        }))
        .unwrap(),
    );

    let mut fixture = fixture.pass(SyncPass::Due);
    assert_eq!(head(&fixture.code, "main"), before);
    assert_eq!(fixture.row_source("source-1")["sync"]["needs_you"], true);

    let asked = fixture.state.handle(req(
        "project.sync_source",
        json!({"project_id": fixture.project_id, "source_id": "source-1"}),
    ));
    assert_eq!(asked["result"]["pending"], true, "{asked:?}");
    let fixture = fixture.pass(SyncPass::Requested);

    assert_eq!(
        head(&fixture.code, "main"),
        head(&fixture.code_upstream, "main")
    );
    assert_eq!(fixture.row_source("source-1")["sync"]["needs_you"], false);
}

#[test]
fn sync_now_refuses_a_folder_with_no_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let plain = root.join("plain");
    std::fs::create_dir(&plain).unwrap();
    let mut state = configured(&root, &root.join("config.json"));
    let project_id = state.add_project(plain, "main".into());

    let asked = state.handle(req(
        "project.sync_source",
        json!({"project_id": project_id, "source_id": "source-1"}),
    ));

    assert_eq!(asked["ok"], false, "{asked:?}");
    let turned = state.handle(req(
        "project.update_source",
        json!({"project_id": project_id, "source_id": "source-1", "sync_base": true}),
    ));
    assert_eq!(turned["ok"], false, "{turned:?}");
}

fn cut(fixture: &mut Fixture, name: &str) -> Value {
    let created = fixture.state.handle(req(
        "workspace.create",
        json!({"project_id": fixture.project_id, "name": name, "isolation": "worktree"}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"].clone()
}

fn checkout_of(created: &Value, mount: &str) -> PathBuf {
    let directory = created["directories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|directory| directory["name"] == mount)
        .unwrap_or_else(|| panic!("no {mount} in {created}"));
    PathBuf::from(directory["path"].as_str().unwrap())
}

/// The failure #267 exists to stop: a workspace cut from a base that has
/// fallen behind its remote. Where the base is checked out nowhere, the cut
/// moves its ref, which is instant.
#[test]
fn a_workspace_is_cut_from_the_base_its_remote_has_now() {
    let mut fixture = fixture();
    git_in(&fixture.code, &["switch", "-q", "--detach"]);
    advance(&fixture.code_upstream, "news.txt");

    let created = cut(&mut fixture, "fresh");

    assert!(created.get("warnings").is_none(), "{created}");
    assert!(checkout_of(&created, "code").join("news.txt").exists());
    assert_eq!(fixture.row_source("source-1")["sync"]["state"], "synced");
}

/// Where the base is checked out in the source's own checkout, moving it
/// moves its files, which a filter can make take minutes. The cut does not
/// wait for that (review #268): it branches from the commit the fetch
/// brought, with nothing to warn about, and the service fast-forwards the
/// checkout straight after (#271).
#[test]
fn a_cut_from_a_checked_out_base_branches_from_the_fetched_commit() {
    let mut fixture = fixture();
    let before = head(&fixture.code, "main");
    advance(&fixture.code_upstream, "news.txt");

    let created = cut(&mut fixture, "fresh");

    assert!(created.get("warnings").is_none(), "{created}");
    let checkout = checkout_of(&created, "code");
    assert!(checkout.join("news.txt").exists());
    assert_eq!(
        head(&checkout, "HEAD"),
        head(&fixture.code_upstream, "main")
    );
    assert_eq!(head(&fixture.code, "main"), before);

    let fixture = fixture.pass(SyncPass::Requested);

    assert!(fixture.code.join("news.txt").exists());
    assert_eq!(
        head(&fixture.code, "main"),
        head(&fixture.code_upstream, "main")
    );
    assert_eq!(fixture.row_source("source-1")["sync"]["state"], "synced");
}

/// A checked-out base the service could not fast-forward either is cut from
/// as it stands, and the cut says why.
#[test]
fn a_cut_from_a_checked_out_base_that_cannot_move_goes_ahead_from_it_and_says_why() {
    let mut fixture = fixture();
    advance(&fixture.code_upstream, "news.txt");
    std::fs::write(fixture.code.join("README.md"), "my unsaved edit\n").unwrap();

    let created = cut(&mut fixture, "behind");

    let warnings = created["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("{created}"));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("1 commit behind its remote. The base checkout has uncommitted changes."),
        "{warnings:?}"
    );
    assert!(!checkout_of(&created, "code").join("news.txt").exists());
    assert_eq!(fixture.row_source("source-1")["sync"]["state"], "skipped");
}

#[test]
fn a_cut_from_a_base_that_could_not_be_synced_goes_ahead_and_says_so() {
    let mut fixture = fixture();
    let nowhere = fixture.root.join("gone.git");
    git_in(
        &fixture.code,
        &["remote", "set-url", "origin", nowhere.to_str().unwrap()],
    );

    let created = cut(&mut fixture, "stale");

    let warnings = created["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("{created}"));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .starts_with("code: this workspace was cut from main as it stood"),
        "{warnings:?}"
    );
    assert!(checkout_of(&created, "code").exists());
    assert_eq!(fixture.row_source("source-1")["sync"]["state"], "failed");
}

#[test]
fn a_cut_leaves_a_source_with_sync_off_as_it_is() {
    let mut fixture = fixture();
    fixture.update("source-1", json!({"sync_base": false}));
    advance(&fixture.code_upstream, "news.txt");

    let created = cut(&mut fixture, "as-is");

    assert!(!checkout_of(&created, "code").join("news.txt").exists());
    assert_eq!(fixture.row_source("source-1")["sync"], Value::Null);
}

/// Agents cut most workspaces, so the warning reaches the agent that asked:
/// in the answer to its own `create_workspace` or `assign_task` call.
#[test]
fn the_agent_that_asked_for_the_workspace_hears_its_base_may_be_stale() {
    use crate::app::tests::project_agent::project_agent;
    use crate::app::tests::tracker::{filed, tracked_with_origin};
    use crate::mcp::BridgeAction;

    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let repo = state.projects.get(&project_id).unwrap().repo_path.clone();
    let nowhere = state_root.join("gone.git");
    git_in(
        &repo,
        &["remote", "set-url", "origin", nowhere.to_str().unwrap()],
    );
    let (owner, caller) = project_agent(&mut state, &project_id);

    let created = state
        .agent_action(
            &owner,
            &caller,
            BridgeAction::CreateWorkspace {
                name: "stale".to_string(),
                isolation: Some("worktree".to_string()),
            },
        )
        .expect("the cut goes ahead");
    assert_eq!(
        created["warnings"].as_array().map(Vec::len),
        Some(1),
        "{created}"
    );

    let task_id = filed(&mut state, &project_id, "on a stale base")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let assigned = state
        .agent_action(
            &owner,
            &caller,
            BridgeAction::TrackerAssignTask {
                task_id,
                assignee: json!({ "kind": "new_workspace", "isolation": "worktree" }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("the dispatch goes ahead");
    assert_eq!(
        assigned["warnings"].as_array().map(Vec::len),
        Some(1),
        "{assigned}"
    );
}

/// The scope a client holding a source's row sends for its Git.
fn source_scope(project_id: &str, source_id: &str, path: &Path) -> Value {
    json!({ "project_id": project_id, "source_id": source_id, "source_path": path.display().to_string() })
}

fn with(scope: &Value, extra: Value) -> Value {
    let mut params = scope.clone();
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

/// How long a test lets a frame that should be waiting go unanswered before
/// it takes the silence as waiting.
const STILL_WAITING: Duration = Duration::from_millis(400);
const ANSWERED_WITHIN: Duration = Duration::from_secs(30);

/// Build's own Git on an original source takes its turn behind a base sync
/// of the same repository instead of racing it (#530, PS522-4), and waits
/// with the app mutex free: other frames, a status read of that very source
/// among them, are answered meanwhile. The stage asked for while the sync
/// held the source lands after it, so the sync found the index clean and
/// fast-forwarded; the commit then lands on top of what the sync brought,
/// and the uncommitted file survives both.
#[test]
fn project_source_mutation_serializes_with_base_sync() {
    let Fixture {
        _dir,
        state,
        project_id,
        code_upstream,
        code,
        ..
    } = fixture();
    advance(&code_upstream, "news.txt");
    std::fs::write(code.join("work.txt"), "mine\n").unwrap();
    std::fs::write(code.join("scratch.txt"), "unsaved\n").unwrap();
    let scope = source_scope(&project_id, "source-1", &code);
    let state = state.shared();

    let (held, _) = SyncLock::acquire(&code, Duration::from_secs(1)).unwrap();
    let staging = frame_on_a_thread(
        &state,
        "stage",
        "git.stage",
        with(&scope, json!({ "paths": ["work.txt"] })),
    );
    assert!(
        staging.recv_timeout(STILL_WAITING).is_err(),
        "the stage waits for the sync holding the source"
    );
    assert!(state.try_lock().is_ok(), "the wait holds the app mutex");
    let status = frame_on_a_thread(&state, "status", "git.status", scope.clone())
        .recv_timeout(ANSWERED_WITHIN)
        .expect("a read of the same source is answered while the stage waits");
    assert_eq!(status["ok"], true, "{status}");
    let listed = frame_on_a_thread(&state, "list", "project.list", json!({}))
        .recv_timeout(ANSWERED_WITHIN)
        .expect("an unrelated frame is answered while the stage waits");
    assert_eq!(listed["ok"], true, "{listed}");

    // What the service does with the lock it holds: the index is still
    // clean, so the base fast-forwards.
    let synced = sync_base(&code, "main", Fetch::Within(Duration::from_secs(10)));
    assert!(
        matches!(synced.outcome, SyncOutcome::FastForwarded { commits: 1 }),
        "{synced:?}"
    );
    drop(held);

    let staged = staging.recv_timeout(ANSWERED_WITHIN).unwrap();
    assert_eq!(staged["ok"], true, "{staged}");
    let committed = frame_on_a_thread(
        &state,
        "commit",
        "git.commit",
        with(&scope, json!({ "message": "my work" })),
    )
    .recv_timeout(ANSWERED_WITHIN)
    .unwrap();
    assert_eq!(committed["ok"], true, "{committed}");
    assert_eq!(head(&code, "main^"), head(&code_upstream, "main"));
    assert!(code.join("news.txt").exists() && code.join("work.txt").exists());
    assert_eq!(
        std::fs::read_to_string(code.join("scratch.txt")).unwrap(),
        "unsaved\n"
    );
}

#[test]
fn workspace_cut_preserves_local_base_commits() {
    for sync_base in [true, false] {
        let mut fixture = fixture();
        fixture.update("source-1", json!({"sync_base": sync_base}));
        advance(&fixture.code, "local.txt");
        let local = head(&fixture.code, "main");
        git_in(&fixture.code, &["switch", "-q", "-c", "other"]);
        advance(&fixture.code, "other.txt");
        std::fs::write(fixture.code.join("README.md"), "unsaved\n").unwrap();
        let created = fixture.state.handle(req(
            "workspace.create",
            json!({"project_id": fixture.project_id, "name": "local", "isolation": "worktree"}),
        ));
        assert_eq!(created["ok"], true, "{created}");
        let checkout = checkout_of(&created["result"], "code");
        assert_eq!(head(&checkout, "HEAD"), local);
        assert!(checkout.join("local.txt").exists());
        assert!(!checkout.join("other.txt").exists());
        assert_eq!(
            std::fs::read_to_string(fixture.code.join("README.md")).unwrap(),
            "unsaved\n"
        );
        assert_eq!(head(&fixture.code, "main"), local);
        let warnings = created["result"]["warnings"].to_string();
        assert!(
            warnings.contains("1 local commit") && warnings.contains("included"),
            "{created}"
        );
    }
}

#[test]
fn moved_source_mutation_refuses_after_sync_lock_wait() {
    let fixture = fixture();
    std::fs::write(fixture.docs.join("work.txt"), "mine\n").unwrap();
    let scope = source_scope(&fixture.project_id, "source-2", &fixture.docs);
    let shared = fixture.state.shared();
    let (held, _) = SyncLock::acquire(&fixture.docs, Duration::ZERO).unwrap();
    let staging = frame_on_a_thread(
        &shared,
        "stage",
        "git.stage",
        with(&scope, json!({"paths": ["work.txt"]})),
    );
    assert!(staging.recv_timeout(STILL_WAITING).is_err());
    // Simulate a source record changing while the operation waits. Production
    // updates also take the source lock; the check must defend stale queued work.
    shared
        .lock()
        .unwrap()
        .projects
        .source_mut(&fixture.project_id, "source-2")
        .unwrap()
        .path = fixture.code.clone();
    drop(held);
    let reply = staging.recv_timeout(ANSWERED_WITHIN).unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error_code"], "conflict", "{reply}");
    assert!(
        crate::git_fixture::git_command(&fixture.docs, &["diff", "--cached", "--name-only"])
            .output()
            .unwrap()
            .stdout
            .is_empty()
    );
}

#[test]
fn sync_source_captured_path_survives_queue_and_lock_wait() {
    let fixture = fixture();
    let upstream = fixture.root.join("docs-upstream");
    advance(&upstream, "remote.txt");
    let before = head(&fixture.docs, "origin/main");
    let shared = fixture.state.shared();
    let stale = frame_on_a_thread(
        &shared,
        "stale",
        "project.sync_source",
        source_scope(&fixture.project_id, "source-2", &fixture.code),
    )
    .recv_timeout(ANSWERED_WITHIN)
    .unwrap();
    assert_eq!(stale["error_code"], "conflict", "{stale}");
    let asked = frame_on_a_thread(
        &shared,
        "sync",
        "project.sync_source",
        source_scope(&fixture.project_id, "source-2", &fixture.docs),
    )
    .recv_timeout(ANSWERED_WITHIN)
    .unwrap();
    assert_eq!(asked["ok"], true, "{asked}");
    let (held, _) = SyncLock::acquire(&fixture.docs, Duration::ZERO).unwrap();
    let state = Arc::clone(&shared);
    let syncing =
        std::thread::spawn(move || AppState::sync_sources(&state, SyncPass::Requested, 1_000));
    // Wait until the service has captured its subject and drained the queue.
    let deadline = std::time::Instant::now() + ANSWERED_WITHIN;
    loop {
        if shared.lock().unwrap().source_sync_requested.is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    shared
        .lock()
        .unwrap()
        .projects
        .source_mut(&fixture.project_id, "source-2")
        .unwrap()
        .path = fixture.code.clone();
    drop(held);
    syncing.join().unwrap();
    assert_eq!(
        head(&fixture.docs, "origin/main"),
        before,
        "stale sync must not even fetch the old source"
    );
    let state = shared.lock().unwrap();
    assert!(state.projects.get(&fixture.project_id).unwrap().sources[1]
        .sync_status
        .is_none());
}

#[test]
fn source_settings_wait_for_base_sync_without_holding_app_mutex() {
    let fixture = fixture();
    let shared = fixture.state.shared();
    let (held, _) = SyncLock::acquire(&fixture.docs, Duration::ZERO).unwrap();
    let update = frame_on_a_thread(
        &shared,
        "update",
        "project.update_source",
        json!({"project_id": fixture.project_id, "source_id": "source-2", "name": "renamed"}),
    );
    assert!(update.recv_timeout(STILL_WAITING).is_err());
    assert!(shared.try_lock().is_ok());
    drop(held);
    assert_eq!(update.recv_timeout(ANSWERED_WITHIN).unwrap()["ok"], true);
}

#[test]
fn manual_sync_does_not_follow_a_source_moved_before_queue_drain() {
    let mut fixture = fixture();
    let before = head(&fixture.docs, "origin/main");
    advance(&fixture.root.join("docs-upstream"), "remote.txt");
    let asked = fixture.state.handle(req(
        "project.sync_source",
        source_scope(&fixture.project_id, "source-2", &fixture.docs),
    ));
    assert_eq!(asked["ok"], true, "{asked}");
    fixture
        .state
        .projects
        .source_mut(&fixture.project_id, "source-2")
        .unwrap()
        .path = fixture.code.clone();
    let fixture = fixture.pass(SyncPass::Requested);
    assert_eq!(head(&fixture.docs, "origin/main"), before);
    assert!(fixture.row_source("source-2")["sync"].is_null());
}
