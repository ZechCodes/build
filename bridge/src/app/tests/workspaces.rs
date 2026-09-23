use super::*;

mod adopted_checkouts;
mod conversation_git;
mod directories;
mod git_init_deferred;
mod pending_adoption;

fn app(root: &Path) -> AppState {
    let root = std::fs::canonicalize(root).unwrap();
    AppState::new_unrooted(root.join("worktrees"), "main", true, "/tmp/test-mcp.sock")
}

fn repo_with_origin(parent: &Path, name: &str) -> (PathBuf, PathBuf) {
    let repo = init_repo_named(parent, name);
    let origin = parent.join(format!("{name}.git"));
    git_in(
        parent,
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&repo, &["fetch", "origin"]);
    git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
    (repo, origin)
}

fn create_mixed_project(state: &mut AppState, git: &Path, plain: &Path) -> String {
    let created = state.handle(req(
        "project.create",
        json!({
            "name": "mixed",
            "sources": [
                {"name": "api", "path": git},
                {"name": "assets", "path": plain},
            ]
        }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn create_workspace(state: &mut AppState, project_id: &str, name: &str) -> Value {
    let created = state.handle(req(
        "workspace.create",
        json!({"project_id": project_id, "name": name, "isolation": "worktree"}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"].clone()
}

#[test]
fn a_workspace_created_by_the_user_is_not_marked_agent_created() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project_id = state.add_project(repo, "main".into());
    let created = create_workspace(&mut state, &project_id, "manual");
    assert_eq!(created["created_by_agent"], false, "{created:?}");
    let listed = state.handle(req("workspace.list", json!({ "project_id": project_id })));
    let workspace_id = created["workspace_id"].as_str().unwrap();
    let workspace = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["workspace_id"] == workspace_id)
        .unwrap();
    assert_eq!(workspace["created_by_agent"], false, "{listed:?}");

    // Older manifests have no creator field. They remain user-visible.
    let manifest =
        Path::new(created["root"].as_str().unwrap()).join(crate::workspace::MANIFEST_FILE);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    saved.as_object_mut().unwrap().remove("created_by_agent");
    std::fs::write(&manifest, serde_json::to_vec(&saved).unwrap()).unwrap();
    state.workspaces.reload().unwrap();
    assert!(!state.workspaces.get(workspace_id).unwrap().created_by_agent);
}

fn directory<'a>(workspace: &'a Value, source_id: &str) -> &'a Value {
    workspace["directories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|directory| directory["source_id"] == source_id)
        .unwrap_or_else(|| panic!("source {source_id} is present: {workspace:?}"))
}

#[test]
fn git_init_can_target_workspace_copy_without_mutating_its_source() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("kept.txt"), "keep me\n").unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": plain})));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace_id, "source_id": "source-1", "target": "workspace"}),
    ));

    assert_eq!(initialized["ok"], true, "{initialized:?}");
    assert_eq!(initialized["result"]["results"][0]["status"], "initialized");
    assert_eq!(
        initialized["result"]["workspace"]["directories"][0]["is_git"], true,
        "{initialized:?}"
    );
    assert_eq!(initialized["result"]["source"]["is_git"], false);
    let copy = Path::new(
        initialized["result"]["workspace"]["directories"][0]["path"]
            .as_str()
            .unwrap(),
    );
    assert!(copy.join(".git").is_dir());
    assert_eq!(
        std::fs::read_to_string(copy.join("kept.txt")).unwrap(),
        "keep me\n"
    );
    assert!(!plain.join(".git").exists());
}

#[test]
fn git_init_source_refreshes_an_adopted_same_path_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": plain})));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    // The project's own checkout is never listed (it is not a place to work),
    // but the verb still reaches it by its id.
    let adopted = state.handle(req(
        "workspace.get",
        json!({"workspace_id": format!("legacy-{project_id}")}),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let adopted = &adopted["result"];

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": adopted["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(
        initialized["result"]["results"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        initialized["result"]["source"]["is_git"], true,
        "{initialized:?}"
    );
    assert_eq!(
        initialized["result"]["workspace"]["directories"][0]["is_git"],
        true
    );
}

#[test]
fn git_init_source_updates_only_the_exact_project_source() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let mut state = app(tmp.path());
    let project_id = create_mixed_project(&mut state, &first, &second);
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace_id, "source_id": "source-2", "target": "source"}),
    ));

    assert_eq!(initialized["ok"], true, "{initialized:?}");
    assert!(!first.join(".git").exists());
    assert!(second.join(".git").is_dir());
    assert_eq!(initialized["result"]["source"]["id"], "source-2");
    assert_eq!(initialized["result"]["source"]["is_git"], true);
    assert_eq!(
        initialized["result"]["workspace"]["directories"][1]["is_git"],
        false
    );
}

#[test]
fn git_init_reconciles_an_existing_repository_and_preserves_its_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": plain})));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    git_in(&plain, &["init", "-b", "develop"]);
    git_in(&plain, &["config", "user.name", "Build Test"]);
    git_in(&plain, &["config", "user.email", "test@build.invalid"]);
    git_in(&plain, &["commit", "--allow-empty", "-m", "existing"]);
    let before = git2::Repository::open(&plain)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace_id, "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(
        initialized["result"]["results"][0]["status"],
        "already_initialized"
    );
    assert_eq!(
        git2::Repository::open(&plain)
            .unwrap()
            .head()
            .unwrap()
            .target(),
        Some(before)
    );
    assert_eq!(initialized["result"]["source"]["base_branch"], "develop");
}

#[test]
fn git_init_rejects_a_nul_branch_without_writing_git_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req(
        "project.add",
        json!({"path": plain, "base_branch": "bad\u{0}branch"}),
    ));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "work");

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(initialized["ok"], true, "{initialized:?}");
    assert_eq!(initialized["result"]["results"][0]["status"], "failed");
    assert!(!plain.join(".git").exists());
}

