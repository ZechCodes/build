use super::*;

/// Add a git worktree Build did not create, at `dir/name` on `branch`, cut
/// from `repo`'s current HEAD — the raw material of adoption tests.
pub(in crate::app::tests) fn add_external_worktree(
    repo: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
    branch: &str,
) -> PathBuf {
    let path = dir.join(name);
    assert!(Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "worktree",
            "add",
            path.to_str().unwrap(),
            "-b",
            branch,
        ])
        .status()
        .unwrap()
        .success());
    path
}

/// Whether the project repo still holds a local branch, asked the way
/// every caller asks it: through the project's own checkout seam.
pub(in crate::app::tests) fn local_branch_exists(
    repo: &std::path::Path,
    branch: &str,
) -> Result<bool, crate::worktree::WorktreeError> {
    WorktreeManager::new(repo, repo.join("worktrees")).branch_exists(branch)
}

#[test]
fn merge_cleanup_rejects_non_string_values_instead_of_pruning() {
    // Absent / null → Prune (backward-compat default).
    assert!(matches!(
        merge_cleanup_from(&json!({}), true),
        Ok(MergeCleanup::Prune)
    ));
    assert!(matches!(
        merge_cleanup_from(&json!({ "cleanup": null }), true),
        Ok(MergeCleanup::Prune)
    ));
    // A present-but-non-string value must fail fast — never silently collapse
    // to the destructive Prune default (spec: a mis-typed client must error).
    for bad in [
        json!(true),
        json!(3),
        json!({ "mode": "keep" }),
        json!(["keep"]),
    ] {
        let params = json!({ "cleanup": bad });
        let err = merge_cleanup_from(&params, true).unwrap_err();
        assert!(
            err.starts_with("invalid cleanup:"),
            "non-string cleanup must be rejected, got: {err}"
        );
    }
}
#[test]
fn fs_tree_lists_one_level_dirs_first_case_insensitive_and_skips_git() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    std::fs::create_dir(repo.join("Zdir")).unwrap();
    std::fs::create_dir(repo.join("adir")).unwrap();
    std::fs::write(repo.join("adir/nested.txt"), "nested\n").unwrap();
    std::fs::write(repo.join("B.txt"), "b\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();

    let res = state.handle(req("fs.tree", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["path"], "");
    let entries = res["result"]["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&".git"), "{names:?}");

    // dirs-first, each group case-insensitive.
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    let last_dir = kinds.iter().rposition(|k| *k == "dir");
    let first_file = kinds.iter().position(|k| *k == "file");
    if let (Some(last_dir), Some(first_file)) = (last_dir, first_file) {
        assert!(last_dir < first_file, "{kinds:?}");
    }
    let dir_names: Vec<&str> = entries
        .iter()
        .filter(|e| e["kind"] == "dir")
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(dir_names, vec!["adir", "Zdir"]);
    let file_names: Vec<&str> = entries
        .iter()
        .filter(|e| e["kind"] == "file")
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(file_names, vec!["a.txt", "B.txt", "README.md"]);
    let readme = entries.iter().find(|e| e["name"] == "README.md").unwrap();
    assert!(readme["size"].as_u64().unwrap() > 0);

    // One level only: nested.txt is not listed at the root.
    assert!(!names.contains(&"nested.txt"));

    // Recurse one level via `path`.
    let sub = state.handle(req(
        "fs.tree",
        json!({ "project_id": project_id, "path": "adir" }),
    ));
    assert_eq!(sub["ok"], true, "{sub:?}");
    assert_eq!(sub["result"]["path"], "adir");
    let sub_entries = sub["result"]["entries"].as_array().unwrap();
    assert_eq!(sub_entries.len(), 1);
    assert_eq!(sub_entries[0]["name"], "nested.txt");
    assert_eq!(sub_entries[0]["kind"], "file");
}

#[test]
fn fs_tree_rejects_escapes_and_non_directories() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    let escape = state.handle(req(
        "fs.tree",
        json!({ "project_id": project_id, "path": "../../../etc" }),
    ));
    assert_eq!(escape["ok"], false, "{escape:?}");
    assert!(
        escape["error"].as_str().unwrap().contains("escapes"),
        "{escape:?}"
    );

    let not_dir = state.handle(req(
        "fs.tree",
        json!({ "project_id": project_id, "path": "README.md" }),
    ));
    assert_eq!(not_dir["ok"], false, "{not_dir:?}");
    assert_eq!(not_dir["error"], "not a directory");
}

#[test]
fn fs_tree_rejects_a_symlinked_directory_escape() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    // A directory symlink inside the worktree pointing outside it: every
    // lexical component is Normal, so only canonical containment (the same
    // fence fs.read rides) can refuse listing through it.
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("linkdir")).unwrap();

    let escape = state.handle(req(
        "fs.tree",
        json!({ "project_id": project_id, "path": "linkdir" }),
    ));
    assert_eq!(escape["ok"], false, "{escape:?}");
    assert!(
        escape["error"].as_str().unwrap().contains("escapes"),
        "{escape:?}"
    );

    // Nested through the symlinked directory is refused the same way.
    let nested = state.handle(req(
        "fs.tree",
        json!({ "project_id": project_id, "path": "linkdir/sub" }),
    ));
    assert_eq!(nested["ok"], false, "{nested:?}");
}

