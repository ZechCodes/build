use super::*;
use crate::app::fs::FS_MEDIA_READ_MAX_BYTES;
use base64::Engine as _;

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
    let project_id = state.project_at(0).id.clone();

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
    let project_id = state.project_at(0).id.clone();

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
    let project_id = state.project_at(0).id.clone();

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
    let project_id = state.project_at(0).id.clone();

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
fn fs_read_marks_complete_utf8_text_editable_with_its_revision() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("notes.md"), "# hi\n").unwrap();
    std::fs::write(repo.join("blob.bin"), [0_u8, 1, 2]).unwrap();

    let text = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "notes.md" }),
    ));
    assert_eq!(text["result"]["editable"], true);
    assert_eq!(text["result"]["encoding"], "utf-8");
    assert_eq!(text["result"]["revision"], sha256_hex(b"# hi\n"));

    let binary = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "blob.bin" }),
    ));
    assert_eq!(binary["result"]["editable"], false);
    assert!(binary["result"]["encoding"].is_null());
}

#[test]
fn fs_write_replaces_text_when_revision_matches_and_refuses_stale_writes() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let path = repo.join("notes.md");
    std::fs::write(&path, "before\n").unwrap();

    let initial = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "notes.md" }),
    ));
    let revision = initial["result"]["revision"].as_str().unwrap();
    let updated = state.handle(req(
        "fs.write",
        json!({
            "project_id": project_id,
            "path": "notes.md",
            "content_b64": b64encode(b"after\n"),
            "expected_revision": revision,
        }),
    ));
    assert_eq!(updated["ok"], true, "{updated:?}");
    assert_eq!(updated["result"]["editable"], true);
    assert_eq!(updated["result"]["encoding"], "utf-8");
    assert_eq!(updated["result"]["revision"], sha256_hex(b"after\n"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");

    let stale = state.handle(req(
        "fs.write",
        json!({
            "project_id": project_id,
            "path": "notes.md",
            "content_b64": b64encode(b"lost\n"),
            "expected_revision": revision,
        }),
    ));
    assert_eq!(stale["ok"], false, "{stale:?}");
    assert!(stale["error"]
        .as_str()
        .unwrap()
        .contains("revision conflict"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "after\n");
}

#[test]
fn fs_write_rejects_binary_oversized_invalid_utf8_and_symlinks() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("binary.bin"), [0, 1, 2]).unwrap();
    std::os::unix::fs::symlink(repo.join("README.md"), repo.join("link.txt")).unwrap();

    let write = |state: &mut AppState, path: &str, content: Vec<u8>, revision: &str| {
        state.handle(req(
            "fs.write",
            json!({
                "project_id": project_id,
                "path": path,
                "content_b64": b64encode(&content),
                "expected_revision": revision,
            }),
        ))
    };

    for (path, content, revision) in [
        ("binary.bin", b"text\n".to_vec(), sha256_hex(&[0, 1, 2])),
        ("README.md", vec![0xff], sha256_hex(b"# Test Repo\n")),
        (
            "README.md",
            vec![b'x'; FS_READ_MAX_BYTES as usize + 1],
            sha256_hex(b"# Test Repo\n"),
        ),
        ("link.txt", b"text\n".to_vec(), sha256_hex(b"# Test Repo\n")),
    ] {
        let response = write(&mut state, path, content, &revision);
        assert_eq!(response["ok"], false, "{path}: {response:?}");
    }
    assert_eq!(std::fs::read(repo.join("binary.bin")).unwrap(), [0, 1, 2]);
}