#[cfg(unix)]
#[test]
fn git_init_rejects_a_symlink_swapped_workspace_directory() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let replacement = tmp.path().join("replacement");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&replacement).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let workspace_path = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let original = workspace_path.with_extension("original");
    std::fs::rename(&workspace_path, &original).unwrap();
    symlink(&replacement, &workspace_path).unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "workspace"}),
    ));

    assert_eq!(initialized["result"]["results"][0]["status"], "failed");
    assert!(initialized["result"]["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("not a directory"));
    assert!(!replacement.join(".git").exists());
    assert!(!original.join(".git").exists());
}

#[cfg(unix)]
#[test]
fn git_init_rejects_a_symlink_swapped_project_source() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let replacement = tmp.path().join("replacement");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&replacement).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let original = source.with_extension("original");
    std::fs::rename(&source, &original).unwrap();
    symlink(&replacement, &source).unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(initialized["result"]["results"][0]["status"], "failed");
    assert!(initialized["result"]["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("no longer matches"));
    assert!(!replacement.join(".git").exists());
    assert!(!original.join(".git").exists());
}

#[cfg(unix)]
#[test]
fn git_init_refuses_a_dangling_git_marker_without_replacing_it() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    symlink("missing-git-dir", source.join(".git")).unwrap();

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(initialized["result"]["results"][0]["status"], "failed");
    assert!(initialized["result"]["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("symlink"));
    assert_eq!(
        std::fs::read_link(source.join(".git")).unwrap(),
        PathBuf::from("missing-git-dir")
    );
}

#[cfg(unix)]
#[test]
fn git_init_both_reports_partial_success_and_retries_without_duplicate_initialization() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    symlink("missing-git-dir", source.join(".git")).unwrap();
    let params = json!({
        "workspace_id": workspace["workspace_id"],
        "source_id": "source-1",
        "target": "both",
    });

    let partial = state.handle(req("workspace.init_git", params.clone()));
    assert_eq!(partial["result"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(partial["result"]["results"][0]["target"], "workspace");
    assert_eq!(partial["result"]["results"][0]["status"], "initialized");
    assert_eq!(partial["result"]["results"][1]["target"], "source");
    assert_eq!(partial["result"]["results"][1]["status"], "failed");

    std::fs::remove_file(source.join(".git")).unwrap();
    let retried = state.handle(req("workspace.init_git", params));
    assert_eq!(retried["result"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        retried["result"]["results"][0]["status"],
        "already_initialized"
    );
    assert_eq!(retried["result"]["results"][1]["status"], "initialized");
    assert_eq!(
        retried["result"]["workspace"]["directories"][0]["is_git"],
        true
    );
    assert_eq!(retried["result"]["source"]["is_git"], true);
}

#[test]
fn git_init_preserves_an_unborn_repository_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    git_in(&source, &["init", "-b", "develop"]);

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(initialized["result"]["results"][0]["status"], "initialized");
    let repository = git2::Repository::open(&source).unwrap();
    assert_eq!(repository.head().unwrap().shorthand(), Some("develop"));
    assert!(repository.head().unwrap().target().is_some());
    assert_eq!(initialized["result"]["source"]["base_branch"], "develop");
}

#[test]
fn git_init_uses_a_resolvable_configured_base_without_moving_head() {
    let tmp = tempfile::tempdir().unwrap();
    let source = init_repo_named(tmp.path(), "source");
    git_in(&source, &["branch", "release"]);
    git_in(&source, &["switch", "-c", "develop"]);
    git_in(&source, &["commit", "--allow-empty", "-m", "develop"]);
    let before = git2::Repository::open(&source)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req(
        "project.add",
        json!({"path": source, "base_branch": "release"}),
    ));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );

    let initialized = state.handle(req(
        "workspace.init_git",
        json!({"workspace_id": workspace["workspace_id"], "source_id": "source-1", "target": "source"}),
    ));

    assert_eq!(
        initialized["result"]["results"][0]["status"],
        "already_initialized"
    );
    let repository = git2::Repository::open(&source).unwrap();
    assert_eq!(repository.head().unwrap().shorthand(), Some("develop"));
    assert_eq!(repository.head().unwrap().target(), Some(before));
    assert_eq!(initialized["result"]["source"]["base_branch"], "release");
}

#[test]
fn mixed_project_workspace_isolates_git_and_plain_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "api");
    let plain = tmp.path().join("assets");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("logo.txt"), "original\n").unwrap();
    let mut state = app(tmp.path());
    let project_id = create_mixed_project(&mut state, &repo, &plain);
    let workspace = create_workspace(&mut state, &project_id, "feature");

    assert_eq!(workspace["status"], "ready", "{workspace:?}");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let git_dir = directory(&workspace, "source-1");
    let plain_dir = directory(&workspace, "source-2");
    assert_ne!(git_dir["path"], repo.display().to_string());
    assert_ne!(plain_dir["path"], plain.display().to_string());

    let opened = state.handle(req(
        "fs.read",
        json!({"workspace_id": workspace_id, "source_id": "source-2", "path": "logo.txt"}),
    ));
    let written = state.handle(req(
        "fs.write",
        json!({
            "workspace_id": workspace_id,
            "source_id": "source-2",
            "path": "logo.txt",
            "expected_revision": opened["result"]["revision"],
            "content_b64": "d29ya3NwYWNlCg==",
        }),
    ));
    assert_eq!(written["ok"], true, "{written:?}");
    assert_eq!(
        std::fs::read_to_string(plain.join("logo.txt")).unwrap(),
        "original\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "# project\n"
    );

    let refs = state.handle(req(
        "git.refs",
        json!({"workspace_id": workspace_id, "source_id": "source-1"}),
    ));
    assert_eq!(refs["ok"], true, "{refs:?}");
    let refused = state.handle(req(
        "git.refs",
        json!({"workspace_id": workspace_id, "source_id": "source-2"}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("not a git repository"));

    assert_workspace_has_no_agents(&mut state, workspace_id);
}

#[test]
fn workspace_conversation_owns_the_exact_multi_source_root_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "api");
    let plain = tmp.path().join("assets");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("logo.txt"), "asset\n").unwrap();
    let mut state = app(tmp.path());
    let project_id = create_mixed_project(&mut state, &repo, &plain);
    let workspace = create_workspace(&mut state, &project_id, "conversation");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();

    let first = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    let second = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(second["result"], first["result"], "{second:?}");
    let run_id = first["result"]["run_id"].as_str().unwrap();
    assert_eq!(state.runs.len(), 1);
    assert_eq!(
        state.runs[run_id].worktree.path,
        PathBuf::from(workspace["root"].as_str().unwrap())
    );
    assert_ne!(
        state.runs[run_id].worktree.path,
        PathBuf::from(directory(&workspace, "source-1")["path"].as_str().unwrap())
    );
    assert_ne!(
        state.runs[run_id].worktree.path,
        PathBuf::from(directory(&workspace, "source-2")["path"].as_str().unwrap())
    );

    let detail = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(detail["result"]["entity_id"], run_id);
    assert_eq!(detail["result"]["agents"], json!([]));
    let added = state.handle(req("agent.add", json!({"entity_id": run_id})));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(
        state.entity_agent_root(run_id).unwrap(),
        AppState::canonical_root(Path::new(workspace["root"].as_str().unwrap()))
    );
}

