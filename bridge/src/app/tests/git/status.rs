use super::*;

/// A repo initialized on `main` but with no commits yet (unborn HEAD).
fn init_unborn_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    crate::git_fixture::configure_repo(&repo);
    (dir, repo)
}

pub(in crate::app::tests) fn git_gui_state(
    dir: &tempfile::TempDir,
    repo: &std::path::Path,
) -> AppState {
    AppState::new(
        repo.to_path_buf(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
}

/// The `files` entry for `path` in a git.status-shaped payload.
pub(in crate::app::tests) fn file_entry<'a>(status: &'a Value, path: &str) -> &'a Value {
    status["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == json!(path))
        .unwrap_or_else(|| panic!("no {path} in {status:?}"))
}

pub(in crate::app::tests) fn has_file_entry(status: &Value, path: &str) -> bool {
    status["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == json!(path))
}

#[test]
fn git_log_on_an_unborn_head_reports_the_branch_and_no_commits() {
    let (dir, repo) = init_unborn_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.log", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["branch"], "main");
    assert_eq!(res["result"]["commits"].as_array().unwrap().len(), 0);
    assert_eq!(res["result"]["more"], false);
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: git_show_shapes_a_commit_and_its_root_parent is at 17, threshold 15 — bring it under, then remove
fn git_show_shapes_a_commit_and_its_root_parent() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    git_in(&repo, &["add", "a.txt"]);
    git_in(&repo, &["commit", "-m", "subject line", "-m", "body text"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let log = state.handle(req("git.log", json!({ "project_id": project_id })));
    let commits = log["result"]["commits"].as_array().unwrap().clone();
    let top_hash = commits[0]["hash"].as_str().unwrap().to_string();
    let root_hash = commits[1]["hash"].as_str().unwrap().to_string();

    let shown = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": top_hash }),
    ));
    assert_eq!(shown["ok"], true, "{shown:?}");
    assert_eq!(shown["result"]["hash"], top_hash.as_str());
    assert_eq!(shown["result"]["short"], top_hash[..7]);
    assert_eq!(shown["result"]["subject"], "subject line");
    assert_eq!(shown["result"]["body"], "body text");
    assert_eq!(shown["result"]["author"], "Test");
    assert_eq!(shown["result"]["email"], "test@build.ing");
    assert!(shown["result"]["time"].as_i64().unwrap() > 0);
    assert_eq!(shown["result"]["stat"]["files_changed"], 1);
    assert_eq!(shown["result"]["stat"]["insertions"], 1);
    assert_eq!(shown["result"]["stat"]["deletions"], 0);
    assert!(shown["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("+hello"));
    assert_eq!(shown["result"]["truncated"], false);

    // A short (7-char) prefix resolves to the same commit.
    let by_prefix = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": top_hash[..7] }),
    ));
    assert_eq!(by_prefix["result"]["hash"], top_hash.as_str());

    // The root commit diffs against the empty tree.
    let root = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": root_hash }),
    ));
    assert_eq!(root["ok"], true, "{root:?}");
    assert!(root["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("+# project"));
}

#[test]
fn git_show_rejects_malformed_and_unknown_hashes() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    for bad in ["HEAD", "abc", "ABCDEF12", "main", "deadbeef^", ""] {
        let res = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": bad }),
        ));
        assert_eq!(res["ok"], false, "hash {bad:?} must be rejected: {res:?}");
    }

    let unknown = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": "ffffffffff" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
}

#[test]
fn git_show_truncates_an_oversized_patch() {
    let (dir, repo) = init_repo();
    let line_count = 80_000; // ~1.36 MiB of "+…" patch lines, over the 1 MiB cap
    let big: String = "0123456789abcdef\n".repeat(line_count);
    std::fs::write(repo.join("big.txt"), &big).unwrap();
    git_in(&repo, &["add", "big.txt"]);
    git_in(&repo, &["commit", "-m", "big"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "limit": 1 }),
    ));
    let hash = log["result"]["commits"][0]["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let shown = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": hash }),
    ));
    assert_eq!(shown["ok"], true, "{shown:?}");
    assert_eq!(shown["result"]["truncated"], true);
    assert!(shown["result"]["patch"].as_str().unwrap().len() <= 1_048_576);
    // The stat stays exact even though the patch degraded.
    assert_eq!(shown["result"]["stat"]["insertions"], line_count);
}