#[test]
fn fs_write_uses_the_same_worktree_for_external_and_run_scopes() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let worktree = add_external_worktree(&repo, dir.path(), "file-edit", "file-edit");
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|item| item.branch.as_deref() == Some("file-edit"))
        .unwrap()
        .id;
    std::fs::write(worktree.join("notes.txt"), "one\n").unwrap();

    let external_scope = json!({
        "project_id": project_id,
        "worktree_id": worktree_id,
        "path": "notes.txt",
    });
    let external_read = state.handle(req("fs.read", external_scope.clone()));
    let mut external_write = external_scope.clone();
    external_write["content_b64"] = json!(b64encode(b"two\n"));
    external_write["expected_revision"] = external_read["result"]["revision"].clone();
    assert_eq!(state.handle(req("fs.write", external_write))["ok"], true);

    let adopted = state.handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);
    let run_scope = json!({ "run_id": run_id, "path": "notes.txt" });
    let run_read = state.handle(req("fs.read", run_scope.clone()));
    assert_eq!(run_read["ok"], true, "{run_read:?}");
    let mut run_write = run_scope;
    run_write["content_b64"] = json!(b64encode(b"three\n"));
    run_write["expected_revision"] = run_read["result"]["revision"].clone();
    assert_eq!(state.handle(req("fs.write", run_write))["ok"], true);
    assert_eq!(
        std::fs::read_to_string(worktree.join("notes.txt")).unwrap(),
        "three\n"
    );
}

#[test]
fn fs_create_directory_creates_root_and_nested_in_project_source() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project = state.project_at(0);
    let scope = json!({ "project_id": project.id, "source_id": project.sources[0].id });
    for (parent, name, path) in [
        ("", "uploads", "uploads"),
        ("uploads", "nested", "uploads/nested"),
    ] {
        let mut params = scope.clone();
        params["parent"] = json!(parent);
        params["name"] = json!(name);
        let response = state.handle(req("fs.createDirectory", params));
        assert_eq!(response["ok"], true, "{response:?}");
        assert_eq!(response["result"], json!({ "path": path }));
        assert!(repo.join(path).is_dir());
    }
}

#[test]
fn fs_create_directory_refuses_invalid_existing_and_symlinked_paths() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    std::fs::create_dir(repo.join("existing")).unwrap();
    std::os::unix::fs::symlink(repo.join("existing"), repo.join("linked")).unwrap();
    for (parent, name) in [
        ("..", "outside"),
        ("/tmp", "outside"),
        ("", "../outside"),
        ("", "/tmp/outside"),
        ("", ""),
        ("", "."),
        ("", ".git"),
        (".git", "objects-new"),
        ("linked", "child"),
        ("existing/.", "child"),
        ("existing//child", "child"),
        ("", "bad\\name"),
        ("", "bad\0name"),
        ("README.md", "child"),
        ("missing", "child"),
    ] {
        let response = state.handle(req(
            "fs.createDirectory",
            json!({
                "project_id": project_id, "parent": parent, "name": name,
            }),
        ));
        assert_eq!(response["ok"], false, "{parent}/{name}: {response:?}");
        assert_eq!(response["error_code"], "invalid_params", "{response:?}");
    }
    let response = state.handle(req(
        "fs.createDirectory",
        json!({
            "project_id": project_id, "parent": "", "name": "existing",
        }),
    ));
    assert_eq!(response["error_code"], "already_exists", "{response:?}");
    assert!(!repo.join("existing/child").exists());
}

#[test]
fn fs_create_directory_selects_one_workspace_source() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let sources = [crate::workspace::WorkspaceSource {
        id: "source-1".into(),
        name: "files".into(),
        mount: "files".into(),
        path: repo.clone(),
        is_git: false,
        base_branch: "main".into(),
    }];
    let workspace = state
        .workspaces
        .begin(&project_id, "new-files", &sources)
        .unwrap();
    let target = workspace.directories[0].path.clone();
    std::fs::create_dir_all(&target).unwrap();
    let response = state.handle(req(
        "fs.createDirectory",
        json!({
            "workspace_id": workspace.id, "source_id": "source-1", "parent": "", "name": "assets",
        }),
    ));
    assert_eq!(response["ok"], true, "{response:?}");
    assert!(target.join("assets").is_dir());
    assert!(!repo.join("assets").exists());
}

fn upload_call(state: &mut AppState, sender: &SessionSender, method: &str, params: Value) -> Value {
    crate::api::v1::changes::with_session(sender, || state.handle(req(method, params)))
}