#[test]
fn workspace_conversation_survives_restart_without_adding_a_legacy_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "api");
    let plain = tmp.path().join("assets");
    std::fs::create_dir(&plain).unwrap();
    let config = tmp.path().join("config.json");
    let worktrees = tmp.path().join("worktrees");
    let context =
        || HarnessContext::resolved(tmp.path().join("mcp.sock"), tmp.path().to_path_buf()).unwrap();
    let (workspace_id, run_id, project_id) = {
        let mut state = AppState::new_unrooted_configured(&worktrees, "main", true, context())
            .with_config(&config)
            .unwrap()
            .with_task_store(tmp.path().join("store"))
            .unwrap();
        let project_id = create_mixed_project(&mut state, &repo, &plain);
        let workspace = create_workspace(&mut state, &project_id, "durable-chat");
        let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
        let ensured = state.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id": workspace_id}),
        ));
        assert_eq!(ensured["ok"], true, "{ensured:?}");
        (
            workspace_id,
            ensured["result"]["run_id"].as_str().unwrap().to_string(),
            project_id,
        )
    };

    let mut restarted = AppState::new_unrooted_configured(&worktrees, "main", true, context())
        .with_config(&config)
        .unwrap()
        .with_task_store(tmp.path().join("store"))
        .unwrap();
    let detail = restarted.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(detail["result"]["run_id"], run_id, "{detail:?}");
    let ensured = restarted.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(ensured["result"]["run_id"], run_id, "{ensured:?}");
    let listed = restarted.handle(req("workspace.list", json!({"project_id": project_id})));
    let root = detail["result"]["root"].as_str().unwrap();
    let same_root = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|workspace| workspace["root"] == root)
        .collect::<Vec<_>>();
    assert_eq!(same_root.len(), 1, "{listed:?}");
    assert_eq!(same_root[0]["workspace_id"], workspace_id);
    assert_eq!(same_root[0]["directories"].as_array().unwrap().len(), 2);
    assert!(restarted.workspaces.get(&workspace_id).unwrap().managed);
    assert_eq!(restarted.runs.len(), 1);
}

#[test]
fn workspace_conversation_persistence_failure_leaves_no_owner_and_can_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let context =
        HarnessContext::resolved(tmp.path().join("mcp.sock"), tmp.path().to_path_buf()).unwrap();
    let mut state =
        AppState::new_unrooted_configured(tmp.path().join("worktrees"), "main", true, context)
            .with_task_store(tmp.path().join("store"))
            .unwrap();
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "retry-chat");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    state.store.as_ref().unwrap().fail_next_write();

    let failed = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .contains("injected store failure"));
    assert!(state.runs.is_empty());
    let unowned = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(unowned["result"]["entity_id"], Value::Null, "{unowned:?}");

    let retried = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(retried["ok"], true, "{retried:?}");
    assert_eq!(state.runs.len(), 1);
}

/// Done removed the workspace, so the conversation verb has no workspace to
/// answer for rather than a finished one to refuse.
#[test]
fn a_workspace_done_removed_has_no_conversation_to_ensure() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "finished");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let finished = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");

    let refused = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("unknown workspace_id"));
    assert!(state.runs.is_empty());
}

#[test]
fn legacy_run_finish_routes_a_workspace_conversation_owner_to_its_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "finish-by-owner");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap();

    let finished = state.handle(req("run.finish", json!({"run_id": run_id})));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["complete"], true, "{finished:?}");
    assert_eq!(finished["result"]["deleted"], true, "{finished:?}");
    let detail = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(detail["ok"], false, "{detail:?}");
}

#[test]
fn workspace_list_never_carries_the_projects_own_checkout() {
    let (tmp, repo) = init_repo();
    let canonical = std::fs::canonicalize(&repo).unwrap();
    let mut state = qa_state(&repo, tmp.path());
    let project_id = state.project_at(0).id.clone();
    let workspace = create_workspace(&mut state, &project_id, "real-work");

    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let ids: Vec<_> = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![workspace["workspace_id"].as_str().unwrap()],
        "{listed:?}"
    );

    // A store written before workspaces can hold a run whose checkout is the
    // project's own repository; that run is not a listed workspace either —
    // the root is still the project's own checkout.
    let run_id = adopted_run(&mut state, &repo, tmp.path(), "was-a-worktree");
    state.runs.get_mut(&run_id).unwrap().worktree.path = AppState::canonical_root(&repo);
    let relisted = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert!(
        !relisted["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["root"] == canonical.display().to_string()),
        "{relisted:?}"
    );
    let unscoped = state.handle(req("workspace.list", json!({})));
    assert!(
        !unscoped["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["root"] == canonical.display().to_string()),
        "{unscoped:?}"
    );
}

#[test]
fn a_legacy_repo_root_run_still_has_its_run_id_workspace_route() {
    let (tmp, repo) = init_repo();
    let mut state = qa_state(&repo, tmp.path());
    let run_id = adopted_run(&mut state, &repo, tmp.path(), "was-a-worktree");
    // What a store written before workspaces holds: a run whose checkout is
    // the project's own repository.
    state.runs.get_mut(&run_id).unwrap().worktree.path = AppState::canonical_root(&repo);

    let detail = state.handle(req("workspace.get", json!({"workspace_id": run_id})));
    assert_eq!(detail["ok"], true, "{detail:?}");
    assert_eq!(detail["result"]["entity_id"], run_id, "{detail:?}");
}

fn assert_workspace_has_no_agents(state: &mut AppState, workspace_id: &str) {
    let detail = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(detail["ok"], true, "{detail:?}");
    assert_eq!(detail["result"]["entity_id"], Value::Null, "{detail:?}");
    assert_eq!(detail["result"]["run_id"], Value::Null, "{detail:?}");
    assert_eq!(detail["result"]["agents"], json!([]), "{detail:?}");
    assert_eq!(detail["result"]["thread"], Value::Null, "{detail:?}");
    assert_eq!(detail["result"]["run"], Value::Null, "{detail:?}");

    let named = state.handle(req(
        "workspace.get",
        json!({"workspace_id": workspace_id, "agent_id": "agent-NOSUCHTHING"}),
    ));
    assert_eq!(named["ok"], false, "{named:?}");
    assert!(named["error"]
        .as_str()
        .unwrap()
        .contains("unknown agent_id"));
}

#[test]
fn project_add_sources_also_materializes_a_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "backend");
    let docs = tmp.path().join("docs");
    std::fs::create_dir(&docs).unwrap();
    std::fs::write(docs.join("guide.md"), "guide\n").unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req(
        "project.add",
        json!({"name": "product", "sources": [{"path": repo}, {"path": docs}]}),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "work");
    assert_eq!(workspace["directories"].as_array().unwrap().len(), 2);
    assert!(Path::new(workspace["root"].as_str().unwrap()).is_dir());
}

