//! The workspace reclaim service and `workspace.reclaim` (#135), end to end
//! over a real project, workspace and tracker.

use super::project_agent::{rooted, workspace};
use super::tracker::{filed, tracked_with_origin};
use super::*;
use crate::reclaim::ReclaimPolicy;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Everything is idle at once: the threshold is zero. Pruning stays off, as
/// it is unless `BRIDGE_WORKSPACE_PRUNE` is set.
pub(super) fn impatient() -> ReclaimPolicy {
    ReclaimPolicy {
        idle_after: std::time::Duration::ZERO,
        ..ReclaimPolicy::default()
    }
}

/// [`impatient`], with pruning switched on.
pub(super) fn pruning() -> ReclaimPolicy {
    ReclaimPolicy {
        prune: true,
        ..impatient()
    }
}

pub(super) fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap()
}

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

/// A project over a repository with an origin, one workspace in it, and one
/// issue linked to that workspace: `(tempdir, state, project, workspace, issue)`.
pub(super) fn linked_workspace() -> (
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    String,
    String,
    String,
) {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let ws = workspace(&mut state, &project_id, "quiet");
    let issue = issue_id(&filed(&mut state, &project_id, "Quiet work"));
    let linked = state.handle(req(
        "issues.link",
        json!({ "issue_id": issue, "workspace_id": ws }),
    ));
    assert_eq!(linked["ok"], true, "{linked:?}");
    // Deterministic terminals: plain bash, whatever the machine's login shell.
    state.term_shell = "/bin/bash".into();
    (tmp, state.shared(), project_id, ws, issue)
}

pub(super) fn call(state: &Arc<Mutex<AppState>>, method: &str, params: Value) -> Value {
    state.lock().unwrap().handle(req(method, params))
}

pub(super) fn lifecycle(state: &Arc<Mutex<AppState>>, ws: &str) -> Value {
    let listed = call(state, "workspace.list", json!({}));
    listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["workspace_id"] == ws)
        .map(|row| row["lifecycle"].clone())
        .unwrap_or_else(|| panic!("no row for {ws}: {listed:?}"))
}

pub(super) fn timeline_kinds(state: &Arc<Mutex<AppState>>, issue: &str) -> Vec<Value> {
    call(state, "issues.get", json!({ "issue_id": issue }))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("workspace_"))
        })
        .cloned()
        .collect()
}

/// What Build said on the project agent's conversation.
fn project_agent_notices(state: &Arc<Mutex<AppState>>, project_id: &str) -> Vec<String> {
    let owner = call(
        state,
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    )["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agents = call(state, "agent.list", json!({ "entity_id": owner }));
    let Some(agent_id) = agents["result"]["agents"][0]["id"]
        .as_str()
        .map(str::to_string)
    else {
        return Vec::new();
    };
    let page = call(
        state,
        "thread.page",
        json!({ "entity_id": owner, "agent_id": agent_id, "limit": 50 }),
    );
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message" && item["data"]["from_build"] == true)
        .map(|item| {
            item["data"]["body"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Move the linked issue to Done.
pub(super) fn finish(state: &Arc<Mutex<AppState>>, issue: &str) {
    let moved = call(
        state,
        "issues.update",
        json!({ "issue_id": issue, "status": "done" }),
    );
    assert_eq!(moved["ok"], true, "{moved:?}");
}

/// The workspace's root, and its one checkout.
pub(super) fn root_and_checkout(state: &Arc<Mutex<AppState>>, ws: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(
        call(state, "workspace.get", json!({ "workspace_id": ws }))["result"]["root"]
            .as_str()
            .unwrap(),
    );
    let checkout = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.join(".git").exists())
        .expect("the workspace's checkout");
    (root, checkout)
}

/// Give the checkout build output its repository ignores, without committing
/// anything the remote does not have: `checkout/node_modules`.
pub(super) fn build_output_in(checkout: &Path) -> PathBuf {
    let dot_git = checkout.join(".git");
    let exclude = if dot_git.is_dir() {
        dot_git.join("info/exclude")
    } else {
        let gitdir = std::fs::read_to_string(&dot_git).unwrap();
        let common = PathBuf::from(gitdir.trim().trim_start_matches("gitdir: "));
        common.join("../../info/exclude")
    };
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "node_modules/\n").unwrap();
    let output = checkout.join("node_modules/pkg");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("index.js"), vec![1u8; 32 * 1024]).unwrap();
    checkout.join("node_modules")
}

fn assert_says_everything(notice: &str, phrases: &[&str]) {
    for phrase in phrases {
        assert!(notice.contains(phrase), "{phrase:?} missing from {notice}");
    }
}

/// Before any sweep there is no verdict, and the row says so with `null`.
#[test]
fn a_workspace_row_carries_no_verdict_before_the_first_sweep() {
    let (_tmp, state, _project, ws, _issue) = linked_workspace();
    assert_eq!(lifecycle(&state, &ws), Value::Null);
}

/// A quiet workspace whose issue is still open is announced, but held: the
/// row names the open issue, the project agent is told once, and the issue's
/// timeline records Build noticing.
#[test]
fn a_quiet_workspace_is_announced_to_the_project_agent_and_on_its_issue() {
    let (_tmp, state, project_id, ws, issue) = linked_workspace();

    AppState::sweep_workspaces(&state, &impatient(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["idle"], true, "{verdict:?}");
    assert_eq!(verdict["reclaimable"], false);
    assert_eq!(verdict["holds"], json!(["issue_open"]));
    assert_eq!(verdict["issues"][0]["number"], 1);
    assert!(verdict["noticed_at_ms"].is_i64(), "{verdict:?}");
    assert!(verdict["size_bytes"].as_u64().unwrap() > 0);

    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_says_everything(
        &notices[0],
        &[
            "1 workspace has had no activity",
            "quiet (",
            "#1 Quiet work (Backlog)",
            "an issue linked to it is not Done",
        ],
    );

    let entries = timeline_kinds(&state, &issue);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "workspace_idle");
    assert_eq!(entries[0]["actor"], json!({ "kind": "build" }));
    assert_eq!(entries[0]["payload"]["workspace_id"], ws.as_str());

    // The next sweep inside the idle period says nothing more.
    AppState::sweep_workspaces(&state, &ReclaimPolicy::default(), now_ms());
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    assert_eq!(timeline_kinds(&state, &issue).len(), 1);
}

/// Once its issue is Done and its work is pushed, a quiet workspace is
/// reclaimable. Its build output stays unless pruning is switched on.
#[test]
fn a_finished_quiet_workspace_is_reclaimable_and_keeps_its_build_output_by_default() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);

    AppState::sweep_workspaces(&state, &impatient(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["reclaimable"], true, "{verdict:?}");
    assert_eq!(verdict["pruned_bytes"], 0);
    assert!(output.exists(), "pruning is off unless it is switched on");
}

/// With pruning switched on, the build output of a finished quiet workspace
/// goes and its source stays, and the issue records it.
#[test]
fn with_pruning_on_a_finished_quiet_workspace_loses_its_build_output() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);

    AppState::sweep_workspaces(&state, &pruning(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["reclaimable"], true, "{verdict:?}");
    assert_eq!(verdict["holds"], json!([]));
    assert!(
        verdict["pruned_bytes"].as_u64().unwrap() >= 32 * 1024,
        "{verdict:?}"
    );
    assert!(!output.exists());
    assert!(checkout.join("README.md").exists(), "the source stays");
    assert_eq!(
        std::fs::read_dir(root.join(".build/reclaim"))
            .unwrap()
            .count(),
        0,
        "the trash is emptied"
    );
    assert!(!state.lock().unwrap().workspace_reserved(&ws), "released");
    let kinds: Vec<Value> = timeline_kinds(&state, &issue)
        .into_iter()
        .map(|entry| entry["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        json!(["workspace_pruned", "workspace_idle"])
            .as_array()
            .unwrap()
            .clone()
    );
}

/// Reclaim refuses in a sentence while anything holds the workspace, and
/// removes nothing: an open issue at once, uncommitted work once Git has been
/// measured off the lock.
#[test]
fn reclaim_refuses_while_an_issue_is_open_or_work_is_only_here() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    let (root, checkout) = root_and_checkout(&state, &ws);

    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "conflict");
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: an issue linked to it is not Done."
    );

    finish(&state, &issue);
    std::fs::write(checkout.join("unsaved.txt"), "mine\n").unwrap();
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: it has uncommitted changes."
    );
    assert!(root.is_dir(), "a refused reclaim removes nothing");
    assert!(
        !state.lock().unwrap().workspace_reserved(&ws),
        "a refusal ends the reservation"
    );
}