#[test]
fn git_log_and_show_cap_oversized_commit_messages() {
    let (dir, repo) = init_repo();
    // A prompt-injected agent can craft a multi-MB message with one
    // `git commit -F`; the display strings must degrade, not ride the
    // poll past the relay frame cap.
    let subject = "s".repeat(1_048_576);
    let body = "b".repeat(2 * 1_048_576);
    let msg_file = dir.path().join("msg.txt");
    std::fs::write(&msg_file, format!("{subject}\n\n{body}")).unwrap();
    std::fs::write(repo.join("x.txt"), "x\n").unwrap();
    git_in(&repo, &["add", "x.txt"]);
    git_in(&repo, &["commit", "-q", "-F", msg_file.to_str().unwrap()]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "limit": 1 }),
    ));
    assert_eq!(log["ok"], true, "{log:?}");
    let entry = &log["result"]["commits"][0];
    assert_eq!(entry["subject"].as_str().unwrap().len(), 512);
    let hash = entry["hash"].as_str().unwrap().to_string();

    let shown = state.handle(req(
        "git.show",
        json!({ "project_id": project_id, "hash": hash }),
    ));
    assert_eq!(shown["ok"], true, "{shown:?}");
    assert_eq!(shown["result"]["subject"].as_str().unwrap().len(), 512);
    assert_eq!(shown["result"]["body"].as_str().unwrap().len(), 65_536);
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: git_status_reports_tristate_staging_and_excludes_the_mcp_config is at 16, threshold 15 — bring it under, then remove
fn git_status_reports_tristate_staging_and_excludes_the_mcp_config() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // full: a new file, staged, with no further worktree edits.
    std::fs::write(repo.join("full.txt"), "staged\n").unwrap();
    git_in(&repo, &["add", "full.txt"]);
    // partial: staged edits AND later worktree edits on the same path.
    std::fs::write(repo.join("README.md"), "# project\nstaged edit\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    std::fs::write(
        repo.join("README.md"),
        "# project\nstaged edit\nunstaged edit\n",
    )
    .unwrap();
    // none: untracked.
    std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();
    // Machine-local scaffolding never surfaces.
    std::fs::create_dir_all(repo.join(".build")).unwrap();
    std::fs::write(repo.join(".build/mcp.json"), "{}\n").unwrap();

    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let status = &res["result"];
    assert_eq!(status["branch"], "main");
    assert!(status["path"].as_str().unwrap().contains("repo"));
    assert_eq!(status["head"].as_str().unwrap().len(), 40);

    let full = file_entry(status, "full.txt");
    assert_eq!(full["staged"], "full");
    assert_eq!(full["index_status"], "A");
    assert!(full["edited_at"].as_u64().unwrap() > 0);
    let partial = file_entry(status, "README.md");
    assert_eq!(partial["staged"], "partial");
    assert_eq!(partial["index_status"], "M");
    assert_eq!(partial["worktree_status"], "M");
    let untracked = file_entry(status, "loose.txt");
    assert_eq!(untracked["staged"], "none");
    assert_eq!(untracked["index_status"], "?");
    assert_eq!(untracked["worktree_status"], "?");
    assert!(!has_file_entry(status, ".build/mcp.json"));

    assert_eq!(status["stat"]["files_changed"], 3);
    assert_eq!(status["files_truncated"], false);
}

/// The poll's cheap turn: a browser that names the key it holds is told
/// only that it still holds it, and hears the whole shape the moment the
/// working tree moves under it.
#[test]
fn a_held_status_key_answers_unchanged_over_the_wire() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();

    let first = state.handle(req("git.status", json!({ "project_id": project_id })));
    let held = first["result"]["status_key"].as_str().unwrap().to_string();

    let unchanged = state.handle(req(
        "git.status",
        json!({ "project_id": project_id, "if_status_key": held }),
    ));
    assert_eq!(
        unchanged["result"],
        json!({ "unchanged": true, "status_key": held })
    );

    std::fs::write(repo.join("loose.txt"), "loose and then some\n").unwrap();
    let moved = state.handle(req(
        "git.status",
        json!({ "project_id": project_id, "if_status_key": held }),
    ));
    assert_ne!(moved["result"]["status_key"].as_str().unwrap(), held);
    assert_eq!(file_entry(&moved["result"], "loose.txt")["added"], 1);
}