#[test]
fn checkout_is_scoped_to_one_repository_and_refuses_to_overwrite_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let first = init_repo_named(tmp.path(), "first");
    let second = init_repo_named(tmp.path(), "second");
    git_in(&first, &["checkout", "-b", "other"]);
    std::fs::write(first.join("README.md"), "other branch\n").unwrap();
    git_in(&first, &["add", "README.md"]);
    git_in(&first, &["commit", "-m", "change other"]);
    git_in(&first, &["checkout", "main"]);
    git_in(&first, &["branch", "clean"]);
    git_in(&second, &["branch", "other"]);
    let mut state = app(tmp.path());
    let created = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": first}, {"path": second}]}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let project_id = created["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "switching");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let sibling_path = PathBuf::from(directory(&workspace, "source-2")["path"].as_str().unwrap());
    let sibling_before = Command::new("git")
        .args([
            "-C",
            sibling_path.to_str().unwrap(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .unwrap()
        .stdout;

    let switched = state.handle(req(
        "git.checkout_ref",
        json!({"workspace_id": workspace_id, "source_id": "source-1", "full_ref": "refs/heads/clean"}),
    ));
    assert_eq!(switched["ok"], true, "{switched:?}");
    let sibling_after = Command::new("git")
        .args([
            "-C",
            sibling_path.to_str().unwrap(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(sibling_after, sibling_before);

    let selected_path = PathBuf::from(directory(&workspace, "source-1")["path"].as_str().unwrap());
    let workspace_branch = directory(&workspace, "source-1")["branch"]
        .as_str()
        .unwrap();
    git_in(&selected_path, &["checkout", workspace_branch]);
    std::fs::write(selected_path.join("README.md"), "uncommitted\n").unwrap();
    let refused = state.handle(req(
        "git.checkout_ref",
        json!({"workspace_id": workspace_id, "source_id": "source-1", "full_ref": "refs/heads/other"}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        std::fs::read_to_string(selected_path.join("README.md")).unwrap(),
        "uncommitted\n"
    );
    let branch = Command::new("git")
        .args([
            "-C",
            selected_path.to_str().unwrap(),
            "branch",
            "--show-current",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        workspace_branch
    );
}

/// Done removes the workspace: the files, the record, and the conversation
/// that stood in it. What is left is the record of what was finished, which
/// the archive reads and a restart still finds.
#[test]
fn done_removes_the_workspace_and_keeps_the_record_of_what_it_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let (one, _) = repo_with_origin(tmp.path(), "one");
    let (two, _) = repo_with_origin(tmp.path(), "two");
    let worktrees = tmp.path().join("worktrees");
    let config = tmp.path().join("config.json");
    let (workspace_id, checkouts) = {
        let mut state = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
            .with_config(&config)
            .unwrap();
        let project = state.handle(req(
            "project.create",
            json!({"name": "pair", "sources": [{"path": one}, {"path": two}]}),
        ));
        let workspace = create_workspace(
            &mut state,
            project["result"]["project_id"].as_str().unwrap(),
            "done",
        );
        let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
        let root = PathBuf::from(workspace["root"].as_str().unwrap());
        let checkouts = workspace["directories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|directory| PathBuf::from(directory["path"].as_str().unwrap()))
            .collect::<Vec<_>>();
        let ensured = state.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id": workspace_id}),
        ));
        let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();

        let finished = state.handle(req(
            "workspace.finish",
            json!({"workspace_id": workspace_id}),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert_eq!(finished["result"]["complete"], true, "{finished:?}");
        assert_eq!(finished["result"]["deleted"], true, "{finished:?}");
        assert_eq!(
            finished["result"]["repositories"].as_array().unwrap().len(),
            2
        );

        assert!(!root.exists(), "Done removes the workspace directory");
        assert!(
            state.workspaces.get(&workspace_id).is_none(),
            "Done drops the live record"
        );
        assert!(!state.runs.contains_key(&run_id), "and its conversation");
        let gone = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
        assert_eq!(gone["ok"], false, "{gone:?}");
        assert_archived_workspace_is_history(&mut state, &workspace_id);
        (workspace_id, checkouts)
    };

    // Each source repository was handed its checkout back rather than left
    // holding a record of a directory that is gone.
    for checkout in &checkouts {
        assert!(!checkout.exists(), "{}", checkout.display());
    }
    let mut restarted = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
        .with_config(&config)
        .unwrap();
    let listed = restarted.handle(req("workspace.list", json!({})));
    assert!(
        listed["result"]["workspaces"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{listed:?}"
    );
    assert_archived_workspace_is_history(&mut restarted, &workspace_id);
}

/// A workspace Done removed is history: the archive names it and says when it
/// was finished, and there is nothing behind the record to open.
fn assert_archived_workspace_is_history(state: &mut AppState, workspace_id: &str) {
    let archived = state.handle(req("archived.list", json!({})));
    let record = archived["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["workspace_id"] == workspace_id)
        .unwrap_or_else(|| panic!("a finished workspace is in the archive: {archived:?}"));
    assert_eq!(record["kind"], "workspace");
    assert!(record["finished_at"].as_str().is_some(), "{record:?}");
}

/// Done waits for every source, not just the first one, and says which of them
/// is in the way. Once the work is pushed, the same call removes the workspace.
#[test]
fn done_waits_until_every_source_has_put_its_work_somewhere_else() {
    let tmp = tempfile::tempdir().unwrap();
    let (one, _) = repo_with_origin(tmp.path(), "one");
    let (two, _) = repo_with_origin(tmp.path(), "two");
    let mut state = app(tmp.path());
    let project = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": one}, {"path": two}]}),
    ));
    let workspace = create_workspace(
        &mut state,
        project["result"]["project_id"].as_str().unwrap(),
        "retry",
    );
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let dirty = PathBuf::from(directory(&workspace, "source-2")["path"].as_str().unwrap());
    std::fs::write(dirty.join("README.md"), "unfinished\n").unwrap();

    let refused = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("uncommitted changes"),
        "{refused:?}"
    );
    assert!(root.is_dir(), "a refused Done removes nothing");

    git_in(&dirty, &["add", "README.md"]);
    git_in(&dirty, &["commit", "-m", "finish work"]);
    let still_refused = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(still_refused["ok"], false, "{still_refused:?}");
    assert!(
        still_refused["error"]
            .as_str()
            .unwrap()
            .contains("no remote has"),
        "{still_refused:?}"
    );

    git_in(&dirty, &["push", "origin", "HEAD"]);
    let finished = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(!root.exists(), "{finished:?}");
}

/// Done refused, in the words the caller is given.
fn assert_done_refused(state: &mut AppState, workspace_id: &str, because: &str) {
    let refused = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"].as_str().unwrap().contains(because),
        "{refused:?}"
    );
}

/// A second workspace, in its own project, with its own agent mid-turn. What
/// Done must leave alone while it takes the workspace it was asked about.
fn neighbor_workspace_working_in_it(
    state: &mut AppState,
    parent: &Path,
    log: &SessionLog,
) -> TabKey {
    let (repo, _) = repo_with_origin(parent, "neighbor-repo");
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = create_workspace(state, &project_id, "neighbor");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    let run_id = owner["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({"entity_id": run_id})));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    insert_agent_tab(
        state,
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working).recording_into(log),
    )
}

/// Done refuses while an agent is working at the root, and once it is offered
/// it closes every writer in that workspace — and only that workspace — before
/// the files go.
#[test]
fn done_refuses_while_an_agent_works_then_stops_every_agent_at_its_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let workspace = create_workspace(&mut state, project_id, "inbox-done");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({"entity_id": run_id})));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let posted = state.handle(req(
        "thread.post",
        json!({"entity_id": run_id, "agent_id": agent_id, "body": "keep working"}),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(state.delivery_queue.queued_len(), 1);
    let stopped = SessionLog::default();
    insert_agent_tab(
        &mut state,
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working).recording_into(&stopped),
    );

    let untouched = SessionLog::default();
    let neighbor_key = neighbor_workspace_working_in_it(&mut state, tmp.path(), &untouched);

    assert_done_refused(&mut state, &workspace_id, "an agent is working");
    assert!(root.is_dir(), "a refused Done removes nothing");
    assert_eq!(state.delivery_queue.queued_len(), 1);

    // This is +0/-0 and was the false-positive behind offering Done from the
    // summary counts alone. The refusal happens before a file is touched.
    let checkout = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    std::fs::write(checkout.join("empty.bin"), []).unwrap();
    insert_agent_tab(
        &mut state,
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Waiting).recording_into(&stopped),
    );
    assert_done_refused(&mut state, &workspace_id, "uncommitted changes");
    assert_eq!(state.delivery_queue.queued_len(), 1);
    assert!(state.workspaces.get(&workspace_id).is_some());

    std::fs::remove_file(checkout.join("empty.bin")).unwrap();
    let finished = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["complete"], true, "{finished:?}");
    assert!(!root.exists(), "Done removes the workspace checkout");
    assert!(state.delivery_queue.queued_is_empty());
    assert!(
        stopped.ended(),
        "Done waits until its live agent has stopped"
    );
    assert!(!untouched.ended(), "Done is scoped to one workspace root");
    assert!(state.session_registry.contains(&neighbor_key));
    assert!(state.workspaces.get(&workspace_id).is_none());
    assert_archived_workspace_is_history(&mut state, &workspace_id);
}

#[test]
fn persisted_workspaces_are_discovered_after_app_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let config = tmp.path().join("config.json");
    let worktrees = tmp.path().join("worktrees");
    let (workspace_id, project_id) = {
        let mut state = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
            .with_config(&config)
            .unwrap();
        let project = state.handle(req("project.add", json!({"path": repo})));
        let project_id = project["result"]["project_id"]
            .as_str()
            .unwrap()
            .to_string();
        let workspace = create_workspace(&mut state, &project_id, "durable");
        (
            workspace["workspace_id"].as_str().unwrap().to_string(),
            project_id,
        )
    };
    let mut restarted = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
        .with_config(&config)
        .unwrap();
    let got = restarted.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(got["ok"], true, "{got:?}");
    assert_eq!(got["result"]["project_id"], project_id);
    assert_eq!(got["result"]["status"], "ready");
    assert!(
        !repo.join(crate::workspace::MANIFEST_FILE).exists(),
        "workspace state is never written into the source repository"
    );
}

