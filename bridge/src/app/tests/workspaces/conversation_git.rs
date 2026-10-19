// A workspace conversation's run stands on the workspace root, which is not a
// repository: the workspace's git is in its git directory. Every run-scoped
// git read, and the git facts a push carries for that run, answer from there —
// which is what the client files under the workspace's entity.

use super::*;

fn conversation_run(state: &mut AppState, workspace_id: &str) -> String {
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(conversation["ok"], true, "{conversation:?}");
    conversation["result"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_workspace_conversation_reads_git_from_its_git_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.create", open_folder(json!({"path": repo}))));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let git_dir = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let run_id = conversation_run(&mut state, &workspace_id);
    std::fs::write(git_dir.join("note.txt"), b"hello").unwrap();

    let status = state.handle(req("git.status", json!({"run_id": run_id})));
    assert_eq!(status["ok"], true, "{status:?}");
    let files: Vec<&str> = status["result"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, vec!["note.txt"]);

    let log = state.handle(req("git.log", json!({"run_id": run_id})));
    assert_eq!(log["ok"], true, "{log:?}");
    assert!(!log["result"]["commits"].as_array().unwrap().is_empty());

    let unpushed = state.handle(req("git.unpushed", json!({"run_id": run_id})));
    assert_eq!(unpushed["ok"], true, "{unpushed:?}");
    let diff = state.handle(req("run.diff", json!({"run_id": run_id})));
    assert_eq!(diff["ok"], true, "{diff:?}");
}

#[test]
fn workspace_conversation_files_share_the_source_root_while_terminals_keep_the_container() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.create", open_folder(json!({"path": repo}))));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let source = directory(&workspace, "source-1");
    let source_root = PathBuf::from(source["path"].as_str().unwrap());
    let source_name = source_root.file_name().unwrap().to_str().unwrap();
    let run_id = conversation_run(&mut state, workspace_id);
    std::fs::write(source_root.join("note.txt"), b"hello").unwrap();

    let run_tree = state
        .fs_tree(&json!({"run_id": run_id, "path": ""}))
        .unwrap();
    let source_tree = state
        .fs_tree(&json!({"workspace_id": workspace_id, "source_id": "source-1", "path": ""}))
        .unwrap();
    assert_eq!(run_tree["entries"], source_tree["entries"]);
    assert!(run_tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "note.txt"));
    assert!(state
        .fs_tree(&json!({"run_id": run_id, "path": source_name}))
        .is_err());

    let terminal_root =
        crate::app::runtime::terminals::terminal_scope_root(&mut state, &json!({"run_id": run_id}))
            .unwrap();
    assert_eq!(
        terminal_root,
        PathBuf::from(workspace["root"].as_str().unwrap())
    );
}

#[test]
fn a_workspace_conversation_is_a_git_subject_at_its_git_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.create", open_folder(json!({"path": repo}))));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let git_dir = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let run_id = conversation_run(&mut state, &workspace_id);

    let roots = state.worktree_roots();
    assert_eq!(roots.get(&run_id), Some(&git_dir), "{roots:?}");
}

/// Subscription changes and attention stamps rebuild this map while holding
/// AppState. Finding every run's workspace must not repeat the complete
/// workspace-by-run filesystem search for each entry in the map.
#[test]
fn worktree_roots_resolves_workspace_ownership_without_cubic_scans() {
    let mut measurements = Vec::new();
    for (count, shared_root) in [(16, false), (64, false), (16, true), (64, true)] {
        let (_tmp, state, expected) = ownership_fixture(count, shared_root);
        let started = std::time::Instant::now();
        let (roots, comparisons) =
            crate::app::workspaces::count_path_comparisons(|| state.worktree_roots());
        eprintln!(
            "{count} runs x {count} workspaces (shared_root={shared_root}): {comparisons} path comparisons, {:?}",
            started.elapsed()
        );
        for (run_id, root) in expected {
            assert_eq!(roots.get(&run_id), Some(&root));
        }
        measurements.push((count, comparisons));
    }
    for (count, comparisons) in measurements {
        assert!(
            comparisons <= 4 * count * count,
            "one roots snapshot repeatedly resolved ownership: {comparisons} path comparisons for {count} workspaces"
        );
    }
}

fn ownership_fixture(
    count: usize,
    shared_root: bool,
) -> (tempfile::TempDir, AppState, Vec<(String, PathBuf)>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = app(tmp.path());
    let project_id = state.add_project(tmp.path().to_path_buf(), "main".into());
    let mut expected = Vec::new();
    for number in 0..count {
        let workspace_id = format!("workspace-{number:02}");
        let run_id = format!("run-{number:02}");
        let root = tmp
            .path()
            .join(if shared_root { "shared" } else { &workspace_id });
        std::fs::create_dir_all(&root).unwrap();
        state.workspaces.adopt_root(
            &project_id,
            workspace_id.clone(),
            workspace_id.clone(),
            root.clone(),
            "source-1".into(),
            false,
        );
        state
            .workspaces
            .record_git_repository(&workspace_id, "source-1", "main")
            .unwrap();
        let root = if shared_root {
            tmp.path().join(&run_id)
        } else {
            root
        };
        std::fs::create_dir_all(&root).unwrap();
        let active = crate::orchestrator::ActiveRun::workspace_conversation(
            crate::run::RunId::new(&run_id),
            workspace_id,
            root.clone(),
            crate::models::ModelChoice::default(),
        );
        state
            .projects
            .bind_entity(run_id.clone(), project_id.clone());
        state.runs.insert(run_id.clone(), active);
        expected.push((run_id, root));
    }
    if shared_root {
        // The matching group comes after unrelated owners in the actual map
        // traversal, making a repeated winner search expensive deterministically.
        let later = state
            .runs
            .keys()
            .skip(count / 2)
            .cloned()
            .collect::<Vec<_>>();
        for id in later {
            let root = tmp.path().join("shared");
            state.runs.get_mut(&id).unwrap().worktree.path = root.clone();
            expected
                .iter_mut()
                .find(|(run_id, _)| run_id == &id)
                .unwrap()
                .1 = root;
        }
    }
    (tmp, state, expected)
}