/// The per-file census the browser draws a row from before it asks for any
/// body: how many lines moved, whether there is a body worth asking for,
/// and the key that says a held body is still the current one.
#[test]
fn git_status_carries_a_content_key_and_counts_per_file() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("gone.txt"), "one\ntwo\n").unwrap();
    git_in(&repo, &["add", "gone.txt"]);
    git_in(&repo, &["commit", "-q", "-m", "fixture"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    std::fs::write(repo.join("README.md"), "# project\nsecond\n").unwrap();
    std::fs::write(repo.join("logo.bin"), [0u8, 1, 2, 0, 255, b'\n']).unwrap();

    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let status = &res["result"];

    let edited = file_entry(status, "README.md");
    assert_eq!(edited["added"], 1);
    assert_eq!(edited["deleted"], 0);
    assert_eq!(edited["binary"], false);
    assert!(!edited["content_key"].as_str().unwrap().is_empty());
    assert_eq!(file_entry(status, "gone.txt")["content_key"], "deleted");
    assert_eq!(file_entry(status, "logo.bin")["binary"], true);
    assert!(status.get("patch").is_none(), "{status}");
    assert_eq!(status["status_key"].as_str().unwrap().len(), 16);
}

#[test]
fn git_status_surfaces_merge_conflicts_as_u_entries() {
    let (dir, repo) = init_repo();
    // A real content conflict: two branches editing the same line.
    git_in(&repo, &["checkout", "-q", "-b", "side"]);
    std::fs::write(repo.join("README.md"), "# side\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    git_in(&repo, &["commit", "-q", "-m", "side edit"]);
    git_in(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("README.md"), "# main\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    git_in(&repo, &["commit", "-q", "-m", "main edit"]);
    let merge = Command::new("git")
        .args(["merge", "side"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(!merge.status.success(), "merge must conflict");
    // A clean untracked file must still classify as before.
    std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();

    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");

    let conflicted = file_entry(&res["result"], "README.md");
    assert_eq!(conflicted["index_status"], "U");
    assert_eq!(conflicted["worktree_status"], "U");
    assert_eq!(conflicted["staged"], "none");

    let untracked = file_entry(&res["result"], "loose.txt");
    assert_eq!(untracked["staged"], "none");
    assert_eq!(untracked["index_status"], "?");
    assert_eq!(untracked["worktree_status"], "?");
}

#[test]
fn git_status_decomposes_a_staged_rename_into_delete_plus_add() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    git_in(&repo, &["mv", "README.md", "RENAMED.md"]);

    // Both sides of the rename appear, matching the patch (which has no
    // rename detection): a staged delete at the old path, a staged add at
    // the new one. index_status "R" never occurs.
    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let old = file_entry(&res["result"], "README.md");
    assert_eq!(old["index_status"], "D");
    assert_eq!(old["staged"], "full");
    let new = file_entry(&res["result"], "RENAMED.md");
    assert_eq!(new["index_status"], "A");
    assert_eq!(new["staged"], "full");
}

/// `git.diff` is where a body comes from now that `git.status` carries
/// only shape: the patch of the asked path, under the key the status shape
/// gave it, and an error for a path it may not read.
#[test]
fn git_diff_answers_the_asked_paths_body_under_its_status_key() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();

    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    let key = file_entry(&status["result"], "loose.txt")["content_key"].clone();

    let res = state.handle(req(
        "git.diff",
        json!({ "project_id": project_id, "paths": ["loose.txt"] }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");
    let file = &res["result"]["files"][0];
    assert_eq!(file["path"], "loose.txt");
    assert_eq!(file["content_key"], key);
    assert!(file["patch"].as_str().unwrap().contains("+loose"));
    assert_eq!(file["truncated"], false);

    for bad in [json!(["../evil"]), json!([".build/mcp.json"]), json!([])] {
        let refused = state.handle(req(
            "git.diff",
            json!({ "project_id": project_id, "paths": bad }),
        ));
        assert_eq!(refused["ok"], false, "paths {bad:?}: {refused:?}");
    }
    let missing = state.handle(req("git.diff", json!({ "project_id": project_id })));
    assert_eq!(missing["ok"], false, "{missing:?}");
}

#[test]
fn git_status_on_an_unborn_head_has_a_null_head() {
    let (dir, repo) = init_unborn_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("first.txt"), "hello\n").unwrap();

    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["branch"], "main");
    assert!(res["result"]["head"].is_null());
    let untracked = file_entry(&res["result"], "first.txt");
    assert_eq!(untracked["staged"], "none");
    assert_eq!(untracked["index_status"], "?");
    assert_eq!(untracked["added"], 1);
}

#[test]
fn git_stage_answers_with_the_fresh_status() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("work.txt"), "work\n").unwrap();

    let staged = state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": ["work.txt"] }),
    ));
    assert_eq!(staged["ok"], true, "{staged:?}");
    // The response IS the fresh status payload.
    let entry = file_entry(&staged["result"], "work.txt");
    assert_eq!(entry["staged"], "full");
    assert_eq!(entry["index_status"], "A");
}