#[test]
fn legacy_git_root_and_external_checkout_are_adopted_without_moving_them() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "legacy");
    let canonical = std::fs::canonicalize(&repo).unwrap();
    let external = add_external_worktree(&repo, tmp.path(), "existing-work", "existing-work");
    let external_canonical = std::fs::canonicalize(&external).unwrap();
    let mut state = AppState::new(
        &repo,
        tmp.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert_eq!(listed["ok"], true, "{listed:?}");
    let workspaces = listed["result"]["workspaces"].as_array().unwrap();
    // The project's own checkout is the template workspaces are cut from, not
    // a place to work: it is adopted under its id, and never listed.
    assert!(
        !workspaces
            .iter()
            .any(|workspace| workspace["root"] == canonical.display().to_string()),
        "the primary checkout is not a listed workspace: {listed:?}"
    );
    let primary = state.handle(req(
        "workspace.get",
        json!({"workspace_id": format!("legacy-{project_id}")}),
    ));
    assert_eq!(primary["ok"], true, "{primary:?}");
    assert_eq!(
        primary["result"]["directories"][0]["path"],
        canonical.display().to_string()
    );
    let adopted = workspaces
        .iter()
        .find(|workspace| workspace["root"] == external_canonical.display().to_string())
        .unwrap_or_else(|| panic!("the existing checkout is adopted: {listed:?}"));
    assert_eq!(
        adopted["directories"][0]["path"],
        external_canonical.display().to_string()
    );
    assert!(repo.join(".git").is_dir());
    assert!(external.join(".git").is_file());
}

