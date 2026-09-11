use super::*;

fn app(root: &Path) -> AppState {
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

fn directory<'a>(workspace: &'a Value, source_id: &str) -> &'a Value {
    workspace["directories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|directory| directory["source_id"] == source_id)
        .unwrap_or_else(|| panic!("source {source_id} is present: {workspace:?}"))
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

#[test]
fn finish_checks_every_git_source_and_retains_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let (one, origin_one) = repo_with_origin(tmp.path(), "one");
    let (two, origin_two) = repo_with_origin(tmp.path(), "two");
    let mut state = app(tmp.path());
    let project = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": one}, {"path": two}]}),
    ));
    let workspace = create_workspace(
        &mut state,
        project["result"]["project_id"].as_str().unwrap(),
        "done",
    );
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    let finished = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["complete"], true, "{finished:?}");
    assert_eq!(
        finished["result"]["repositories"].as_array().unwrap().len(),
        2
    );
    assert!(root.is_dir(), "Finish retains the workspace directory");
    for origin in [origin_one, origin_two] {
        let branches = Command::new("git")
            .args([
                "--git-dir",
                origin.to_str().unwrap(),
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/",
            ])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&branches.stdout).lines().count() >= 2);
    }
    let current = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(current["result"]["status"], "finished", "{current:?}");
    // A terminal writes directly to disk, without an fs.write invalidation.
    let selected = Path::new(workspace["directories"][0]["path"].as_str().unwrap());
    std::fs::write(selected.join("later.txt"), "new work\n").unwrap();
    let reopened = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(reopened["result"]["status"], "ready", "{reopened:?}");
}

#[test]
fn incomplete_finish_reports_one_source_and_can_be_retried() {
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
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let dirty = PathBuf::from(directory(&workspace, "source-2")["path"].as_str().unwrap());
    std::fs::write(dirty.join("README.md"), "unfinished\n").unwrap();

    let first = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(first["result"]["complete"], false, "{first:?}");
    let failed = first["result"]["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|repo| repo["pushed"] == false)
        .collect::<Vec<_>>();
    assert_eq!(failed.len(), 1, "{first:?}");
    assert!(dirty.is_dir());

    git_in(&dirty, &["add", "README.md"]);
    git_in(&dirty, &["commit", "-m", "finish work"]);
    let retry = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(retry["ok"], true, "{retry:?}");
    assert_eq!(retry["result"]["complete"], true, "{retry:?}");
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
        let finished = state.handle(req(
            "workspace.finish",
            json!({"workspace_id": workspace["workspace_id"]}),
        ));
        assert_eq!(finished["result"]["complete"], true, "{finished:?}");
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
    assert_eq!(got["result"]["status"], "finished");
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
    let primary = workspaces
        .iter()
        .find(|workspace| workspace["root"] == canonical.display().to_string())
        .unwrap_or_else(|| panic!("the primary checkout is adopted: {listed:?}"));
    assert_eq!(
        primary["directories"][0]["path"],
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
    let failed = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["status"] == "failed")
        .unwrap_or_else(|| panic!("failed provisioning remains recoverable: {listed:?}"));
    let finish = state.handle(req(
        "workspace.finish",
        json!({"workspace_id": failed["workspace_id"]}),
    ));
    assert_eq!(finish["ok"], false, "{finish:?}");
    let after = state.handle(req(
        "workspace.get",
        json!({"workspace_id": failed["workspace_id"]}),
    ));
    assert_eq!(after["result"]["status"], "failed", "{after:?}");
    assert!(failed["root"]
        .as_str()
        .is_some_and(|root| Path::new(root).is_dir()));
}
