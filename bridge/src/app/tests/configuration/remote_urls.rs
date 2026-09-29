//! Every verb that hands a client-named remote to git refuses one git would
//! read as an option or a program to run, before any git is spawned.

use super::*;

/// A program git would run if it took the remote as `--upload-pack=<program>`,
/// and the file it leaves behind when it does.
fn hostile_upload_pack(dir: &Path) -> (String, PathBuf) {
    let marker = dir.join("pwned");
    let program = dir.join("upload-pack.sh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (format!("--upload-pack={}", program.display()), marker)
}

fn state_with_project(dir: &Path) -> (AppState, String, PathBuf) {
    let repo = init_repo_named(dir, "code");
    let mut state =
        AppState::new_unrooted(dir.join("worktrees"), "main", true, "/tmp/test-mcp.sock");
    state.set_projects_dir(dir.join("projects"));
    let project_id = state.add_project(repo.clone(), "main".to_string());
    (state, project_id, repo)
}

/// Refused by the remote check itself, not by a git that ran and failed.
fn assert_refused_as_a_remote(answer: &Value) {
    assert_eq!(answer["ok"], false, "{answer:?}");
    let error = answer["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("Git remote") || error.contains("Git transport"),
        "{answer:?}"
    );
}

fn assert_refused(answer: &Value, marker: &Path) {
    assert_refused_as_a_remote(answer);
    assert!(
        !marker.exists(),
        "git ran the remote as a program: {answer:?}"
    );
}

#[test]
fn add_source_refuses_a_remote_git_would_read_as_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, project_id, _) = state_with_project(dir.path());
    let (remote, marker) = hostile_upload_pack(dir.path());

    let answer = state.handle(req(
        "project.add_source",
        json!({"project_id": project_id, "remote": remote, "name": "evil"}),
    ));

    assert_refused(&answer, &marker);
    assert!(!dir
        .path()
        .join("projects")
        .join("code-sources")
        .join("evil")
        .exists());
}

#[test]
fn project_create_refuses_a_hostile_remote_in_its_sources_or_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, _, _) = state_with_project(dir.path());
    let (remote, marker) = hostile_upload_pack(dir.path());

    let from_sources = state.handle(req(
        "project.create",
        json!({"name": "evil", "sources": [{"remote": remote}]}),
    ));
    assert_refused(&from_sources, &marker);

    let with_origin = state.handle(req(
        "project.create",
        json!({"name": "fresh", "remote": "ext::sh"}),
    ));
    assert_refused(&with_origin, &marker);
    assert!(!dir.path().join("projects").join("fresh").exists());
}

#[test]
fn set_remote_refuses_a_remote_git_would_read_as_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, project_id, repo) = state_with_project(dir.path());

    let answer = state.handle(req(
        "project.set_remote",
        json!({"project_id": project_id, "url": "--mirror=fetch"}),
    ));

    assert_refused_as_a_remote(&answer);
    assert_eq!(git_remote_origin(&repo), None);
}

#[test]
fn set_remote_still_clears_with_an_empty_url() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, project_id, repo) = state_with_project(dir.path());
    git_in(
        &repo,
        &["remote", "add", "origin", "https://example.invalid/a.git"],
    );

    let answer = state.handle(req(
        "project.set_remote",
        json!({"project_id": project_id, "url": "  "}),
    ));

    assert_eq!(answer["ok"], true, "{answer:?}");
    assert_eq!(git_remote_origin(&repo), None);
}

#[test]
fn add_directory_refuses_a_remote_git_would_read_as_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, project_id, _) = state_with_project(dir.path());
    let created = state.handle(req(
        "workspace.create",
        json!({"project_id": project_id, "name": "work", "isolation": "worktree"}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let (remote, marker) = hostile_upload_pack(dir.path());

    let answer = state.handle(req(
        "workspace.add_directory",
        json!({"workspace_id": created["result"]["workspace_id"], "remote": remote, "name": "evil"}),
    ));

    assert_refused(&answer, &marker);
}
