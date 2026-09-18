use super::*;

#[test]
fn direct_add_canonicalizes_dedupes_and_never_persists_config() {
    let directory = tempfile::tempdir().unwrap();
    let (_initial_directory, initial_repo) = init_repo();
    let (_added_directory, added_repo) = init_repo();
    let config = directory.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let mut state = AppState::new(
        initial_repo,
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let config_before = std::fs::read(&config).unwrap();
    let alias = added_repo.join("..").join(added_repo.file_name().unwrap());
    let added = state.add_project(alias, "main".to_string());
    let next_after_add = state.projects.next_id();
    let duplicate = state.add_project(added_repo, "other-base".to_string());

    assert_eq!(added, duplicate);
    assert_eq!(state.projects.next_id(), next_after_add);
    assert_eq!(std::fs::read(&config).unwrap(), config_before);
}

#[test]
fn direct_add_uses_the_original_path_when_canonicalization_fails() {
    let directory = tempfile::tempdir().unwrap();
    let (_initial_directory, initial_repo) = init_repo();
    let mut state = AppState::new(
        initial_repo,
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let missing = directory.path().join("missing-repository");

    let added = state.add_project(missing.clone(), "main".to_string());
    let next_after_add = state.projects.next_id();
    let duplicate = state.add_project(missing, "other-base".to_string());

    assert_eq!(added, duplicate);
    assert_eq!(state.projects.next_id(), next_after_add);
}

#[test]
fn project_add_validates_and_dedupes() {
    let (dir_a, repo_a) = init_repo();
    let mut state = AppState::new(
        repo_a.clone(),
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    // A non-repo path is rejected.
    let bad = state.handle(req(
        "project.add",
        json!({ "path": "/definitely/not/a/repo" }),
    ));
    assert_eq!(bad["ok"], false);
    // A real repo is added and listed.
    let (_dir_c, repo_c) = init_repo();
    let ok = state.handle(req(
        "project.add",
        json!({ "path": repo_c.to_str().unwrap() }),
    ));
    assert_eq!(ok["ok"], true, "{ok:?}");
    assert_eq!(
        state.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Adding the same repo again is idempotent (deduped by canonical path).
    state.handle(req(
        "project.add",
        json!({ "path": repo_c.to_str().unwrap() }),
    ));
    assert_eq!(
        state.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn plain_folder_add_does_not_initialize_git_and_can_be_initialized_explicitly() {
    let (dir, repo) = init_repo();
    let plain = dir.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "keep me\n").unwrap();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );

    let added = state.handle(req("project.add", json!({"path": plain})));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(added["result"]["is_git"], false);
    assert!(!plain.join(".git").exists());
    let project_id = added["result"]["project_id"].as_str().unwrap();

    let initialized = state.handle(req("project.init_git", json!({"project_id": project_id})));
    assert_eq!(initialized["ok"], true, "{initialized:?}");
    assert_eq!(initialized["result"]["is_git"], true);
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.txt")).unwrap(),
        "keep me\n"
    );
    let status = std::process::Command::new("git")
        .args(["-C", plain.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "?? notes.txt\n");
}

#[test]
fn plain_folder_persists_and_remains_browsable_after_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let (_initial_dir, initial_repo) = init_repo();
    let plain = tmp.path().join("plain");
    let config = tmp.path().join("config.json");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "hello\n").unwrap();
    {
        let mut state = AppState::new(
            initial_repo.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&config)
        .unwrap();
        assert_eq!(
            state.handle(req("project.add", json!({"path": plain})))["ok"],
            true
        );
    }

    let mut state = AppState::new(
        initial_repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let listed = state.handle(req("project.list", json!({})));
    let project = listed["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["path"] == plain.display().to_string())
        .unwrap();
    assert_eq!(project["is_git"], false);
    let tree = state.handle(req("fs.tree", json!({"project_id": project["project_id"]})));
    assert_eq!(tree["ok"], true, "{tree:?}");
    assert_eq!(tree["result"]["entries"][0]["name"], "notes.txt");
    let read = state.handle(req(
        "fs.read",
        json!({"project_id": project["project_id"], "path": "notes.txt"}),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(read["result"]["content_b64"], "aGVsbG8K");
    let board = state.handle(req("board.list", json!({})));
    let board_project = board["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["project_id"] == project["project_id"])
        .unwrap();
    assert_eq!(board_project["is_git"], false);
    assert!(!plain.join(".git").exists());
}

#[test]
fn adding_subdirectory_of_parent_repo_registers_that_folder_as_plain() {
    let (dir, repo) = init_repo();
    let nested = repo.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );

    let added = state.handle(req("project.add", json!({"path": nested})));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(added["result"]["is_git"], false);
    assert_eq!(
        added["result"]["path"],
        std::fs::canonicalize(nested).unwrap().display().to_string()
    );
}

/// Project rows probe repository config and volume capabilities. Those reads
/// can be slow, but must neither hold the app mutex nor tear one response.
#[test]
fn project_list_reads_metadata_off_lock_from_one_snapshot() {
    let (dir, repo) = init_repo();
    let mut app = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let expected = app.project_list();
    let expected_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_project_list_gate = Some(gate);
    let state = app.shared();

    let listed = frame_on_a_thread(&state, "s-list", "project.list", json!({}));
    gate_handle.wait_for_arrival();
    let models = frame_on_a_thread(&state, "s-models", "models.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("an unrelated foreground read answers while project metadata is blocked");
    assert_eq!(models["ok"], true, "{models:?}");

    state.lock().unwrap().clear_projects_for_test();
    gate_handle.release();
    let listed = listed
        .recv_timeout(Duration::from_secs(30))
        .expect("the project list answers once its metadata read is released");
    assert_eq!(listed["ok"], true, "{listed:?}");
    assert_eq!(listed["result"], expected, "{listed:?}");
    assert_eq!(listed["result"]["projects"][0]["project_id"], expected_id);
    assert!(
        state.lock().unwrap().projects.is_empty(),
        "the stale response must not restore a project removed while it ran"
    );
}

/// Registering a project opens the repository, shells out for its default
/// branch and resolves it — three disk reads on a directory the daemon has
/// never seen, none of which may hold the app mutex.
#[test]
fn project_add_reads_the_default_branch_with_the_state_lock_free() {
    let (dir_a, repo_a) = init_repo();
    let (_dir_b, repo_b) = init_repo();
    let mut app = AppState::new(
        repo_a,
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let added = frame_on_a_thread(
        &state,
        "s-add",
        "project.add",
        json!({ "path": repo_b.to_str().unwrap() }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the add is holding the app mutex through its git"
    );
    let listed = frame_on_a_thread(&state, "s-list", "project.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the project list answers while a repository is being read");
    assert_eq!(listed["ok"], true, "{listed:?}");

    gate_handle.release();
    let added = added
        .recv_timeout(Duration::from_secs(30))
        .expect("the add answers once its git is done");
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(added["result"]["base_branch"], "main", "{added:?}");
    assert_eq!(
        state.lock().unwrap().projects.len(),
        2,
        "the project is registered by the epilogue"
    );
}

/// The clone lands somewhere the decide phase never saw, so what the reply
/// says about the project is read out of the directory git left.
#[test]
fn project_clone_registers_its_project_from_the_landed_path() {
    let (dir_src, repo_src) = init_repo();
    let mut app = AppState::new(
        repo_src.clone(),
        dir_src.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    app.projects_dir = dir_src.path().join("projects");
    let projects_dir = app.projects_dir.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let cloned = frame_on_a_thread(
        &state,
        "s-clone",
        "project.clone",
        json!({ "url": repo_src.to_str().unwrap(), "name": "landed" }),
    );
    gate_handle.wait_for_arrival();
    let listed = frame_on_a_thread(&state, "s-list", "project.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the project list answers while a repository is being cloned");
    assert_eq!(listed["ok"], true, "{listed:?}");

    gate_handle.release();
    let cloned = cloned
        .recv_timeout(Duration::from_secs(60))
        .expect("the clone answers once its git is done");
    assert_eq!(cloned["ok"], true, "{cloned:?}");
    let landed = std::path::PathBuf::from(cloned["result"]["path"].as_str().unwrap());
    assert_eq!(
        landed,
        crate::worktree::canonical_root(&projects_dir.join("landed")),
        "{cloned:?}"
    );
    assert!(landed.join("README.md").exists(), "the clone is on disk");
    assert_eq!(
        cloned["result"]["remote"].as_str().map(str::to_string),
        git_remote_origin(&landed),
        "the reply carries the origin the clone wired"
    );
}

/// Setting a remote is three git subprocesses at worst, and a project's
/// wire view is read for every one of them.
#[test]
fn project_set_remote_writes_its_config_with_the_state_lock_free() {
    let (dir_a, repo_a) = init_repo();
    let mut app = AppState::new(
        repo_a.clone(),
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let wired = frame_on_a_thread(
        &state,
        "s-remote",
        "project.set_remote",
        json!({ "project_id": project_id, "url": "https://example.invalid/one.git" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the remote write is holding the app mutex"
    );
    let listed = frame_on_a_thread(&state, "s-list", "project.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the project list answers while a remote is being written");
    assert_eq!(listed["ok"], true, "{listed:?}");

    gate_handle.release();
    let wired = wired
        .recv_timeout(Duration::from_secs(30))
        .expect("the remote write answers once its git is done");
    assert_eq!(wired["ok"], true, "{wired:?}");
    assert_eq!(
        wired["result"]["remote"], "https://example.invalid/one.git",
        "{wired:?}"
    );
    assert_eq!(
        git_remote_origin(&repo_a).as_deref(),
        Some("https://example.invalid/one.git")
    );
}

/// Making a project is four subprocesses — `init`, `add`, `commit` and the
/// optional `remote add` — on a directory that did not exist when the verb
/// was asked for.
#[test]
fn project_create_writes_its_repository_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let parent = dir.path().join("made-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let created = frame_on_a_thread(
        &state,
        "s-create",
        "project.create",
        json!({
            "name": "fresh",
            "parent": parent.to_str().unwrap(),
            "remote": "https://example.invalid/fresh.git",
        }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the create is holding the app mutex through its git"
    );
    let listed = frame_on_a_thread(&state, "s-list", "project.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the project list answers while a repository is being created");
    assert_eq!(listed["ok"], true, "{listed:?}");

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    let landed = std::path::PathBuf::from(created["result"]["path"].as_str().unwrap());
    assert!(landed.join(".git").exists(), "the repository is on disk");
    assert_eq!(
        created["result"]["remote"], "https://example.invalid/fresh.git",
        "{created:?}"
    );
    assert_eq!(
        state.lock().unwrap().projects.len(),
        2,
        "the project is registered by the epilogue"
    );
}

#[test]
fn name_only_project_create_makes_the_missing_configured_projects_directory() {
    let (dir, repo) = init_repo();
    let projects_dir = dir.path().join("missing-parent").join("projects");
    let mut state = qa_state(&repo, dir.path()).with_projects_dir_default(projects_dir.clone());
    assert!(!projects_dir.exists());

    let created = state.handle(req("project.create", json!({ "name": "fresh" })));

    assert_eq!(created["ok"], true, "{created:?}");
    let destination = projects_dir.join("fresh").canonicalize().unwrap();
    assert_eq!(created["result"]["path"], destination.to_str().unwrap());
    let repository = git2::Repository::open(&destination).unwrap();
    assert_eq!(repository.head().unwrap().shorthand(), Some("main"));
    assert!(repository.head().unwrap().peel_to_commit().is_ok());
    assert_eq!(state.projects.len(), 2);
}

/// The row a create reserves stands for the directory the create writes:
/// one destination, settled in the decide phase and carried into the git.
/// A second asker for that directory is refused by the row guarding it, and
/// the repository lands exactly where the row said it would.
#[test]
fn a_second_create_of_one_directory_is_refused_by_the_row_guarding_it() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let parent = dir.path().join("made-here");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();
    let asked = json!({ "name": "fresh", "parent": parent.to_str().unwrap() });

    let created = frame_on_a_thread(&state, "s-create", "project.create", asked.clone());
    gate_handle.wait_for_arrival();
    let second = frame_on_a_thread(&state, "s-again", "project.create", asked)
        .recv_timeout(Duration::from_secs(10))
        .expect("the second create is answered while the first one's git runs");
    assert_eq!(
        second["ok"], false,
        "two creates wrote one directory: {second:?}"
    );

    gate_handle.release();
    let created = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the first create answers once its git is done");
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(
        std::path::PathBuf::from(created["result"]["path"].as_str().unwrap()),
        std::fs::canonicalize(parent.join("fresh")).expect("the repository is on disk"),
        "the repository landed somewhere other than the reserved directory: {created:?}"
    );
}

/// A clone whose git failed leaves nothing at all: no project, no row on
/// the board, and a destination the retry finds as empty as this one did.
#[test]
fn a_clone_that_fails_rolls_its_reservation_back_and_leaves_no_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    state.projects_dir = dir.path().join("projects");
    let destination = state.projects_dir.join("doomed");

    let failed = state.handle(req(
        "project.clone",
        json!({
            "url": dir.path().join("not-a-repository").to_str().unwrap(),
            "name": "doomed",
        }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert_eq!(
        state.projects.len(),
        1,
        "the failed clone registered a project"
    );
    assert!(
        state.pending_rows.is_empty(),
        "the failed clone left its row on the board"
    );
    let board = state.handle(req("board.list", json!({})));
    assert!(pending_on_the_board(&board).is_empty(), "{board:?}");
    assert!(
        !destination.exists(),
        "the half-written destination outlived the clone that failed"
    );
}

/// A project verb reserves the folder it is reaching for — two clones into
/// one directory are one clone — but a folder is not a card, so the board's
/// list of cards is never handed a row for it.
#[test]
fn a_project_verb_reserves_its_directory_without_a_row_on_the_board() {
    let (dir_src, repo_src) = init_repo();
    let mut app = qa_state(&repo_src, dir_src.path());
    app.projects_dir = dir_src.path().join("projects");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let cloning = frame_on_a_thread(
        &state,
        "s-clone",
        "project.clone",
        json!({ "url": repo_src.to_str().unwrap(), "name": "landing" }),
    );
    gate_handle.wait_for_arrival();

    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while a repository is being cloned");
    assert!(
        pending_on_the_board(&board).is_empty(),
        "the board was handed a row standing for a folder: {board:?}"
    );
    // The reservation is still what serializes the folder: a second clone
    // into it is refused rather than run into the first one's directory.
    let second = frame_on_a_thread(
        &state,
        "s-second",
        "project.clone",
        json!({ "url": repo_src.to_str().unwrap(), "name": "landing" }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the second clone is answered");
    assert_eq!(second["ok"], false, "{second:?}");

    gate_handle.release();
    let cloned = cloning
        .recv_timeout(Duration::from_secs(60))
        .expect("the clone answers once its git is done");
    assert_eq!(cloned["ok"], true, "{cloned:?}");
    assert!(
        state.lock().unwrap().pending_rows.is_empty(),
        "the reservation outlived the verb that took it"
    );
}

#[test]
fn project_add_persistence_failures_leave_state_and_user_repo_unchanged() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let directory = tempfile::tempdir().unwrap();
        let (_initial_repo_directory, initial_repo) = init_repo();
        let (_added_repo_directory, added_repo) = init_repo();
        let config = directory.path().join("config.json");
        let mut state = AppState::new(
            initial_repo,
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&config)
        .unwrap();
        let projects_before = state.project_list();
        let next_project_before = state.projects.next_id();
        state.config_persist_failure = Some(failure);

        let response = state.handle(req(
            "project.add",
            json!({ "path": added_repo.to_str().unwrap() }),
        ));

        assert_eq!(response["ok"], false, "{response:?}");
        assert!(response["error"].as_str().unwrap().contains("injected"));
        assert_eq!(state.project_list(), projects_before);
        assert_eq!(state.projects.next_id(), next_project_before);
        assert!(added_repo.join(".git").exists(), "user repo must remain");
    }
}

#[test]
fn project_clone_persistence_failures_leave_state_and_remove_new_clone() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let directory = tempfile::tempdir().unwrap();
        let (_initial_repo_directory, initial_repo) = init_repo();
        let (_source_directory, source_repo) = init_repo();
        let config = directory.path().join("config.json");
        let projects_dir = directory.path().join("projects");
        std::fs::create_dir(&projects_dir).unwrap();
        let existing_clone_path = projects_dir.join("existing-project");
        git2::Repository::clone(source_repo.to_str().unwrap(), &existing_clone_path).unwrap();
        let clone_path = projects_dir.join("cloned-project");
        let mut state = AppState::new(
            initial_repo,
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&config)
        .unwrap();
        state.projects_dir = projects_dir;
        let projects_before = state.project_list();
        let next_project_before = state.projects.next_id();
        state.config_persist_failure = Some(failure);

        let existing_response = state.handle(req(
            "project.clone",
            json!({
                "url": source_repo.to_str().unwrap(),
                "name": "existing-project",
            }),
        ));

        assert_eq!(existing_response["ok"], false, "{existing_response:?}");
        assert!(existing_response["error"]
            .as_str()
            .unwrap()
            .contains("injected"));
        assert_eq!(state.project_list(), projects_before);
        assert_eq!(state.projects.next_id(), next_project_before);
        assert!(
            existing_clone_path.join(".git").exists(),
            "an existing checkout is user-owned"
        );

        let response = state.handle(req(
            "project.clone",
            json!({
                "url": source_repo.to_str().unwrap(),
                "name": "cloned-project",
            }),
        ));

        assert_eq!(response["ok"], false, "{response:?}");
        assert!(response["error"].as_str().unwrap().contains("injected"));
        assert_eq!(state.project_list(), projects_before);
        assert_eq!(state.projects.next_id(), next_project_before);
        assert!(
            !clone_path.exists(),
            "failed clone registration is cleaned up"
        );
        assert!(source_repo.join(".git").exists(), "source repo must remain");
    }
}

#[test]
fn project_create_persistence_failures_leave_state_and_remove_new_repo() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let directory = tempfile::tempdir().unwrap();
        let (_initial_repo_directory, initial_repo) = init_repo();
        let config = directory.path().join("config.json");
        let parent = directory.path().join("created-projects");
        let created_path = parent.join("new-project");
        let mut state = AppState::new(
            initial_repo,
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&config)
        .unwrap();
        let projects_before = state.project_list();
        let next_project_before = state.projects.next_id();
        state.config_persist_failure = Some(failure);

        let response = state.handle(req(
            "project.create",
            json!({
                "name": "new-project",
                "parent": parent.to_str().unwrap(),
            }),
        ));

        assert_eq!(response["ok"], false, "{response:?}");
        assert!(response["error"].as_str().unwrap().contains("injected"));
        assert_eq!(state.project_list(), projects_before);
        assert_eq!(state.projects.next_id(), next_project_before);
        assert!(
            !created_path.exists(),
            "failed project creation is cleaned up"
        );
    }
}

#[test]
fn project_remote_change_does_not_write_unchanged_config() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_directory, repo) = init_repo();
    let config = directory.path().join("config.json");
    let mut state = AppState::new(
        repo.clone(),
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(config)
    .unwrap();
    let project_id = state.project_at(0).id.clone();
    state.config_persist_failure = Some(ConfigPersistStep::Write);

    let response = state.handle(req(
        "project.set_remote",
        json!({ "project_id": project_id, "url": "https://example.com/repo.git" }),
    ));

    assert_eq!(response["ok"], true, "{response:?}");
    assert_eq!(
        git_remote_origin(&repo).as_deref(),
        Some("https://example.com/repo.git")
    );
}

#[test]
fn fs_list_browses_dirs_and_flags_git_repos() {
    let (dir_a, repo_a) = init_repo();
    let mut state = AppState::new(
        repo_a,
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    // A browse root with a plain folder, a git repo, and a hidden folder.
    let root = dir_a.path().join("browse");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("plain")).unwrap();
    std::fs::create_dir(root.join(".hidden")).unwrap();
    let repo_dir = root.join("myrepo");
    std::fs::create_dir(&repo_dir).unwrap();
    std::fs::create_dir(repo_dir.join(".git")).unwrap();

    let res = state.handle(req("fs.list", json!({ "path": root.to_str().unwrap() })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert!(res["result"]["parent"].is_string());
    let entries = res["result"]["entries"].as_array().unwrap();
    assert_eq!(
        entries.iter().find(|e| e["name"] == "myrepo").unwrap()["is_git"],
        true
    );
    assert_eq!(
        entries.iter().find(|e| e["name"] == "plain").unwrap()["is_git"],
        false
    );
    // Hidden dirs are still returned, flagged so the client can toggle them.
    let hidden = entries.iter().find(|e| e["name"] == ".hidden").unwrap();
    assert_eq!(hidden["is_hidden"], true);
    assert_eq!(
        entries.iter().find(|e| e["name"] == "plain").unwrap()["is_hidden"],
        false
    );
}

#[test]
fn fs_mkdir_creates_one_plain_directory_and_rejects_unsafe_names() {
    let (dir_a, repo_a) = init_repo();
    let mut state = AppState::new(
        repo_a,
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let root = dir_a.path().join("browse");
    std::fs::create_dir(&root).unwrap();

    let created = state.handle(req(
        "fs.mkdir",
        json!({ "parent": root.to_str().unwrap(), "name": "new source" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(
        created["result"]["path"],
        root.join("new source").to_str().unwrap()
    );
    assert!(root.join("new source").is_dir());
    assert!(!root.join("new source/.git").exists());

    for name in ["", ".", "..", "nested/child", "nested\\child"] {
        let rejected = state.handle(req(
            "fs.mkdir",
            json!({ "parent": root.to_str().unwrap(), "name": name }),
        ));
        assert_eq!(rejected["ok"], false, "name {name:?}: {rejected:?}");
    }

    let duplicate = state.handle(req(
        "fs.mkdir",
        json!({ "parent": root.to_str().unwrap(), "name": "new source" }),
    ));
    assert_eq!(duplicate["ok"], false, "{duplicate:?}");
}

#[test]
fn settings_set_then_clone_registers_project() {
    let (dir_src, repo_src) = init_repo();
    let (dir_a, repo_a) = init_repo();
    let mut state = AppState::new(
        repo_a,
        dir_a.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    // Point the projects folder at a temp location.
    let projects_dir = dir_src.path().join("projects");
    let set = state.handle(req(
        "settings.set",
        json!({ "projects_dir": projects_dir.to_str().unwrap() }),
    ));
    assert_eq!(set["ok"], true, "{set:?}");
    assert!(
        state.handle(req("settings.get", json!({})))["result"]["projects_dir"]
            .as_str()
            .unwrap()
            .contains("projects")
    );

    // Clone the source repo into the projects folder and register it.
    let cloned = state.handle(req(
        "project.clone",
        json!({ "url": repo_src.to_str().unwrap() }),
    ));
    assert_eq!(cloned["ok"], true, "{cloned:?}");
    let clone_path = cloned["result"]["path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&clone_path).join("README.md").exists());
    assert_eq!(
        state.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Cloning the same repo again registers the existing checkout — no error, no
    // duplicate, same path (not a second clone).
    let again = state.handle(req(
        "project.clone",
        json!({ "url": repo_src.to_str().unwrap() }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(again["result"]["path"].as_str().unwrap(), clone_path);
    assert_eq!(
        state.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn config_persists_projects_and_dir_across_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir_a, repo_a) = init_repo();
    let (_dir_b, repo_b) = init_repo();
    let cfg = tmp.path().join("config.json");
    {
        let mut state = AppState::new(
            repo_a.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&cfg)
        .unwrap();
        state.handle(req(
            "project.add",
            json!({ "path": repo_b.to_str().unwrap() }),
        ));
        state.handle(req(
            "settings.set",
            json!({ "projects_dir": tmp.path().join("myprojects").to_str().unwrap() }),
        ));
    }
    // A fresh instance restores the added project and the chosen dir from disk.
    let mut reloaded = AppState::new(
        repo_a,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&cfg)
    .unwrap();
    assert_eq!(
        reloaded.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        reloaded.handle(req("settings.get", json!({})))["result"]["projects_dir"]
            .as_str()
            .unwrap()
            .contains("myprojects")
    );
}

#[test]
fn configured_project_ids_survive_gaps_and_keep_the_allocator_above_deleted_ids() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_directory, repo) = init_repo();
    let (_added_directory, added_repo) = init_repo();
    let config = directory.path().join("config.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "next_project": 9,
            "projects": [{ "id": "proj-3", "path": repo, "base_branch": "main" }]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    assert_eq!(state.projects.at(0).id, "proj-3");
    assert_eq!(state.add_project(added_repo, "main".to_string()), "proj-9");
    state.persist();
    let saved: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["projects"][0]["id"], "proj-3");
    assert_eq!(saved["next_project"], 10);
    let restored = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    assert_eq!(restored.projects.at(0).id, "proj-3");
    assert_eq!(restored.projects.at(1).id, "proj-9");
    assert_eq!(restored.projects.next_id(), 10);
}

/// A project's sources are not settled when it is opened. `project.add_source`
/// appends one the way the project was opened over the ones it has — the same
/// open, the same validation — and writes the config, so the next boot reads
/// the project with its new folder on it.
#[test]
fn add_source_appends_a_folder_to_a_project_and_writes_it_down() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_directory, repo) = init_repo();
    let extra = directory.path().join("assets");
    std::fs::create_dir(&extra).unwrap();
    std::fs::write(extra.join("logo.svg"), b"<svg/>").unwrap();
    let config = directory.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let mut state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let project_id = state.add_project(repo, "main".to_string());

    let added = state.handle(req(
        "project.add_source",
        json!({"project_id": project_id, "path": extra, "name": "assets"}),
    ));

    assert_eq!(added["ok"], true, "{added:?}");
    let sources = added["result"]["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2, "{added:?}");
    assert_eq!(sources[1]["name"], "assets");
    assert_eq!(sources[1]["is_git"], false);
    assert_ne!(sources[1]["id"], sources[0]["id"]);
    let saved: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["projects"][0]["sources"][1]["name"], "assets");
}

#[test]
fn add_source_refuses_a_project_this_bridge_does_not_have() {
    let directory = tempfile::tempdir().unwrap();
    let extra = directory.path().join("assets");
    std::fs::create_dir(&extra).unwrap();
    let mut state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );

    let refused = state.handle(req(
        "project.add_source",
        json!({"project_id": "proj-9", "path": extra}),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "not_found", "{refused:?}");
}

/// Removing a source is forward-looking: the project stops cutting that folder
/// into new workspaces, and the workspaces that already have it keep it.
#[test]
fn remove_source_drops_it_from_the_project_and_leaves_workspaces_alone() {
    let directory = tempfile::tempdir().unwrap();
    let repo = init_repo_named(directory.path(), "code");
    let extra = directory.path().join("assets");
    std::fs::create_dir(&extra).unwrap();
    let config = directory.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let mut state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let opened = state.handle(req(
        "project.create",
        json!({
            "name": "mixed",
            "sources": [{"name": "code", "path": repo}, {"name": "assets", "path": extra}],
        }),
    ));
    assert_eq!(opened["ok"], true, "{opened:?}");
    let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
    let created = state.handle(req(
        "workspace.create",
        json!({"project_id": project_id, "name": "work", "isolation": "worktree"}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let workspace_id = created["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let removed = state.handle(req(
        "project.remove_source",
        json!({"project_id": project_id, "source_id": "source-2"}),
    ));

    assert_eq!(removed["ok"], true, "{removed:?}");
    let sources = removed["result"]["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1, "{removed:?}");
    assert_eq!(sources[0]["id"], "source-1");
    assert!(extra.is_dir(), "the folder itself is not Build's to remove");
    let read = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(
        read["result"]["directories"].as_array().unwrap().len(),
        2,
        "an existing workspace keeps the directory it was cut with: {read:?}"
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["projects"][0]["sources"].as_array().unwrap().len(), 1);
}

#[test]
fn remove_source_refuses_a_source_the_project_does_not_have() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_directory, repo) = init_repo();
    let mut state = AppState::new_unrooted(
        directory.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.add_project(repo, "main".to_string());

    let refused = state.handle(req(
        "project.remove_source",
        json!({"project_id": project_id, "source_id": "source-9"}),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "not_found", "{refused:?}");
}