/// A reclaim removes the workspace, logs itself on the linked issue with who
/// did it, and leaves the issue where it was.
#[test]
fn reclaim_removes_the_workspace_and_logs_it_on_the_issue() {
    let (_tmp, state, _project, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let source = state
        .lock()
        .unwrap()
        .workspaces
        .get(&ws)
        .unwrap()
        .directories[0]
        .source_path
        .clone();
    let name = crate::isolation::checkout_name(&checkout).unwrap();
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&name)
        .is_ok());

    let reclaimed = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));

    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");
    assert_eq!(
        reclaimed["result"],
        json!({ "workspace_id": ws, "deleted": true })
    );
    assert!(!root.exists());
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&name)
        .is_err());
    let entries = timeline_kinds(&state, &issue);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "workspace_reclaimed");
    assert_eq!(entries[0]["actor"], json!({ "kind": "user" }));
    assert_eq!(entries[0]["payload"]["workspace_name"], "quiet");
    let read = call(&state, "issues.get", json!({ "issue_id": issue }));
    assert_eq!(
        read["result"]["issue"]["state"], "open",
        "reclaim closes nothing"
    );
}

/// The project agent reclaims through its own tool, and the issue names it.
#[test]
fn the_project_agent_reclaims_under_its_own_name() {
    let (_tmp, state, project_id, ws, issue) = linked_workspace();
    finish(&state, &issue);
    let (owner, agent_id) =
        super::project_agent::project_agent(&mut state.lock().unwrap(), &project_id);

    let reclaimed = state.lock().unwrap().agent_action(
        &owner,
        &agent_id,
        crate::mcp::BridgeAction::ReclaimWorkspace {
            workspace_id: ws.clone(),
        },
    );

    assert!(reclaimed.is_ok(), "{reclaimed:?}");
    let entries = timeline_kinds(&state, &issue);
    assert_eq!(entries[0]["actor"]["agent_id"], agent_id.as_str());
}

/// A verdict survives a restart, so the bridge does not announce every quiet
/// workspace again on the way back up.
#[test]
fn the_verdict_is_kept_across_a_restart() {
    let (tmp, state, _project, ws, _issue) = linked_workspace();
    AppState::sweep_workspaces(&state, &impatient(), now_ms());
    let before = lifecycle(&state, &ws);
    drop(state);

    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let reopened = rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let reopened = Arc::new(Mutex::new(reopened));
    assert_eq!(lifecycle(&reopened, &ws), before);
}