#[test]
fn fs_upload_begin_stages_private_file_and_checks_size_and_destination() {
    use std::os::unix::fs::PermissionsExt as _;
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let sender = SessionSender::detached("uploader");
    std::fs::create_dir(repo.join("assets")).unwrap();
    let params =
        json!({ "project_id": project_id, "parent": "assets", "name": "new.bin", "size": 10 });
    let begun = upload_call(&mut state, &sender, "fs.uploadBegin", params.clone());
    assert_eq!(begun["ok"], true, "{begun:?}");
    assert_eq!(begun["result"]["path"], "assets/new.bin");
    assert_eq!(begun["result"]["chunk_bytes"], 4 * 1_048_576);
    let id = begun["result"]["upload_id"].as_str().unwrap();
    let temp = repo.join("assets").join(format!(".build-upload-{id}.part"));
    assert_eq!(
        std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!repo.join("assets/new.bin").exists());
    let mut oversized = params.clone();
    oversized["size"] = json!(256 * 1_048_576u64 + 1);
    assert_eq!(
        upload_call(&mut state, &sender, "fs.uploadBegin", oversized)["error_code"],
        "invalid_params"
    );
    std::fs::write(repo.join("assets/new.bin"), b"original").unwrap();
    assert_eq!(
        upload_call(&mut state, &sender, "fs.uploadBegin", params.clone())["error_code"],
        "already_exists"
    );
    let mut replacing = params;
    replacing["replace"] = json!(true);
    assert_eq!(
        upload_call(&mut state, &sender, "fs.uploadBegin", replacing)["ok"],
        true
    );
    assert_eq!(
        std::fs::read(repo.join("assets/new.bin")).unwrap(),
        b"original"
    );
    drop(state);
    assert!(
        !temp.exists(),
        "dropping app state cleans unfinished uploads"
    );
    assert_eq!(std::fs::read_dir(repo.join("assets")).unwrap().count(), 1);
}

