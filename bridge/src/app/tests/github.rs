//! `github.repos` through the app: the verb holds nothing under the mutex,
//! and the drain answers what `gh` printed — or the sentence saying why not.

use super::*;
use crate::github::GithubCli;
use std::os::unix::fs::PermissionsExt;

/// An app whose `gh` is `script`, and the directory holding both.
fn with_fake_gh(script: &str) -> (tempfile::TempDir, AppState) {
    let (dir, repo) = init_repo();
    let gh = dir.path().join("gh");
    std::fs::write(&gh, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let state = qa_state(&repo, dir.path()).with_github_cli(GithubCli::at(gh, "zech-desktop"));
    (dir, state)
}

#[test]
fn github_repos_lists_off_the_lock_and_answers_its_declared_shape() {
    let (_dir, mut state) = with_fake_gh(
        r#"case "$1" in
  repo) echo '[{"nameWithOwner":"zech/build","description":"","sshUrl":"git@github.com:zech/build.git","url":"https://github.com/zech/build","isPrivate":true,"pushedAt":"2026-09-24T22:00:00Z"}]' ;;
  org) exit 0 ;;
esac"#,
    );

    let (held, deferred) = state.dispatch_deferring("github.repos", &json!({}));
    assert_eq!(
        held.unwrap(),
        Value::Null,
        "nothing is answered under the lock"
    );
    let done = deferred.expect("github.repos defers gh").run();
    let published = state
        .apply_deferred("github.repos", &json!({}), done)
        .expect("the listing passes its declared type");
    assert_eq!(
        published,
        json!({ "repos": [{
            "name_with_owner": "zech/build",
            "ssh_url": "git@github.com:zech/build.git",
            "url": "https://github.com/zech/build",
            "private": true,
            "pushed_at": "2026-09-24T22:00:00Z",
        }] })
    );
}

#[test]
fn github_repos_refuses_with_the_sentence_the_ui_shows() {
    let (_dir, mut state) = with_fake_gh("exit 4");

    let (_, deferred) = state.dispatch_deferring("github.repos", &json!({}));
    let done = deferred.expect("github.repos defers gh").run();
    let refusal = state
        .apply_deferred("github.repos", &json!({}), done)
        .unwrap_err();
    assert_eq!(
        refusal,
        "Build cannot list GitHub repositories on zech-desktop because gh is not signed in. \
         Run `gh auth login` on zech-desktop."
    );
}

#[test]
fn github_repos_is_announced_by_name() {
    assert!(crate::api::capabilities(false).contains(&"github.repos"));
}

#[test]
fn a_github_repos_refusal_is_unavailable_not_internal() {
    let (_dir, mut state) = with_fake_gh("exit 4");
    let refused = state.dispatch_api("github.repos", &json!({})).unwrap_err();
    assert_eq!(refused.code(), "unavailable", "{}", refused.message());
}

#[test]
fn a_unit_test_app_never_runs_this_machines_gh() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let refused = state.dispatch_api("github.repos", &json!({})).unwrap_err();
    assert_eq!(
        refused.message(),
        "Build cannot list GitHub repositories on this machine because gh is not installed."
    );
}