#[test]
fn git_stage_rejects_paths_that_escape_the_worktree() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("ok.txt"), "ok\n").unwrap();

    for bad in ["../../../etc/passwd", "/etc/passwd", "./ok.txt", ""] {
        let res = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["ok.txt", bad] }),
        ));
        assert_eq!(res["ok"], false, "path {bad:?} must be rejected: {res:?}");
    }
    // One bad path failed the whole request: nothing got staged.
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(file_entry(&status["result"], "ok.txt")["staged"], "none");

    // paths is required and must be a non-empty array of strings.
    let missing = state.handle(req("git.stage", json!({ "project_id": project_id })));
    assert_eq!(missing["ok"], false);
    let empty = state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": [] }),
    ));
    assert_eq!(empty["ok"], false);
}

#[test]
fn git_stage_treats_paths_as_literals_never_globs() {
    // A file literally named "*" alongside an innocent bystander: staging
    // "*" must stage only that file, never glob-expand.
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("*"), "star\n").unwrap();
    std::fs::write(repo.join("bystander.txt"), "hi\n").unwrap();

    let staged = state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": ["*"] }),
    ));
    assert_eq!(staged["ok"], true, "{staged:?}");
    assert_eq!(file_entry(&staged["result"], "*")["staged"], "full");
    assert_eq!(
        file_entry(&staged["result"], "bystander.txt")["staged"],
        "none"
    );

    // Without a file actually named "*", the request errors instead of
    // matching everything.
    let (dir2, repo2) = init_repo();
    let mut state2 = git_gui_state(&dir2, &repo2);
    let project_id2 = state2.project_at(0).id.clone();
    std::fs::write(repo2.join("bystander.txt"), "hi\n").unwrap();

    let res = state2.handle(req(
        "git.stage",
        json!({ "project_id": project_id2, "paths": ["*"] }),
    ));
    assert_eq!(res["ok"], false, "{res:?}");
    let status = state2.handle(req("git.status", json!({ "project_id": project_id2 })));
    assert_eq!(
        file_entry(&status["result"], "bystander.txt")["staged"],
        "none"
    );
}

#[test]
fn git_stage_silently_drops_the_mcp_config() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::create_dir_all(repo.join(".build")).unwrap();
    std::fs::write(repo.join(".build/mcp.json"), "{}\n").unwrap();

    // The list collapses to empty → a successful no-op.
    let res = state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": [".build/mcp.json"] }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");
    assert!(!has_file_entry(&res["result"], ".build/mcp.json"));
}

#[test]
fn git_commit_commits_only_what_is_staged() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    std::fs::write(repo.join("README.md"), "# project\nunstaged edit\n").unwrap();
    state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": ["staged.txt"] }),
    ));

    let res = state.handle(req(
        "git.commit",
        json!({ "project_id": project_id, "message": "add staged file" }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["hash"].as_str().unwrap().len(), 40);
    assert_eq!(res["result"]["subject"], "add staged file");
    let hash = res["result"]["hash"].as_str().unwrap();
    assert_eq!(res["result"]["short"], hash[..7]);

    // The unstaged edit survived, uncommitted; the staged file is gone
    // from status.
    let status = &res["result"]["status"];
    assert!(!has_file_entry(status, "staged.txt"), "{status:?}");
    assert_eq!(file_entry(status, "README.md")["staged"], "none");
    assert_eq!(status["head"], json!(hash));

    // And the commit is on top of the log.
    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id, "limit": 1 }),
    ));
    assert_eq!(log["result"]["commits"][0]["subject"], "add staged file");
}

#[test]
fn git_commit_rejects_empty_messages_and_an_empty_stage() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    state.handle(req(
        "git.stage",
        json!({ "project_id": project_id, "paths": ["staged.txt"] }),
    ));

    for empty in ["", "   \n\t"] {
        let res = state.handle(req(
            "git.commit",
            json!({ "project_id": project_id, "message": empty }),
        ));
        assert_eq!(res["ok"], false, "{res:?}");
        assert_eq!(res["error"], "commit message must not be empty");
    }

    // Drain the stage, then a commit has nothing to do.
    git_in(&repo, &["rm", "--cached", "-q", "staged.txt"]);
    let nothing = state.handle(req(
        "git.commit",
        json!({ "project_id": project_id, "message": "msg" }),
    ));
    assert_eq!(nothing["ok"], false, "{nothing:?}");
    assert_eq!(nothing["error"], "nothing staged to commit");
}