#[cfg(unix)]
#[test]
fn a_workspace_with_a_failed_plain_source_cannot_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let fifo = plain.join("unsupported-fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut state = app(tmp.path());
    let project = state.handle(req(
        "project.add",
        json!({"name": "plain", "sources": [{"path": plain}]}),
    ));
    assert_eq!(project["ok"], true, "{project:?}");
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let create = state.handle(req(
        "workspace.create",
        json!({"project_id": project_id, "name": "broken", "isolation": "worktree"}),
    ));
    assert_eq!(create["ok"], false, "{create:?}");
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let workspaces = listed["result"]["workspaces"].as_array().unwrap();
    assert!(
        workspaces.is_empty(),
        "the failed creation leaves nothing listed, and the project's own checkout never is: {listed:?}"
    );
    let project_workspace_root = state.workspaces.root().join(project_id);
    assert!(
        !project_workspace_root.exists()
            || std::fs::read_dir(project_workspace_root)
                .unwrap()
                .next()
                .is_none(),
        "failed creation must remove its workspace directory"
    );
}

#[cfg(unix)]
#[test]
fn a_later_source_failure_removes_prior_git_checkout_branch_and_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert!(Command::new("mkfifo")
        .arg(plain.join("unsupported-fifo"))
        .status()
        .unwrap()
        .success());
    let refs_before = crate::git_process::run_git(
        &repo,
        &["for-each-ref", "--format=%(refname)", "refs/heads/build"],
    )
    .unwrap();
    let worktrees_before =
        crate::git_process::run_git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
    let mut state = app(tmp.path());
    let project = state.handle(req(
        "project.add",
        json!({
            "name": "mixed",
            "sources": [
                {"name": "repo", "path": repo},
                {"name": "plain", "path": plain}
            ]
        }),
    ));
    assert_eq!(project["ok"], true, "{project:?}");
    let project_id = project["result"]["project_id"].as_str().unwrap();

    let create = state.handle(req(
        "workspace.create",
        json!({
            "project_id": project_id,
            "name": "Bridge wire interface / 🦀",
            "isolation": "worktree"
        }),
    ));

    assert_eq!(create["ok"], false, "{create:?}");
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert!(
        listed["result"]["workspaces"]
            .as_array()
            .unwrap()
            .is_empty(),
        "failed workspace was published: {listed:?}"
    );
    assert_eq!(
        crate::git_process::run_git(
            &repo,
            &["for-each-ref", "--format=%(refname)", "refs/heads/build"]
        )
        .unwrap(),
        refs_before,
        "the branch cut for the first source must be removed"
    );
    assert_eq!(
        crate::git_process::run_git(&repo, &["worktree", "list", "--porcelain"]).unwrap(),
        worktrees_before,
        "the linked-worktree registration must be removed"
    );
    let project_workspace_root = state.workspaces.root().join(project_id);
    assert!(
        !project_workspace_root.exists()
            || std::fs::read_dir(project_workspace_root)
                .unwrap()
                .next()
                .is_none(),
        "failed creation must remove its unpublished root"
    );
}

#[test]
fn workspace_create_preserves_free_form_name_and_uses_a_safe_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();
    let name = "  Bridge wire interface / 🦀 ..  ";

    let created = create_workspace(&mut state, project_id, name);
    let workspace = state.handle(req(
        "workspace.get",
        json!({"workspace_id": created["workspace_id"]}),
    ));

    assert_eq!(workspace["ok"], true, "{workspace:?}");
    assert_eq!(workspace["result"]["name"], name);
    assert_eq!(
        workspace["result"]["directories"][0]["branch"],
        "build/bridge-wire-interface"
    );
    let root = Path::new(workspace["result"]["root"].as_str().unwrap());
    assert_eq!(root.file_name().unwrap(), "bridge-wire-interface");
}

#[test]
fn repeated_workspace_names_use_distinct_checkout_registrations() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"].as_str().unwrap();

    let first = create_workspace(&mut state, project_id, "same name");
    let second = create_workspace(&mut state, project_id, "same name");
    let first = state.handle(req(
        "workspace.get",
        json!({"workspace_id": first["workspace_id"]}),
    ));
    let second = state.handle(req(
        "workspace.get",
        json!({"workspace_id": second["workspace_id"]}),
    ));

    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(second["ok"], true, "{second:?}");
    assert_ne!(first["result"]["root"], second["result"]["root"]);
    assert_ne!(
        first["result"]["directories"][0]["branch"],
        second["result"]["directories"][0]["branch"]
    );
    assert!(Path::new(first["result"]["directories"][0]["path"].as_str().unwrap()).is_dir());
    assert!(Path::new(second["result"]["directories"][0]["path"].as_str().unwrap()).is_dir());
}

#[test]
fn project_delete_requires_confirmation_and_removes_workspaces_but_preserves_sources() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let other_repo = init_repo_named(dir.path(), "other");
    let config = dir.path().join("config.json");
    let mut state = app(dir.path()).with_config(&config).unwrap();
    state.store = Some(crate::store::Store::new(dir.path().join("tasks")).unwrap());
    let project_id = state.add_project(repo.clone(), "main".into());
    let other_id = state.add_project(other_repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "delete-me");
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace["workspace_id"]}),
    ));
    assert_eq!(conversation["ok"], true, "{conversation}");
    let conversation_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_workspace = create_workspace(&mut state, &other_id, "keep-me");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let kept_root = PathBuf::from(other_workspace["root"].as_str().unwrap());
    let refused = state.handle(req("project.delete", json!({"project_id": project_id})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(root.exists());
    assert!(state.project(&project_id).is_some());
    let deleted = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(deleted["result"]["deleted"], true);
    assert_project_deletion_state(
        &state,
        &project_id,
        &other_id,
        &conversation_id,
        &root,
        &repo,
        &kept_root,
    );
    assert_project_deletion_persisted(dir.path(), &config, &project_id, &other_id);
}

fn assert_project_deletion_state(
    state: &AppState,
    project_id: &str,
    other_id: &str,
    conversation_id: &str,
    root: &Path,
    repo: &Path,
    kept_root: &Path,
) {
    assert!(!root.exists());
    assert!(repo.join(".git").exists());
    assert!(kept_root.exists());
    assert!(state.project(project_id).is_none());
    assert!(!state.runs.contains_key(conversation_id));
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .is_empty());
    assert!(state.project(other_id).is_some());
    assert!(state.workspaces.list(Some(project_id)).is_empty());
    assert!(!state.project_deletion_in_progress);
}