#[test]
fn fs_upload_begin_refuses_traversal_git_symlinks_and_non_files() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let sender = SessionSender::detached("uploader");
    std::fs::create_dir(repo.join("assets")).unwrap();
    std::os::unix::fs::symlink(repo.join("assets"), repo.join("linked")).unwrap();
    std::os::unix::fs::symlink(repo.join("README.md"), repo.join("leaf-link")).unwrap();
    for (parent, name) in [
        ("..", "new.bin"),
        ("/tmp", "new.bin"),
        ("", "../new.bin"),
        ("", ".git"),
        (".git", "new.bin"),
        ("linked", "new.bin"),
        ("", "leaf-link"),
        ("", "assets"),
    ] {
        let response = upload_call(
            &mut state,
            &sender,
            "fs.uploadBegin",
            json!({
                "project_id": project_id, "parent": parent, "name": name, "size": 1, "replace": true,
            }),
        );
        assert_eq!(
            response["error_code"], "invalid_params",
            "{parent}/{name}: {response:?}"
        );
    }
    assert_eq!(std::fs::read_dir(repo.join("assets")).unwrap().count(), 0);
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
    let project_id = state.project_at(0).id.clone();

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
    assert_eq!(res["result"]["editable"], false);
    assert!(res["result"]["revision"].is_null());
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
fn fs_read_media_raw_pages_keep_every_byte_and_bound_file_size() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let mut bytes = vec![b'x'; 5000];
    bytes[2] = b'\n';
    bytes[3] = 0xff;
    std::fs::write(repo.join("short.webm"), &bytes).unwrap();
    let raw = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "short.webm",
            "range": { "offset": 0, "bytes": 4096, "raw": true } }),
    ));
    assert_eq!(raw["ok"], true, "{raw:?}");
    assert_eq!(raw["result"]["range"]["end"], 4096);
    assert_eq!(
        b64decode(raw["result"]["content_b64"].as_str().unwrap()).unwrap(),
        bytes[..4096]
    );
    std::fs::write(repo.join("large.png"), &bytes).unwrap();
    let image = state.handle(req(
        "fs.read",
        json!({
            "project_id": project_id, "path": "large.png",
            "range": { "offset": 0, "bytes": 4096, "raw": true },
        }),
    ));
    assert_eq!(image["ok"], true, "{image:?}");
    assert_eq!(image["result"]["range"]["end"], 4096);
    std::fs::write(repo.join("note.txt"), &bytes).unwrap();
    let text = state.handle(req(
        "fs.read",
        json!({
            "project_id": project_id, "path": "note.txt",
            "range": { "offset": 0, "bytes": 4096, "raw": true },
        }),
    ));
    assert_eq!(text["error_code"], "invalid_params", "{text:?}");

    let forty_megabytes = 40 * 1_048_576u64;
    let large = std::fs::File::create(repo.join("large.mp4")).unwrap();
    large.set_len(forty_megabytes).unwrap();
    let page = state.handle(req(
        "fs.read",
        json!({
            "project_id": project_id, "path": "large.mp4",
            "range": { "offset": 34 * 1_048_576, "bytes": 1_048_576, "raw": true },
        }),
    ));
    assert_eq!(page["ok"], true, "{page:?}");
    assert_eq!(page["result"]["range"]["offset"], 34 * 1_048_576);
    assert_eq!(page["result"]["range"]["end"], 35 * 1_048_576);

    let too_large = std::fs::File::create(repo.join("too-large.mp4")).unwrap();
    too_large.set_len(FS_MEDIA_READ_MAX_BYTES + 1).unwrap();
    let oversized_page = state.handle(req(
        "fs.read",
        json!({
            "project_id": project_id, "path": "too-large.mp4",
            "range": { "offset": 0, "bytes": 1_048_576, "raw": true },
        }),
    ));
    assert_eq!(oversized_page["ok"], true, "{oversized_page:?}");
    assert_eq!(
        oversized_page["result"]["size"],
        FS_MEDIA_READ_MAX_BYTES + 1
    );
    assert_eq!(oversized_page["result"]["truncated"], true);
    assert_eq!(oversized_page["result"]["content_b64"], "");
}

/// One page of `big.log` from `offset`: its bytes and its `range`, checked
/// against what every page of a `size`-byte file answers.
fn read_file_page(
    state: &mut AppState,
    project_id: &str,
    offset: u64,
    size: u64,
) -> (Vec<u8>, Value) {
    let res = state.handle(req(
        "fs.read",
        json!({
            "project_id": project_id,
            "path": "big.log",
            "range": { "offset": offset, "bytes": 262_144 },
        }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");
    let result = &res["result"];
    assert_eq!(result["size"], size);
    assert_eq!(result["truncated"], false);
    assert_eq!(result["editable"], false);
    assert_eq!(result["mime"], "text/plain");
    assert!(result["revision"].is_null());
    let page = base64::engine::general_purpose::STANDARD
        .decode(result["content_b64"].as_str().unwrap())
        .unwrap();
    assert!(page.len() <= 262_144);
    let range = result["range"].clone();
    assert_eq!(range["offset"], offset);
    assert_eq!(range["total"], size);
    assert_eq!(range["end"], offset + page.len() as u64);
    (page, range)
}

/// The pages of a file, read on from each `end`, are the file (#95): whole
/// lines each, past the whole-read cap, and a page is never truncated.
#[test]
fn fs_read_answers_a_range_in_pages_of_whole_lines() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let body: String = (0..200_000)
        .map(|line| format!("line {line:06}\n"))
        .collect();
    assert!(body.len() as u64 > FS_READ_MAX_BYTES);
    std::fs::write(repo.join("big.log"), &body).unwrap();

    let mut offset = 0u64;
    let mut joined = Vec::new();
    let mut versions = std::collections::BTreeSet::new();
    loop {
        let (page, range) = read_file_page(&mut state, &project_id, offset, body.len() as u64);
        versions.insert(range["version"].as_str().unwrap().to_string());
        joined.extend_from_slice(&page);
        offset = range["end"].as_u64().unwrap();
        if offset == body.len() as u64 {
            break;
        }
        assert!(
            page.ends_with(b"\n"),
            "a page that is not the last ends a line"
        );
    }
    assert_eq!(joined, body.as_bytes());
    assert_eq!(versions.len(), 1, "every page names the same file");
}

/// A range past the file's end is the empty last page, and the version moves
/// when the file does, so a client knows its pages no longer belong together.
#[test]
fn fs_read_range_names_the_file_version_and_answers_past_the_end() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    let mut read = |offset: u64| {
        state.handle(req(
            "fs.read",
            json!({
                "project_id": project_id.clone(),
                "path": "a.txt",
                "range": { "offset": offset, "bytes": 4096 },
            }),
        ))["result"]
            .clone()
    };
    let past = read(100);
    assert_eq!(past["content_b64"], "");
    assert_eq!(past["range"]["offset"], 8);
    assert_eq!(past["range"]["end"], 8);
    let before = read(0)["range"]["version"].clone();
    std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    assert_ne!(read(0)["range"]["version"], before);

    let refused = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "a.txt", "range": { "offset": 0 } }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
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
    let project_id = state.project_at(0).id.clone();

    let fifo_path = repo.join("pipe");
    let fifo = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
    // SAFETY: `fifo` is a valid NUL-terminated path in the temporary repo.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let fifo_read = state.handle(req(
        "fs.read",
        json!({ "project_id": project_id, "path": "pipe" }),
    ));
    assert_eq!(fifo_read["ok"], false, "{fifo_read:?}");
    assert_eq!(fifo_read["error"], "not a file");

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