/// Keep the old name-ordered forward-owner lookup as the independent oracle.
/// Direct ids, legacy aliases and a path shared by several runs must retain
/// the exact winner, including when an earlier alias precedes a direct id.
#[test]
fn reverse_workspace_lookup_preserves_existing_ownership_answers() {
    let (tmp, mut state, _) = ownership_fixture(8, false);
    let project_id = state.projects.project_id_of("run-00").unwrap().to_string();
    state.runs.get_mut("run-00").unwrap().run.state = crate::run::RunState::Merged;
    state.workspaces.adopt_root(
        &project_id,
        "run-00".into(),
        "zz-terminal-direct".into(),
        tmp.path().join("terminal-direct"),
        "source-1".into(),
        false,
    );
    state.workspaces.adopt_root(
        &project_id,
        "run-01".into(),
        "zz-live-direct".into(),
        tmp.path().join("live-direct"),
        "source-1".into(),
        false,
    );
    let shared = state.runs["run-02"].worktree.path.clone();
    state.runs.get_mut("run-03").unwrap().worktree.path = shared;
    let other_project = tmp.path().join("other-project");
    std::fs::create_dir(&other_project).unwrap();
    let other_project_id = state.add_project(other_project, "main".into());
    state
        .projects
        .bind_entity("run-04".into(), other_project_id);
    let scratch = crate::app::projects::scratch_dir(&state.state_root, tmp.path());
    std::fs::create_dir_all(&scratch).unwrap();
    state.runs.get_mut("run-06").unwrap().worktree.path = scratch.clone();
    state.workspaces.adopt_root(
        &project_id,
        "run-06".into(),
        "project-scratch".into(),
        scratch,
        "source-1".into(),
        false,
    );
    std::fs::remove_dir(&state.runs["run-07"].worktree.path).unwrap();
    #[cfg(unix)]
    {
        let alias = tmp.path().join("linked-root");
        std::os::unix::fs::symlink(&state.runs["run-05"].worktree.path, &alias).unwrap();
        state.runs.get_mut("run-05").unwrap().worktree.path = alias;
    }
    assert_ownership_matches_forward_lookup(&state);
    assert_eq!(
        state.workspace_of_conversation("run-00").as_deref(),
        Some("run-00")
    );
    assert_eq!(
        state.workspace_of_conversation("run-01").as_deref(),
        Some("workspace-01")
    );
    assert_eq!(state.workspace_of_conversation("run-04"), None);
    assert_eq!(state.workspace_of_conversation("run-06"), None);
}

fn assert_ownership_matches_forward_lookup(state: &AppState) {
    for (run_id, active) in &state.runs {
        let expected = state.workspaces.list(None).into_iter().find(|workspace| {
            state.workspace_conversation_owner(workspace).as_deref() == Some(run_id)
        });
        assert_eq!(
            state.workspace_of_conversation(run_id),
            expected.map(|workspace| workspace.id.clone())
        );
        assert_eq!(
            state
                .workspace_summary_subject_of_run(run_id)
                .map(|(id, _)| id),
            expected.map(|workspace| workspace.id.clone())
        );
        let expected_root = expected
            .and_then(|workspace| {
                workspace
                    .directories
                    .iter()
                    .find(|directory| directory.is_git)
            })
            .map(|directory| directory.path.clone())
            .unwrap_or_else(|| active.worktree.path.clone());
        assert_eq!(
            state.run_git_root(run_id, &active.worktree.path),
            expected_root
        );
    }
}

/// The list names each workspace's conversation entity the way `workspace.get`
/// does, so a client standing on the list can file the workspace's git under
/// it without asking for the row.
#[test]
fn the_workspace_list_names_each_conversation_entity() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.create", open_folder(json!({"path": repo}))));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let spoken = create_workspace(&mut state, &project_id, "spoken");
    let quiet = create_workspace(&mut state, &project_id, "quiet");
    let run_id = conversation_run(&mut state, spoken["workspace_id"].as_str().unwrap());

    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert_eq!(listed["ok"], true, "{listed:?}");
    let rows = listed["result"]["workspaces"].as_array().unwrap();
    let row_of = |id: &Value| rows.iter().find(|row| row["workspace_id"] == *id).unwrap();
    assert_eq!(row_of(&spoken["workspace_id"])["entity_id"], json!(run_id));
    assert_eq!(row_of(&quiet["workspace_id"])["entity_id"], Value::Null);
}