fn assert_project_deletion_persisted(
    app_root: &Path,
    config: &Path,
    project_id: &str,
    other_id: &str,
) {
    let restored = app(app_root).with_config(config).unwrap();
    assert!(restored.project(project_id).is_none());
    assert!(restored.project(other_id).is_some());
    assert!(restored.workspaces.list(Some(project_id)).is_empty());
}

#[test]
fn project_delete_persistence_failure_preserves_project_and_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path())
        .with_config(dir.path().join("config.json"))
        .unwrap();
    let project_id = state.add_project(repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "keep-me");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    state.config_persist_failure = Some(ConfigPersistStep::Write);
    let refused = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(root.exists());
    assert!(state.project(&project_id).is_some());
    assert!(!state.project_deletion_in_progress);
}

#[test]
fn project_delete_cleanup_failure_keeps_project_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path());
    let project_id = state.add_project(repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "retry-me");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let parked = dir.path().join("parked");
    std::fs::rename(&root, &parked).unwrap();
    std::fs::write(&root, "simulate a replaced workspace directory").unwrap();
    let refused = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(state.project(&project_id).is_some());
    assert!(!state.project_deletion_in_progress);
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(&parked, &root).unwrap();
    state.workspaces.reload().unwrap();
    let deleted = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(!root.exists());
    assert!(repo.exists());
}

#[test]
fn project_delete_blocks_mutations_until_filesystem_cleanup_settles() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path());
    let project_id = state.add_project(repo, "main".into());
    state
        .project_delete(&json!({"project_id": project_id, "confirm": true}))
        .unwrap();
    assert!(state.project_deletion_in_progress);
    // The runtime normally takes this work immediately before releasing its lock.
    let pending = state.deferred_work.take();
    let refused = state.handle(req("settings.set", json!({"default_harness": "pi"})));
    assert_eq!(refused["ok"], false, "{refused}");
    let read = state.handle(req("settings.get", json!({})));
    assert_eq!(read["ok"], true, "{read}");
    state.deferred_work = pending;
}

#[test]
fn project_delete_refuses_a_workspace_containing_a_source_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path());
    let project_id = state.add_project(repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "protected");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    state.project_at_mut(0).sources[0].path = root.clone();
    let refused = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(root.exists());
    assert!(repo.exists());
    assert!(state.project(&project_id).is_some());
}

#[test]
fn project_delete_waits_for_existing_filesystem_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path());
    let project_id = state.add_project(repo.clone(), "main".into());
    let status_params = json!({"project_id": project_id});
    let (status, deferred) = state.dispatch_deferring("git.status", &status_params);
    assert!(status.is_ok(), "{status:?}");
    let deferred = deferred.expect("git status releases the state lock");
    let refused = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(state.project(&project_id).is_some());
    assert!(repo.exists());
    assert!(!state.project_deletion_in_progress);
    state
        .apply_deferred("git.status", &status_params, deferred.run())
        .unwrap();
    let deleted = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(deleted["ok"], true, "{deleted}");
}

#[test]
fn project_delete_preserves_another_projects_source_inside_its_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let mut state = app(dir.path());
    let project_id = state.add_project(repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "shared-source");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let nested_source = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let other_id = state.add_project(nested_source.clone(), "main".into());
    let refused = state.handle(req(
        "project.delete",
        json!({"project_id": project_id, "confirm": true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(root.exists());
    assert!(nested_source.exists());
    assert!(state.project(&project_id).is_some());
    assert!(state.project(&other_id).is_some());
}

// ---- renaming and deleting one workspace -----------------------------------

/// A pretty name is a label and nothing else: the directory the checkouts live
/// in and the branch they were cut on are what every terminal and agent is
/// already holding, so renaming must not move either — and must still be there
/// after the registry is read back off disk.
#[test]
fn workspace_rename_moves_the_label_and_survives_a_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let config = tmp.path().join("config.json");
    let worktrees = tmp.path().join("worktrees");
    let (workspace_id, root, branch) = {
        let mut state = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
            .with_config(&config)
            .unwrap();
        let project_id = state.add_project(repo.clone(), "main".into());
        let workspace = create_workspace(&mut state, &project_id, "first-name");
        let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
        let root = PathBuf::from(workspace["root"].as_str().unwrap());
        let branch = workspace["directories"][0]["branch"]
            .as_str()
            .unwrap()
            .to_string();

        let renamed = state.handle(req(
            "workspace.rename",
            json!({"workspace_id": workspace_id, "name": "  second name  "}),
        ));
        assert_eq!(renamed["ok"], true, "{renamed:?}");
        assert_eq!(renamed["result"]["name"], "second name", "trimmed");
        assert_eq!(renamed["result"]["root"], root.display().to_string());
        assert_eq!(renamed["result"]["directories"][0]["branch"], branch);
        // The detail shape `workspace.get` answers, so one read repaints.
        assert!(renamed["result"].get("entity_id").is_some());
        (workspace_id, root, branch)
    };

    assert!(
        root.exists(),
        "the folder on disk keeps the name it was cut with"
    );
    let mut restarted = AppState::new_unrooted(&worktrees, "main", true, "/tmp/test-mcp.sock")
        .with_config(&config)
        .unwrap();
    let got = restarted.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(got["result"]["name"], "second name", "{got:?}");
    assert_eq!(got["result"]["directories"][0]["branch"], branch);
}

#[test]
fn workspace_rename_refuses_a_name_that_is_only_whitespace() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project_id = state.add_project(repo, "main".into());
    let workspace = create_workspace(&mut state, &project_id, "keeps-its-name");
    let refused = state.handle(req(
        "workspace.rename",
        json!({"workspace_id": workspace["workspace_id"], "name": "   "}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert_eq!(listed["result"]["workspaces"][0]["name"], "keeps-its-name");
}

/// Delete is the whole teardown in one verb: the conversation the workspace
/// owned stops and leaves the store, the isolated checkout is handed back to
/// the repository it was cut from, the root goes, and the listing forgets it —
/// while the source repository beside it is untouched.
#[test]
fn workspace_delete_removes_the_checkout_the_root_and_the_listing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    state.store = Some(crate::store::Store::new(tmp.path().join("tasks")).unwrap());
    let project_id = state.add_project(repo.clone(), "main".into());
    let workspace = create_workspace(&mut state, &project_id, "delete-me");
    let kept = create_workspace(&mut state, &project_id, "keep-me");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let kept_root = PathBuf::from(kept["root"].as_str().unwrap());
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(conversation["ok"], true, "{conversation}");
    let conversation_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();

    let deleted = state.handle(req(
        "workspace.delete",
        json!({"workspace_id": workspace_id}),
    ));

    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert_eq!(deleted["result"]["deleted"], true);
    assert_eq!(deleted["result"]["workspace_id"], workspace_id);
    assert!(!root.exists(), "the workspace root is gone");
    assert!(kept_root.exists(), "its neighbour is untouched");
    assert!(repo.join(".git").is_dir(), "the source repository stays");
    assert!(!state.runs.contains_key(&conversation_id));
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .is_empty());
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let names: Vec<_> = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"keep-me".to_string()), "{listed:?}");
    assert!(!names.contains(&"delete-me".to_string()), "{listed:?}");
    // The worktree registration went with it, so the same branch can be cut
    // again rather than colliding with an administrative record of a checkout
    // that is no longer on disk.
    let again = create_workspace(&mut state, &project_id, "delete-me");
    assert_eq!(again["status"], "ready", "{again:?}");
}