/// Distinct configured sources, including a plain folder, with the same leaf
/// name so resolving the wrong root cannot silently satisfy a file request.
fn project_files_fixture() -> (tempfile::TempDir, AppState, String, PathBuf, PathBuf) {
    let (directory, repo) = init_repo();
    let plain = directory.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(repo.join("notes.md"), "git source\n").unwrap();
    std::fs::write(plain.join("notes.md"), "plain source\n").unwrap();
    let mut state = AppState::new(
        repo.clone(),
        directory.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.project_at(0).id.clone();
    let mut sources = state.project_at(0).sources.clone();
    sources.push(crate::app::projects::ProjectSource::added(
        "source-2".into(),
        "Plain folder".into(),
        "plain".into(),
        plain.clone(),
        false,
        "main".into(),
    ));
    assert!(state.projects.set_sources(&project_id, sources));
    (directory, state, project_id, repo, plain)
}

fn project_file_params(project_id: &str, source_id: &str, path: &str) -> Value {
    json!({
        "project_id": project_id, "source_id": source_id, "path": path,
        "content_b64": b64encode(b"after\n"),
        "expected_revision": sha256_hex(b"plain source\n"),
    })
}

#[test]
fn project_source_files_tree_read_and_write_use_the_selected_configured_root() {
    let (_directory, mut state, project_id, repo, plain) = project_files_fixture();
    for (source_id, contents) in [("source-1", "git source\n"), ("source-2", "plain source\n")] {
        let tree = state.handle(req(
            "fs.tree",
            json!({
                "project_id": project_id, "source_id": source_id,
            }),
        ));
        assert_eq!(tree["ok"], true, "{tree}");
        assert!(tree["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "notes.md"));
        let read = state.handle(req(
            "fs.read",
            json!({
                "project_id": project_id, "source_id": source_id, "path": "notes.md",
            }),
        ));
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(
            read["result"]["content_b64"],
            b64encode(contents.as_bytes())
        );
        assert_eq!(read["result"]["editable"], true);
    }
    let params = project_file_params(&project_id, "source-2", "notes.md");
    let write = state.handle(req("fs.write", params.clone()));
    assert_eq!(write["ok"], true, "{write}");
    assert_eq!(write["result"]["revision"], sha256_hex(b"after\n"));
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.md")).unwrap(),
        "after\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "git source\n"
    );
    let stale = state.handle(req("fs.write", params));
    assert_eq!(stale["ok"], false, "{stale}");
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .contains("revision conflict"),
        "{stale}"
    );
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.md")).unwrap(),
        "after\n"
    );
}

