use super::*;
use crate::git_fixture::{git_command, git_in, init_repo};
use crate::reviews::model::ReviewDirectoryStatus;

fn oid(repository: &Path, reference: &str) -> String {
    git2::Repository::open(repository)
        .unwrap()
        .revparse_single(reference)
        .unwrap()
        .id()
        .to_string()
}

fn feature(repository: &Path) -> ReviewDirectory {
    git_in(repository, &["checkout", "-b", "feature"]);
    std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
    git_in(repository, &["add", "."]);
    git_in(repository, &["commit", "-m", "feature"]);
    let saved = ReviewDirectory {
        id: "directory".into(),
        source_id: "source".into(),
        name: "repo".into(),
        path: repository.into(),
        source_path: repository.into(),
        is_git: true,
        status: ReviewDirectoryStatus::Git,
        reason: None,
        common_git_dir: Some(
            git2::Repository::open(repository)
                .unwrap()
                .commondir()
                .canonicalize()
                .unwrap(),
        ),
        branch: Some("feature".into()),
        base: None,
        head: Some(oid(repository, "HEAD")),
        uncommitted_files: Some(0),
    };
    git_in(repository, &["checkout", "main"]);
    saved
}

#[cfg(unix)]
fn install_hook(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
#[cfg(unix)]
fn expected_merge_honors_default_merge_and_commit_hooks() {
    for hook in ["pre-merge-commit", "commit-msg"] {
        let (_temporary, repository) = init_repo();
        let saved = feature(&repository);
        let before = oid(&repository, "main");
        install_hook(&repository.join(".git/hooks").join(hook), "exit 37");
        let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
        assert!(
            matches!(result, Err(GitActionError::Failed(_))),
            "{hook}: {result:?}"
        );
        assert_eq!(oid(&repository, "main"), before);
        assert!(!repository.join("feature.txt").exists());
    }
}

#[test]
#[cfg(unix)]
fn expected_merge_honors_a_relative_configured_hooks_path() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let hooks = "hooks with 'quotes'";
    git_in(&repository, &["config", "core.hooksPath", hooks]);
    // A Git hook is ignored unless executable. Its sibling resource remains available.
    let directory = repository.join(hooks);
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("policy"), "reject").unwrap();
    install_hook(
        &directory.join("pre-merge-commit"),
        "test \"$(git config core.hooksPath)\" = \"hooks with 'quotes'\" || exit 99\n\
         test \"$(cat \"$(dirname \"$0\")/policy\")\" = reject || exit 98\n\
         printf honored > \"$(git rev-parse --git-dir)/user-hook-policy\"\nexit 37",
    );
    git_in(&repository, &["add", "."]);
    git_in(&repository, &["commit", "-m", "hook resources"]);
    let before = oid(&repository, "main");
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
    let Err(GitActionError::Failed(error)) = result else {
        panic!("{result:?}");
    };
    assert!(error.contains("37") || error.contains("hook"), "{error}");
    assert_eq!(
        fs::read_to_string(repository.join(".git/user-hook-policy")).unwrap(),
        "honored"
    );
    assert_eq!(oid(&repository, "main"), before);
}

#[test]
#[cfg(unix)]
fn expected_merge_delegates_reference_transaction_input_and_veto() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    install_hook(
        &repository.join(".git/hooks/reference-transaction"),
        "[ \"$1\" = prepared ] || exit 0\n\
         directory=$(git rev-parse --git-dir)\n\
         cat > \"$directory/user-transaction-current\"\n\
         if grep -q ' refs/heads/main$' \"$directory/user-transaction-current\"; then\n\
         cp \"$directory/user-transaction-current\" \"$directory/user-transaction\"\nexit 37\nfi",
    );
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
    assert!(
        matches!(result, Err(GitActionError::Failed(_))),
        "{result:?}"
    );
    assert_eq!(oid(&repository, "main"), before);
    let input = fs::read_to_string(repository.join(".git/user-transaction")).unwrap();
    assert!(input.contains(&format!("{before} ")), "{input}");
    assert!(input.contains(" refs/heads/main\n"), "{input}");
}

#[test]
#[cfg(unix)]
fn expected_merge_does_not_silently_drop_non_utf8_hooks() {
    use std::os::unix::ffi::OsStringExt;
    let (temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let hooks = temporary
        .path()
        .join(std::ffi::OsString::from_vec(b"hooks-\xff".to_vec()));
    install_hook(&hooks.join("pre-merge-commit"), "exit 37");
    let status = git_command(&repository, &["config", "core.hooksPath"])
        .arg(&hooks)
        .status()
        .unwrap();
    assert!(status.success());
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
    assert!(
        matches!(result, Err(GitActionError::Failed(_))),
        "{result:?}"
    );
    assert_eq!(oid(&repository, "main"), before);
}

fn advance_without_files(repository: &Path, before: &str) -> String {
    let tree = oid(repository, &format!("{before}^{{tree}}"));
    let output = git_command(
        repository,
        &["commit-tree", &tree, "-p", before, "-m", "external"],
    )
    .output()
    .unwrap();
    assert!(output.status.success());
    let head = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    git_in(
        repository,
        &["update-ref", "refs/heads/main", &head, before],
    );
    head
}

#[test]
fn expected_merge_refuses_a_target_that_advanced_before_admission() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let external = advance_without_files(&repository, &before);
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before);
    assert!(
        matches!(result, Err(GitActionError::Failed(_))),
        "{result:?}"
    );
    assert_eq!(oid(&repository, "main"), external);
    assert!(!repository.join("feature.txt").exists());
}