#[test]
fn workspace_delete_refuses_an_adopted_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "legacy");
    let external = add_external_worktree(&repo, tmp.path(), "existing-work", "existing-work");
    let external_canonical = std::fs::canonicalize(&external).unwrap();
    let mut state = AppState::new(
        &repo,
        tmp.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let adopted = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["root"] == external_canonical.display().to_string())
        .unwrap_or_else(|| panic!("the existing checkout is adopted: {listed:?}"))
        .clone();

    let refused = state.handle(req(
        "workspace.delete",
        json!({"workspace_id": adopted["workspace_id"]}),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    // The refusal is said plainly, to the reader, not as a verb's log line.
    assert_eq!(
        refused["error"],
        "Build cannot remove an adopted checkout. Only workspaces Build created can be deleted.",
        "{refused:?}"
    );
    assert!(external.join(".git").is_file(), "nothing was removed");

    let also_refused = state.handle(req(
        "workspace.rename",
        json!({"workspace_id": adopted["workspace_id"], "name": "renamed"}),
    ));
    assert_eq!(also_refused["ok"], false, "{also_refused:?}");
    assert_eq!(also_refused["error_code"], "conflict", "{also_refused:?}");
}

/// `(can_finish, finish_blockers)` off the feed's row for this workspace. The
/// cached summary is aged first, so each step of a test measures the tree it
/// just changed rather than the one the previous poll walked.
fn done_row(state: &mut AppState, workspace_id: &str) -> (bool, Vec<String>) {
    state.age_workspace_summary_for_test(workspace_id, crate::app::WORKSPACE_SUMMARY_TTL * 2);
    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["workspace_summaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["workspace_id"] == json!(workspace_id))
        .unwrap_or_else(|| panic!("the workspace is on the feed: {board:?}"))
        .clone();
    (
        row["can_finish"]
            .as_bool()
            .unwrap_or_else(|| panic!("{row:?}")),
        row["finish_blockers"]
            .as_array()
            .unwrap_or_else(|| panic!("{row:?}"))
            .iter()
            .map(|blocker| blocker.as_str().unwrap().to_string())
            .collect(),
    )
}

/// Done only appears once the user has synced the repo with a remote, and the
/// row says which of the three things is in the way until then.
#[test]
fn a_workspace_row_offers_done_once_its_git_work_is_clean_and_pushed() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project = state.handle(req("project.add", json!({"path": repo})));
    let project_id = project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = create_workspace(&mut state, &project_id, "done-row");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let checkout = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());

    assert_eq!(done_row(&mut state, &workspace_id), (true, Vec::new()));

    // An empty untracked file carries a +0/-0 stat, so only the status walk
    // sees it.
    std::fs::write(checkout.join("empty.bin"), []).unwrap();
    assert_eq!(
        done_row(&mut state, &workspace_id),
        (false, vec!["dirty".to_string()])
    );

    git_in(&checkout, &["add", "empty.bin"]);
    git_in(&checkout, &["commit", "-m", "local work"]);
    assert_eq!(
        done_row(&mut state, &workspace_id),
        (false, vec!["unpushed".to_string()])
    );

    git_in(&checkout, &["push", "origin", "HEAD"]);
    assert_eq!(done_row(&mut state, &workspace_id), (true, Vec::new()));

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({"entity_id": run_id})));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    insert_agent_tab(
        &mut state,
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    );
    assert_eq!(
        done_row(&mut state, &workspace_id),
        (false, vec!["agent_working".to_string()])
    );
}

/// Done removes the workspace, so an ordinary directory stands in its way: no
/// remote holds a copy of those files, and nothing measures whether they were
/// edited. The row says so, the verb refuses in the same words, and removing
/// the directory is the way out.
#[test]
fn done_waits_for_a_directory_no_repository_publishes() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let plain = tmp.path().join("assets");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("logo.svg"), "<svg/>\n").unwrap();
    let mut state = app(tmp.path());
    let project_id = create_mixed_project(&mut state, &repo, &plain);
    let workspace = create_workspace(&mut state, &project_id, "mixed-done");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let assets = PathBuf::from(directory(&workspace, "source-2")["path"].as_str().unwrap());
    std::fs::write(assets.join("logo.svg"), "<svg viewBox=\"0 0 1 1\"/>\n").unwrap();

    assert_eq!(
        done_row(&mut state, &workspace_id),
        (false, vec!["plain_directory".to_string()])
    );
    assert_done_refused(&mut state, &workspace_id, "is not a repository");
    assert!(root.is_dir(), "a refused Done removes nothing");

    let shrunk = state.handle(req(
        "workspace.remove_directory",
        json!({"workspace_id": workspace_id, "directory_id": "source-2"}),
    ));
    assert_eq!(shrunk["ok"], true, "{shrunk:?}");
    assert_eq!(done_row(&mut state, &workspace_id), (true, Vec::new()));

    // A workspace that is nothing but ordinary directories is the same fact
    // with nothing beside it.
    let plain_only = state.handle(req("project.add", json!({"path": plain})));
    let plain_project = plain_only["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let bare = create_workspace(&mut state, &plain_project, "no-repository");
    let bare_id = bare["workspace_id"].as_str().unwrap().to_string();
    assert_eq!(
        done_row(&mut state, &bare_id),
        (false, vec!["plain_directory".to_string()])
    );
}
