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
        |checkout, arguments| {
            external = Some(advance_without_files(checkout, &before));
            super::super::merge_git_action(checkout, arguments)
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
        |_, _| Err(GitActionError::OutcomeUnknown("uncertain child".into())),
    );
    assert_eq!(
        result,
        Err(GitActionError::OutcomeUnknown("uncertain child".into()))
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