#[test]
fn fs_read_round_trips_content_and_infers_mime() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    std::fs::write(repo.join("notes.md"), "# hi\n").unwrap();
    std::fs::write(repo.join("page.html"), "<h1>hi</h1>\n").unwrap();
    std::fs::write(repo.join("icon.svg"), "<svg></svg>\n").unwrap();
    std::fs::write(repo.join("pic.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
    std::fs::write(repo.join("sound.mp3"), b"ID3audio").unwrap();
    std::fs::write(repo.join("clip.mp4"), b"media").unwrap();
    std::fs::write(repo.join("plain.txt"), "just text\n").unwrap();
    std::fs::write(repo.join("blob.bin"), [0u8, 1, 2, 3, 0, 4]).unwrap();

    let mut read = |path: &str| {
        state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": path }),
        ))
    };

    let md = read("notes.md");
    assert_eq!(md["result"]["mime"], "text/markdown");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(md["result"]["content_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"# hi\n");
    assert_eq!(md["result"]["truncated"], false);
    assert_eq!(md["result"]["size"], 5);

    assert_eq!(read("page.html")["result"]["mime"], "text/html");
    assert_eq!(read("icon.svg")["result"]["mime"], "image/svg+xml");
    assert_eq!(read("pic.png")["result"]["mime"], "image/png");
    assert_eq!(read("sound.mp3")["result"]["mime"], "audio/mpeg");
    assert_eq!(read("clip.mp4")["result"]["mime"], "video/mp4");
    assert_eq!(read("plain.txt")["result"]["mime"], "text/plain");
    assert_eq!(
        read("blob.bin")["result"]["mime"],
        "application/octet-stream"
    );

    let missing = read("nope.txt");
    assert_eq!(missing["ok"], false, "{missing:?}");

    let dir_read = read("");
    assert_eq!(dir_read["ok"], false, "{dir_read:?}");
}

#[test]
fn fs_read_truncates_oversized_files() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    let real_size = FS_READ_MAX_BYTES as usize + 4096;
    std::fs::write(repo.join("big.bin"), vec![b'a'; real_size]).unwrap();
    std::fs::write(repo.join("clip.mp4"), vec![b'm'; real_size]).unwrap();

    let res = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id.clone(), "path": "big.bin" }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["size"], real_size as u64);
    assert_eq!(res["result"]["truncated"], true);
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(res["result"]["content_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded.len(), FS_READ_MAX_BYTES as usize);

    let media = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "clip.mp4" }),
    ));
    assert_eq!(media["result"]["mime"], "video/mp4");
    assert_eq!(media["result"]["truncated"], false);
    let media_decoded = base64::engine::general_purpose::STANDARD
        .decode(media["result"]["content_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(media_decoded.len(), real_size);
}

#[test]
fn fs_read_rejects_lexical_and_symlink_escapes() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();

    // Lexical escape: caught before any filesystem access.
    let lexical = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "../../../etc/passwd" }),
    ));
    assert_eq!(lexical["ok"], false, "{lexical:?}");
    assert!(
        lexical["error"].as_str().unwrap().contains("escapes"),
        "{lexical:?}"
    );

    // Symlink leaf pointing inside the root: still refused (leaf check,
    // regardless of target).
    std::os::unix::fs::symlink(repo.join("README.md"), repo.join("inside-link")).unwrap();
    let inside_link = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "inside-link" }),
    ));
    assert_eq!(inside_link["ok"], false, "{inside_link:?}");
    assert!(
        inside_link["error"].as_str().unwrap().contains("symlink"),
        "{inside_link:?}"
    );

    // Symlinked directory pointing outside the root: canonical containment
    // catches it even though every lexical component is Normal.
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("linkdir")).unwrap();
    let dir_escape = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "linkdir/secret.txt" }),
    ));
    assert_eq!(dir_escape["ok"], false, "{dir_escape:?}");
    assert!(
        dir_escape["error"].as_str().unwrap().contains("escapes"),
        "{dir_escape:?}"
    );
}
#[test]
fn project_diff_shape_and_unknown_project() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.projects[0].id.clone();
    std::fs::write(repo.join("uncommitted.txt"), "dirty\n").unwrap();

    let res = state.handle(req("project.diff", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(res["result"]["project_id"], project_id);
    assert_eq!(res["result"]["branch"], "main");
    assert!(res["result"]["path"].as_str().unwrap().contains("repo"));
    assert!(res["result"]["stat"]["files_changed"].as_u64().unwrap() >= 1);
    let files = res["result"]["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f["path"] == "uncommitted.txt"));
    assert!(res["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("uncommitted.txt"));

    let unknown = state.handle(req("project.diff", json!({ "project_id": "proj-99" })));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert_eq!(unknown["error"], "unknown project_id");
}