#[test]
fn expected_merge_updates_a_clean_checked_out_target_and_records_the_tip() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before).unwrap();
    assert_eq!(result.head, oid(&repository, "main"));
    assert_eq!(oid(&repository, "main^1"), before);
    assert_eq!(oid(&repository, "main^2"), saved.head.unwrap());
    assert!(repository.join("feature.txt").is_file());
}

#[test]
fn expected_merge_uses_an_owned_target_without_switching_the_source_branch() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    git_in(&repository, &["checkout", "feature"]);
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before).unwrap();
    assert_eq!(result.head, oid(&repository, "main"));
    assert_eq!(oid(&repository, "HEAD"), saved.head.unwrap());
    assert!(super::super::checked_out_at(&repository, "refs/heads/main")
        .unwrap()
        .is_none());
}

#[test]
fn expected_merge_records_an_already_contained_head_without_another_commit() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    git_in(&repository, &["merge", "--ff-only", "feature"]);
    let before = oid(&repository, "main");
    let result = merge_expected(&saved, &repository, "refs/heads/main", &before).unwrap();
    assert_eq!(result.head, before);
    assert_eq!(oid(&repository, "main"), before);
}

#[test]
fn prepared_ref_transaction_rejects_external_movement_after_the_last_preflight() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let mut external = None;
    let result = merge_checkout_expected_with_runner(
        &repository,
        "main",
        saved.head.as_ref().unwrap(),
        &before,
        |checkout, arguments, observer| {
            external = Some(advance_without_files(checkout, &before));
            super::super::merge_git_action_observed(checkout, arguments, observer)
        },
    );
    assert!(
        matches!(result, Err(GitActionError::Failed(_))),
        "{result:?}"
    );
    assert_eq!(oid(&repository, "main"), external.unwrap());
    assert_eq!(
        git2::Repository::open(&repository).unwrap().state(),
        git2::RepositoryState::Clean
    );
    assert!(!repository.join("feature.txt").exists());
}

#[test]
fn expected_merge_preserves_an_uncertain_git_outcome() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let result = merge_checkout_expected_with_runner(
        &repository,
        "main",
        saved.head.as_ref().unwrap(),
        &before,
        |_, _, _| Err(GitActionError::OutcomeUnknown("uncertain child".into())),
    );
    assert_eq!(
        result,
        Err(GitActionError::OutcomeUnknown("uncertain child".into()))
    );
}

#[test]
fn child_identity_failure_after_git_updates_the_target_is_uncertain() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let result = merge_checkout_expected_with_runner(
        &repository,
        "main",
        saved.head.as_ref().unwrap(),
        &before,
        |checkout, arguments, observer| {
            super::super::merge_git_action_observed(checkout, arguments, &mut |event| {
                observer(event)?;
                if matches!(event, crate::git_process::GitProcessEvent::Started(_)) {
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while oid(checkout, "main") == before {
                        assert!(
                            std::time::Instant::now() < until,
                            "native Git updates its target"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    return Err("child identity journal failed after Git completed".into());
                }
                Ok(())
            })
        },
    );
    assert_ne!(oid(&repository, "main"), before);
    assert!(
        matches!(result, Err(GitActionError::OutcomeUnknown(_))),
        "{result:?}"
    );
}

#[test]
fn target_ref_shell_characters_remain_literal_in_the_transaction_fence() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let branch = "target'$(false)";
    git_in(&repository, &["branch", branch]);
    let before = oid(&repository, branch);
    let result = merge_expected(
        &saved,
        &repository,
        &format!("refs/heads/{branch}"),
        &before,
    )
    .unwrap();
    assert_eq!(result.head, oid(&repository, branch));
    assert_eq!(oid(&repository, "HEAD"), before);
}

#[test]
fn final_integration_verification_refuses_an_unmerged_received_head() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    assert!(
        verify_integrated(&repository, "refs/heads/main", saved.head.as_ref().unwrap()).is_err()
    );
}

#[test]
fn final_integration_verification_reads_the_target_while_another_branch_is_checked_out() {
    let (_temporary, repository) = init_repo();
    let saved = feature(&repository);
    let before = oid(&repository, "main");
    let integrated = merge_expected(&saved, &repository, "refs/heads/main", &before).unwrap();
    git_in(&repository, &["checkout", "feature"]);
    assert_eq!(
        verify_integrated(&repository, "refs/heads/main", saved.head.as_ref().unwrap()).unwrap(),
        integrated.head
    );
}