#[test]
fn project_source_files_refuse_unknown_cross_project_and_mixed_scope_ids() {
    let (directory, mut state, project_id, _repo, _plain) = project_files_fixture();
    let other = directory.path().join("other-project");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("notes.md"), "other project's source\n").unwrap();
    let other_id = state.add_project(other, "main".into());
    // Source ids are project-local: both source-1 rows must select their own root.
    let other_read = state.handle(req(
        "fs.read",
        json!({
            "project_id": other_id, "source_id": "source-1", "path": "notes.md",
        }),
    ));
    assert_eq!(other_read["ok"], true, "{other_read}");
    assert_eq!(
        other_read["result"]["content_b64"],
        b64encode(b"other project's source\n")
    );
    // source-2 exists in the first project but never belonged to this one.
    for params in [
        json!({ "project_id": other_id, "source_id": "source-2" }),
        json!({ "project_id": project_id, "source_id": "missing" }),
        json!({ "project_id": "missing", "source_id": "source-2" }),
    ] {
        for method in ["fs.tree", "fs.read", "fs.write"] {
            let mut params = params.clone();
            params["path"] = json!("notes.md");
            params["expected_revision"] = json!("stale");
            params["content_b64"] = json!("YQ==");
            let error = match method {
                "fs.tree" => state.fs_tree(&params),
                "fs.read" => state.fs_read(&params),
                _ => state.fs_write(&params),
            }
            .unwrap_err();
            assert!(error.starts_with("unknown "), "{method}: {error}");
        }
    }
    for field in ["workspace_id", "run_id", "worktree_id"] {
        let mut params = project_file_params(&project_id, "source-2", "notes.md");
        params[field] = json!("other-scope");
        for call in [AppState::fs_tree, AppState::fs_read, AppState::fs_write] {
            let error = call(&mut state, &params).unwrap_err();
            assert!(error.contains("scope"), "{error}");
        }
    }
}

#[test]
fn project_source_files_refuse_host_paths_and_traversal_without_writing() {
    let (directory, mut state, project_id, _repo, plain) = project_files_fixture();
    let outside = directory.path().join("outside.md");
    std::fs::write(&outside, "outside\n").unwrap();
    for path in [
        outside.to_str().unwrap(),
        "../outside.md",
        "plain/../../outside.md",
    ] {
        let params = project_file_params(&project_id, "source-2", path);
        for call in [AppState::fs_tree, AppState::fs_read, AppState::fs_write] {
            assert_eq!(
                call(&mut state, &params).unwrap_err(),
                "path escapes the worktree"
            );
        }
    }
    let forged = state.handle(req(
        "fs.tree",
        json!({
            "project_id": project_id, "source_id": "source-2", "root": directory.path(),
        }),
    ));
    assert_eq!(forged["error_code"], "invalid_params", "{forged}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside\n");
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.md")).unwrap(),
        "plain source\n"
    );
}

#[cfg(unix)]
#[test]
fn project_source_files_refuse_symlink_escape_without_writing() {
    use std::os::unix::fs::symlink;
    let (_directory, mut state, project_id, repo, plain) = project_files_fixture();
    symlink(&repo, plain.join("escape")).unwrap();
    symlink(repo.join("notes.md"), plain.join("leaf.md")).unwrap();
    for path in ["escape/notes.md", "leaf.md"] {
        let params = project_file_params(&project_id, "source-2", path);
        for call in [AppState::fs_tree, AppState::fs_read, AppState::fs_write] {
            assert_eq!(
                call(&mut state, &params).unwrap_err(),
                "path escapes the worktree"
            );
        }
    }
    let tree = state.fs_tree(&json!({
        "project_id": project_id, "source_id": "source-2", "path": "escape",
    }));
    assert_eq!(tree.unwrap_err(), "path escapes the worktree");
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "git source\n"
    );
}
