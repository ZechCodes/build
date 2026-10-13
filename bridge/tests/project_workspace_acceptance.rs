//! Combined project-workspace acceptance (#538), over real RPC frames and
//! temporary sources only. No serving binary, provider CLI or user session.
//!
//! The journey crosses the original checkout, exact worker conversations,
//! source shells, an isolated child and source/project teardown. The source
//! matrix adds unborn/plain/secondary inputs and a Git mutation waiting for
//! base sync. One test owns PATH/HOME so these fixtures cannot race each other.

mod project_bridge;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use build_bridge::source_sync::{sync_base, Fetch, SyncLock, SyncOutcome};
use project_bridge::{git, ids, init_repo, private_path, purposes, scratch_of, Bridge, PATIENCE};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Project {
    id: String,
    owner: String,
    pin: String,
    primary: PathBuf,
    secondary: PathBuf,
    plain: PathBuf,
    upstream: PathBuf,
    sources: Vec<Value>,
}

fn configure(repo: &Path) {
    git(repo, &["config", "--local", "user.name", "Test"]);
    git(repo, &["config", "--local", "user.email", "test@build.ing"]);
    git(repo, &["config", "--local", "commit.gpgsign", "false"]);
}

fn head(repo: &Path) -> git2::Oid {
    git2::Repository::open(repo)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
}

fn source_scope(project: &str, source: &Value) -> Value {
    json!({ "project_id": project, "source_id": source["id"], "source_path": source["path"] })
}

fn with(scope: &Value, extra: Value) -> Value {
    let mut params = scope.clone();
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

fn sources(bridge: &Bridge, project: &str) -> Vec<Value> {
    bridge.ok("project.list", json!({}))["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == project)
        .unwrap()["sources"]
        .as_array()
        .unwrap()
        .clone()
}

fn make_project(bridge: &Bridge, root: &Path) -> Project {
    let primary = init_repo(root);
    let remote_root = root.join("remote-fixture");
    std::fs::create_dir(&remote_root).unwrap();
    let upstream = init_repo(&remote_root);
    git(root, &["clone", upstream.to_str().unwrap(), "secondary"]);
    let secondary = std::fs::canonicalize(root.join("secondary")).unwrap();
    configure(&secondary);
    let plain = root.join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "original notes\n").unwrap();
    let id = bridge.ok(
        "project.create",
        json!({ "name": "Acceptance", "sources": [
            { "path": primary, "name": "Code", "base_branch": "main" },
            { "path": secondary, "name": "Docs", "base_branch": "main" },
            { "path": plain, "name": "Notes" },
        ] }),
    )["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner = bridge.ensure(&id);
    let pin = bridge.roster(&owner, true)["pinned_agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    let sources = sources(bridge, &id);
    assert_eq!(sources.len(), 3);
    Project {
        id,
        owner,
        pin,
        primary,
        secondary,
        plain,
        upstream,
        sources,
    }
}

fn post(bridge: &Bridge, owner: &str, agent: &str, body: &str) {
    bridge.ok(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": agent, "body": body }),
    );
    bridge.wait_for_state(owner, agent, "live");
}

fn page(bridge: &Bridge, owner: &str, agent: &str) -> Value {
    bridge.ok(
        "thread.page",
        json!({ "entity_id": owner, "agent_id": agent, "limit": 100 }),
    )
}

fn worker(bridge: &Bridge, owner: &str, name: &str) -> String {
    bridge.ok(
        "agent.add",
        json!({ "entity_id": owner, "name": name, "project_role": "worker" }),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn commit_original_base(bridge: &Bridge, project: &Project) {
    let scope = source_scope(&project.id, &project.sources[0]);
    std::fs::write(project.primary.join("README.md"), "edited original base\n").unwrap();
    bridge.ok("git.stage", with(&scope, json!({ "paths": ["README.md"] })));
    let committed = bridge.ok(
        "git.commit",
        with(&scope, json!({ "message": "local base acceptance" })),
    );
    assert_eq!(committed["subject"], "local base acceptance");
    assert_eq!(
        git2::Repository::open(&project.primary)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some("main")
    );
    assert_eq!(
        std::fs::read_to_string(project.secondary.join("README.md")).unwrap(),
        "the project\n"
    );
}

fn assign_one_worker(bridge: &Bridge, project: &Project, sibling: &str) -> (String, Value) {
    let task = bridge.ok(
        "tasks.create",
        json!({ "project_id": project.id, "title": "Combined acceptance" }),
    )["task"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let request = json!({ "task_id": task, "assignee": {
        "kind": "new_project_agent", "agent_name": "Acceptance worker",
    }, "operation_id": "op-project-workspace-acceptance" });
    let first = bridge.ok("tasks.assign", request.clone());
    let again = bridge.ok("tasks.assign", request);
    assert_eq!(first["dispatch"], again["dispatch"]);
    let assigned = first["dispatch"]["agent_id"].as_str().unwrap().to_string();
    assert_eq!(first["dispatch"]["entity_id"], project.owner);
    assert_eq!(first["dispatch"]["workspace_id"], Value::Null);
    assert_eq!(first["task"]["links"]["workspace_ids"], json!([]));
    assert_eq!(
        first["task"]["links"]["conversation_ids"],
        json!([project.owner])
    );
    assert_eq!(
        ids(&bridge.roster(&project.owner, true)),
        [project.pin.clone(), sibling.into(), assigned.clone()]
    );
    bridge.wait_for_state(&project.owner, &assigned, "live");
    let history = page(bridge, &project.owner, &assigned);
    let delivered = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["data"]["operation_id"] == "op-project-workspace-acceptance")
        .count();
    assert_eq!(delivered, 1, "{history}");
    for untouched in [&project.pin, sibling] {
        let history = page(bridge, &project.owner, untouched);
        assert!(history["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["data"]["operation_id"] != "op-project-workspace-acceptance"));
    }
    bridge.refused(
        "tasks.assign",
        json!({ "task_id": task,
            "assignee": { "kind": "agent", "agent_id": sibling },
            "operation_id": "op-project-workspace-acceptance",
        }),
        "conflict",
    );
    (assigned, bridge.ok("tasks.get", json!({ "task_id": task })))
}

fn terminal_ids(list: &Value) -> Vec<String> {
    list["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["term_id"].as_str().unwrap().to_string())
        .collect()
}

fn open_shell(bridge: &Bridge, scope: &Value, folder: &Path) -> String {
    let term = bridge.ok("term.create", scope.clone())["term_id"]
        .as_str()
        .unwrap()
        .to_string();
    let attach = with(scope, json!({ "term_id": term }));
    bridge.ok("term.attach", attach.clone());
    bridge.ok(
        "term.input",
        json!({ "term_id": term,
        "data": STANDARD.encode(b"printf 'ACCEPTANCE-CWD:%s\\n' \"$PWD\"\r") }),
    );
    let expected = format!("ACCEPTANCE-CWD:{}", folder.display());
    let deadline = Instant::now() + PATIENCE;
    loop {
        let screen = bridge.ok("term.attach", attach.clone());
        let snapshot = STANDARD
            .decode(screen["snapshot"].as_str().unwrap())
            .unwrap();
        if String::from_utf8_lossy(&snapshot).contains(&expected) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shell never printed {expected}: {screen}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    term
}

fn shells_and_child(bridge: &Bridge, project: &Project) -> Vec<String> {
    let folders = [&project.primary, &project.secondary, &project.plain];
    let opened: Vec<_> = project
        .sources
        .iter()
        .zip(folders)
        .map(|(source, folder)| {
            let scope = source_scope(&project.id, source);
            assert!(
                terminal_ids(&bridge.ok("term.list", scope.clone())).is_empty(),
                "page load opens no shell"
            );
            open_shell(bridge, &scope, folder)
        })
        .collect();
    for (source, term) in project.sources.iter().zip(&opened) {
        assert_eq!(
            terminal_ids(&bridge.ok("term.list", source_scope(&project.id, source))),
            [term.clone()]
        );
    }
    assert_eq!(
        terminal_ids(&bridge.ok("term.list", json!({ "project_id": project.id }))),
        [opened[0].clone()]
    );
    bridge.refused(
        "term.attach",
        with(
            &source_scope(&project.id, &project.sources[0]),
            json!({ "term_id": opened[1] }),
        ),
        "not_found",
    );

    let child = bridge.ok(
        "workspace.create",
        json!({ "project_id": project.id, "name": "acceptance-child", "isolation": "worktree" }),
    );
    assert_eq!(child["status"], "ready", "{child}");
    let primary = PathBuf::from(child["directories"][0]["path"].as_str().unwrap());
    let plain = PathBuf::from(child["directories"][2]["path"].as_str().unwrap());
    assert_eq!(
        head(&primary),
        head(&project.primary),
        "the local committed base is the cut"
    );
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.txt")).unwrap(),
        "original notes\n"
    );
    std::fs::write(primary.join("README.md"), "child-only edit\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(project.primary.join("README.md")).unwrap(),
        "edited original base\n"
    );
    let child_owner = bridge.ok(
        "workspace.ensure_conversation",
        json!({ "workspace_id": child["workspace_id"] }),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let child_agents = bridge.roster(&child_owner, true);
    assert!(!ids(&child_agents)
        .iter()
        .any(|id| ids(&bridge.roster(&project.owner, true)).contains(id)));
    assert!(terminal_ids(&bridge.ok(
        "term.list",
        json!({ "workspace_id": child["workspace_id"], "source_id": "source-1" })
    ))
    .is_empty());
    assert_eq!(bridge.ensure(&project.id), project.owner);
    assert_eq!(
        terminal_ids(&bridge.ok("term.list", source_scope(&project.id, &project.sources[1]))),
        [opened[1].clone()]
    );
    let listed = bridge.ok("workspace.list", json!({ "project_id": project.id }));
    assert!(listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["workspace_id"] != project.owner));
    bridge.ok(
        "workspace.delete",
        json!({ "workspace_id": child["workspace_id"] }),
    );
    assert_eq!(
        bridge.roster(&project.owner, true)["pinned_agent_id"],
        project.pin
    );
    opened
}

fn removal_and_move(
    bridge: &Bridge,
    project: &Project,
    assigned: &str,
    sibling: &str,
    saved: &Value,
    shells: &[String],
    root: &Path,
) -> PathBuf {
    let moved = root.join("moved-secondary");
    git(
        root,
        &[
            "clone",
            project.secondary.to_str().unwrap(),
            "moved-secondary",
        ],
    );
    configure(&moved);
    let change = json!({ "project_id": project.id, "source_id": "source-2", "path": moved });
    let refused = bridge.refused("project.update_source", change.clone(), "conflict");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Acceptance worker"),
        "{refused}"
    );
    bridge.refused(
        "project.remove_source",
        json!({ "project_id": project.id, "source_id": "source-2" }),
        "conflict",
    );
    let sibling_before = page(bridge, &project.owner, sibling);
    let pin_before = page(bridge, &project.owner, &project.pin);
    bridge.ok(
        "agent.remove",
        json!({ "entity_id": project.owner, "agent_id": assigned }),
    );
    assert_eq!(
        page(bridge, &project.owner, sibling)["items"],
        sibling_before["items"]
    );
    assert_eq!(
        page(bridge, &project.owner, &project.pin)["items"],
        pin_before["items"]
    );
    let read = bridge.ok("tasks.get", json!({ "task_id": saved["task"]["id"] }));
    assert_eq!(read["task"]["assignee"], saved["task"]["assignee"]);
    assert_eq!(read["task"]["links"], saved["task"]["links"]);
    assert_eq!(read["task"]["identities"][assigned]["available"], false);
    assert_eq!(
        read["task"]["identities"][assigned]["project_role"],
        "worker"
    );
    assert_eq!(
        read["task"]["identities"][assigned]["name"],
        "Acceptance worker"
    );
    let dispatches = read["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "dispatched")
        .count();
    assert_eq!(dispatches, 1);
    bridge.refused(
        "thread.post",
        json!({ "entity_id": project.owner, "agent_id": assigned, "body": "stale worker" }),
        "not_found",
    );
    bridge.ok("conversation.reset", json!({ "project_id": project.id, "entity_id": project.owner,
        "agent_id": sibling, "conversation_id": sibling, "expected_thread_id": sibling_before["thread_id"] }));
    let refused = bridge.refused("project.update_source", change.clone(), "conflict");
    assert!(
        refused["error"].as_str().unwrap().contains("shell"),
        "{refused}"
    );
    bridge.ok(
        "term.close",
        with(
            &source_scope(&project.id, &project.sources[1]),
            json!({ "term_id": shells[1] }),
        ),
    );
    bridge.ok("project.update_source", change);
    for method in [
        "git.status",
        "term.create",
        "term.list",
        "project.sync_source",
    ] {
        bridge.refused(
            method,
            source_scope(&project.id, &project.sources[1]),
            "conflict",
        );
    }
    let fresh = sources(bridge, &project.id)[1].clone();
    let fresh_scope = source_scope(&project.id, &fresh);
    assert_eq!(
        bridge.ok("git.status", fresh_scope.clone())["path"],
        moved.display().to_string()
    );
    let new_shell = open_shell(bridge, &fresh_scope, &moved);
    assert_ne!(new_shell, shells[1]);
    assert_eq!(
        terminal_ids(&bridge.ok("term.list", source_scope(&project.id, &project.sources[0]))),
        [shells[0].clone()]
    );
    assert_eq!(
        bridge.roster(&project.owner, true)["pinned_agent_id"],
        project.pin
    );
    moved
}

fn delete_preserves_originals(bridge: &Bridge, project: &Project, root: &Path, moved: &Path) {
    let last = worker(bridge, &project.owner, "Deletion worker");
    post(bridge, &project.owner, &last, "a final live session");
    std::fs::write(project.primary.join("unsaved.txt"), "keep my work\n").unwrap();
    let scratch = scratch_of(root);
    let before = head(&project.primary);
    bridge.ok(
        "project.delete",
        json!({ "project_id": project.id, "confirm": true }),
    );
    assert!(bridge.ok("project.list", json!({}))["projects"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!scratch.exists());
    assert_eq!(head(&project.primary), before);
    assert_eq!(
        std::fs::read_to_string(project.primary.join("unsaved.txt")).unwrap(),
        "keep my work\n"
    );
    assert!(project.secondary.join(".git").is_dir());
    assert!(moved.join(".git").is_dir());
    assert_eq!(
        std::fs::read_to_string(project.plain.join("notes.txt")).unwrap(),
        "original notes\n"
    );
    bridge.refused("term.attach", json!({ "term_id": "term-1" }), "not_found");
    bridge.refused(
        "thread.post",
        json!({ "entity_id": project.owner, "agent_id": last, "body": "retired" }),
        "not_found",
    );
}

fn concurrent_secondary_sync(bridge: &Bridge, project: &Project) {
    std::fs::write(project.upstream.join("remote.txt"), "remote news\n").unwrap();
    git(&project.upstream, &["add", "remote.txt"]);
    git(&project.upstream, &["commit", "-m", "remote advance"]);
    std::fs::write(project.secondary.join("work.txt"), "worker work\n").unwrap();
    std::fs::write(
        project.secondary.join("scratch.txt"),
        "uncommitted scratch\n",
    )
    .unwrap();
    let scope = source_scope(&project.id, &project.sources[1]);
    let (held, _) = SyncLock::acquire(&project.secondary, Duration::from_secs(1)).unwrap();
    let staging =
        bridge.call_on_thread("git.stage", with(&scope, json!({ "paths": ["work.txt"] })));
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !staging.is_finished(),
        "stage must wait behind the sync lock"
    );
    assert_eq!(
        bridge.ok("git.status", scope.clone())["path"],
        project.secondary.display().to_string()
    );
    assert_eq!(
        bridge.roster(&project.owner, true)["pinned_agent_id"],
        project.pin
    );
    let report = sync_base(
        &project.secondary,
        "main",
        Fetch::Within(Duration::from_secs(10)),
    );
    assert_eq!(
        report.outcome,
        SyncOutcome::FastForwarded { commits: 1 },
        "{report:?}"
    );
    drop(held);
    let staged = staging.join().unwrap();
    assert_eq!(staged["ok"], true, "{staged}");
    bridge.ok(
        "git.commit",
        with(&scope, json!({ "message": "worker after sync" })),
    );
    let repo = git2::Repository::open(&project.secondary).unwrap();
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_id(0)
            .unwrap(),
        head(&project.upstream)
    );
    assert_eq!(
        std::fs::read_to_string(project.secondary.join("scratch.txt")).unwrap(),
        "uncommitted scratch\n"
    );
    let local = head(&project.secondary);
    let report = sync_base(&project.secondary, "main", Fetch::Skip);
    assert!(
        matches!(report.outcome, SyncOutcome::Skipped(_)),
        "{report:?}"
    );
    assert_eq!(
        head(&project.secondary),
        local,
        "sync does not discard a worker's local commit"
    );
}

fn unborn_and_plain_matrix(bridge: &Bridge, root: &Path) {
    let unborn = root.join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "-b", "main"]);
    configure(&unborn);
    let plain = root.join("unborn-notes");
    std::fs::create_dir(&plain).unwrap();
    let project = bridge.ok(
        "project.create",
        json!({ "name": "Unborn matrix", "sources": [
        { "path": unborn, "base_branch": "main" }, { "path": plain },
    ] }),
    )["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let rows = sources(bridge, &project);
    let scope = source_scope(&project, &rows[0]);
    assert_eq!(rows[0]["is_git"], true);
    assert_eq!(rows[1]["is_git"], false);
    assert_eq!(bridge.ok("git.log", scope.clone())["commits"], json!([]));
    bridge.refused(
        "git.status",
        source_scope(&project, &rows[1]),
        "invalid_params",
    );
    bridge.refused(
        "project.sync_source",
        source_scope(&project, &rows[1]),
        "invalid_params",
    );
    assert!(!plain.join(".git").exists());
    std::fs::write(unborn.join("first.txt"), "first commit\n").unwrap();
    bridge.ok("git.stage", with(&scope, json!({ "paths": ["first.txt"] })));
    bridge.ok(
        "git.commit",
        with(&scope, json!({ "message": "first unborn commit" })),
    );
    assert_eq!(
        bridge.ok("git.log", scope)["commits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let child = bridge.ok(
        "workspace.create",
        json!({ "project_id": project, "name": "born-child", "isolation": "worktree" }),
    );
    assert_eq!(child["status"], "ready", "{child}");
    assert_eq!(
        head(Path::new(child["directories"][0]["path"].as_str().unwrap())),
        head(&unborn)
    );
    bridge.ok(
        "workspace.delete",
        json!({ "workspace_id": child["workspace_id"] }),
    );
    bridge.ok(
        "project.delete",
        json!({ "project_id": project, "confirm": true }),
    );
    assert_eq!(
        std::fs::read_to_string(unborn.join("first.txt")).unwrap(),
        "first commit\n"
    );
    assert!(!plain.join(".git").exists());
}

#[test]
fn project_workspace_combined_journey_and_source_matrix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", private_path(&root));
    std::env::set_var("SHELL", "/bin/sh");
    let bridge = Bridge::boot(&root);
    let project = make_project(&bridge, &root);
    let greeting = bridge.ok("session.hello", json!({}));
    for feature in [
        "git.projectSources",
        "term.projectSources",
        "agents.projectWorkers",
        "tasks.assignProjectAgent",
    ] {
        assert!(
            greeting["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(feature)),
            "{feature}"
        );
    }
    commit_original_base(&bridge, &project);
    let sibling = worker(&bridge, &project.owner, "Sibling");
    post(
        &bridge,
        &project.owner,
        &project.pin,
        "pin's retained history",
    );
    post(
        &bridge,
        &project.owner,
        &sibling,
        "sibling's retained history",
    );
    let (assigned, task) = assign_one_worker(&bridge, &project, &sibling);
    assert_eq!(
        ids(&bridge.roster(&project.owner, false)),
        [project.pin.clone()]
    );
    assert_eq!(
        purposes(&bridge.roster(&project.owner, true)),
        ["coordinator", "worker", "worker"]
    );
    bridge.refused(
        "agent.add",
        json!({ "entity_id": project.owner, "project_role": "coordinator" }),
        "invalid_params",
    );
    bridge.refused(
        "agent.remove",
        json!({ "entity_id": project.owner, "agent_id": project.pin }),
        "conflict",
    );
    concurrent_secondary_sync(&bridge, &project);
    let shells = shells_and_child(&bridge, &project);
    let moved = removal_and_move(
        &bridge, &project, &assigned, &sibling, &task, &shells, &root,
    );
    delete_preserves_originals(&bridge, &project, &root, &moved);
    unborn_and_plain_matrix(&bridge, &root);
    bridge.shutdown();
}
