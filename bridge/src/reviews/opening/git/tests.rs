use super::ownership::OWNERSHIP_DIRECTORY;
use super::*;
use crate::git_fixture::{git_in, init_repo};
use crate::reviews::model::{ReviewMembership, ReviewMembershipKind};
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory, WorkspaceStatus};
use std::collections::BTreeMap;
use std::path::Path;

fn workspace(repo: &Path) -> Workspace {
    Workspace {
        id: "workspace-1".into(),
        project_id: "project-1".into(),
        name: "review fixture".into(),
        root: repo.parent().unwrap().to_path_buf(),
        status: WorkspaceStatus::Ready,
        archived_at: None,
        directories: vec![WorkspaceDirectory {
            id: "directory-1".into(),
            source_id: "source-1".into(),
            name: "repo".into(),
            path: repo.to_path_buf(),
            is_git: true,
            branch: Some("main".into()),
            effective_isolation: None,
            finished_head: None,
            status: DirectoryStatus::Ready,
            source_path: repo.to_path_buf(),
            base_branch: "main".into(),
            error: None,
        }],
        isolation: Default::default(),
        managed: true,
        created_by_agent: false,
        locked: false,
    }
}

fn memberships() -> Vec<ReviewMembership> {
    vec![ReviewMembership {
        directory_id: "directory-1".into(),
        source_id: "source-1".into(),
        kind: ReviewMembershipKind::Git,
        reason: None,
    }]
}

fn plan(workspace: &Workspace) -> Vec<ReviewBranchBinding> {
    plan_bindings(
        workspace,
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Add CSV export!",
    )
    .unwrap()
}

#[test]
fn preview_is_stable_and_does_not_write_refs_or_change_head() {
    let (_temp, repo) = init_repo();
    let before = std::fs::read(repo.join(".git/HEAD")).unwrap();
    let first = plan(&workspace(&repo));
    assert_eq!(first, plan(&workspace(&repo)));
    assert_eq!(
        first[0].dedicated_branch_ref,
        "refs/heads/review/402-add-csv-export"
    );
    assert_eq!(first[0].base_branch_ref, "refs/heads/main");
    assert_eq!(std::fs::read(repo.join(".git/HEAD")).unwrap(), before);
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_reference(&first[0].dedicated_branch_ref)
        .is_err());
}

#[test]
fn prepare_and_cancel_preserve_staged_unstaged_untracked_and_original_branch() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    std::fs::write(repo.join("README.md"), "staged\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    std::fs::write(repo.join("README.md"), "unstaged\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
    let index_before = std::fs::read(repo.join(".git/index")).unwrap();
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    let opened = git2::Repository::open(&repo).unwrap();
    assert_eq!(
        opened.head().unwrap().name(),
        Some(binding.dedicated_branch_ref.as_str())
    );
    assert_eq!(
        opened
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap()
            .to_string(),
        binding.initial_head
    );
    assert_eq!(
        std::fs::read(repo.join(".git/index")).unwrap(),
        index_before
    );
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(opened.head().unwrap().name(), Some("refs/heads/main"));
    assert!(opened
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
    assert_eq!(
        std::fs::read(repo.join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "unstaged\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("untracked.txt")).unwrap(),
        "untracked\n"
    );
}

#[test]
fn collisions_choose_one_stable_task_suffix_and_never_overwrite() {
    let (_temp, repo) = init_repo();
    git_in(&repo, &["branch", "review/402-add-csv-export"]);
    let binding = plan(&workspace(&repo)).remove(0);
    assert!(binding
        .dedicated_branch_ref
        .starts_with("refs/heads/review/402-add-csv-export-"));
    assert_eq!(
        binding.dedicated_branch_ref,
        plan(&workspace(&repo))[0].dedicated_branch_ref
    );
    git_in(
        &repo,
        &[
            "branch",
            binding
                .dedicated_branch_ref
                .strip_prefix("refs/heads/")
                .unwrap(),
        ],
    );
    assert!(plan_bindings(
        &workspace(&repo),
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Add CSV export!"
    )
    .is_err());
}

#[test]
fn resumed_preparation_requires_the_ownership_stamp() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    binding.preparation = ReviewPreparationState::Planned;
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    assert!(prepare_branch("task-stable-identity", "different-request", &mut binding).is_err());
    assert!(cleanup_branch("task-stable-identity", "different-request", &binding).is_err());
}

#[test]
fn external_commit_refuses_resume_and_cleanup() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    git_in(
        &repo,
        &["commit", "--allow-empty", "-m", "external advance"],
    );
    let advanced = git2::Repository::open(&repo)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert!(cleanup_branch("task-stable-identity", "request-1", &binding).is_err());
    assert_eq!(
        git2::Repository::open(&repo)
            .unwrap()
            .head()
            .unwrap()
            .target(),
        Some(advanced)
    );
}

#[test]
fn changed_head_and_in_progress_operation_are_refused_before_mutation() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    git_in(&repo, &["checkout", "-b", "other"]);
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    git_in(&repo, &["checkout", "main"]);
    std::fs::write(repo.join(".git/MERGE_HEAD"), &binding.initial_head).unwrap();
    assert!(plan_bindings(
        &workspace(&repo),
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Add CSV export!"
    )
    .is_err());
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
}

#[test]
fn detached_head_is_restored_without_checkout() {
    let (_temp, repo) = init_repo();
    git_in(&repo, &["checkout", "--detach"]);
    let mut binding = plan(&workspace(&repo)).remove(0);
    assert_eq!(binding.original_branch_ref, None);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .head_detached()
        .unwrap());
}

#[test]
fn teardown_stamp_is_preserved_and_restored_on_cancel() {
    let (_temp, repo) = init_repo();
    let teardown = repo.join(".git/build-branch-teardown");
    std::fs::write(&teardown, "deletes-branch").unwrap();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    assert_eq!(std::fs::read_to_string(&teardown).unwrap(), "keeps-branch");
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(
        std::fs::read_to_string(&teardown).unwrap(),
        "deletes-branch"
    );
}

#[test]
fn shared_source_repositories_receive_distinct_stable_directory_suffixes() {
    let (temp, repo) = init_repo();
    let checkout = temp.path().join("second");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "build/second",
            checkout.to_str().unwrap(),
        ],
    );
    let mut workspace = workspace(&repo);
    let mut second = workspace.directories[0].clone();
    second.id = "directory-2".into();
    second.path = checkout;
    second.branch = Some("build/second".into());
    workspace.directories.push(second);
    let mut memberships = memberships();
    memberships.push(ReviewMembership {
        directory_id: "directory-2".into(),
        source_id: "source-1".into(),
        kind: ReviewMembershipKind::Git,
        reason: None,
    });
    let planned = plan_bindings(
        &workspace,
        &memberships,
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Add CSV export!",
    )
    .unwrap();
    assert_ne!(
        planned[0].dedicated_branch_ref,
        planned[1].dedicated_branch_ref
    );
    assert!(planned.iter().all(|binding| binding
        .dedicated_branch_ref
        .starts_with("refs/heads/review/402-add-csv-export-")));
    assert_eq!(
        planned,
        plan_bindings(
            &workspace,
            &memberships,
            &BTreeMap::new(),
            402,
            "task-stable-identity",
            "Add CSV export!"
        )
        .unwrap()
    );
}

#[test]
fn branch_taken_after_preview_is_refused_without_switching_head() {
    let (temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            binding
                .dedicated_branch_ref
                .strip_prefix("refs/heads/")
                .unwrap(),
            temp.path().join("other").to_str().unwrap(),
        ],
    );
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert_eq!(
        git2::Repository::open(&repo)
            .unwrap()
            .head()
            .unwrap()
            .name(),
        Some("refs/heads/main")
    );
}

#[test]
fn unmanaged_nonroot_and_unrelated_base_are_refused() {
    let (_temp, repo) = init_repo();
    let mut workspace = workspace(&repo);
    workspace.managed = false;
    assert!(plan_bindings(
        &workspace,
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Review"
    )
    .is_err());
    workspace.managed = true;
    std::fs::create_dir(repo.join("subdirectory")).unwrap();
    workspace.directories[0].path = repo.join("subdirectory");
    assert!(plan_bindings(
        &workspace,
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Review"
    )
    .is_err());
    workspace.directories[0].path = repo.clone();
    git_in(&repo, &["checkout", "--orphan", "unrelated"]);
    git_in(&repo, &["commit", "--allow-empty", "-m", "unrelated root"]);
    assert!(plan_bindings(
        &workspace,
        &memberships(),
        &BTreeMap::new(),
        402,
        "task-stable-identity",
        "Review"
    )
    .is_err());
}

#[test]
fn independent_clone_history_is_compared_without_importing_objects() {
    let (temp, source) = init_repo();
    let checkout = temp.path().join("clone");
    git_in(
        &source,
        &[
            "clone",
            source.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "source advanced"],
    );
    git_in(&checkout, &["checkout", "-b", "build/work"]);
    git_in(
        &checkout,
        &["commit", "--allow-empty", "-m", "work advanced"],
    );
    let mut workspace = workspace(&checkout);
    workspace.directories[0].source_path = source.clone();
    let binding = plan(&workspace).remove(0);
    let source_head = git2::Repository::open(&source)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    assert!(git2::Repository::open(&checkout)
        .unwrap()
        .find_commit(source_head)
        .is_err());
    assert_eq!(binding.source_repository, source.canonicalize().unwrap());
}

#[test]
fn receiver_collision_uses_the_same_stable_task_suffix_without_writing() {
    let (temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    let receiver =
        crate::reviews::receivers::plan_receiver(&repo, &temp.path().join("receivers")).unwrap();
    crate::reviews::receivers::ensure_receiver(&receiver).unwrap();
    binding.receiving_repository = receiver.path.clone();
    let receiver_repo = git2::Repository::open_bare(&receiver.path).unwrap();
    receiver_repo
        .odb()
        .unwrap()
        .write(git2::ObjectType::Blob, b"occupied ref")
        .map(|oid| {
            receiver_repo
                .reference(&binding.receiving_ref, oid, false, "existing review")
                .unwrap();
        })
        .unwrap();
    let original = binding.dedicated_branch_ref.clone();
    resolve_receiver_collision(&mut binding, "task-stable-identity").unwrap();
    assert_eq!(
        binding.dedicated_branch_ref,
        format!("{original}-{}", short_identity("task-stable-identity"))
    );
    assert_eq!(binding.receiving_ref, binding.dedicated_branch_ref);
    assert!(receiver_repo
        .find_reference(&binding.receiving_ref)
        .is_err());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
}

#[test]
fn cancellation_does_not_depend_on_the_live_review_base() {
    let (_temp, repo) = init_repo();
    git_in(&repo, &["checkout", "-b", "build/work"]);
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    git_in(&repo, &["branch", "user/retained"]);
    git_in(&repo, &["branch", "-D", "main"]);
    validate_cleanup("task-stable-identity", "request-1", &binding).unwrap();
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    let opened = git2::Repository::open(&repo).unwrap();
    assert_eq!(opened.head().unwrap().name(), Some("refs/heads/build/work"));
    assert!(opened.find_reference("refs/heads/user/retained").is_ok());
}

#[test]
fn created_branch_before_journal_save_is_recognized_on_resume() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    let (working, source) = validate_repositories(&binding).unwrap();
    let ownership = BranchOwnership::new(
        "task-stable-identity",
        "request-1",
        &binding,
        &working,
        &source,
    )
    .unwrap();
    persist_ownership(&ownership_path(&working, &binding), &ownership).unwrap();
    ensure_branch(&working, &binding, &ownership).unwrap();
    assert_eq!(working.head().unwrap().name(), Some("refs/heads/main"));
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    assert_eq!(
        working.head().unwrap().name(),
        Some(binding.dedicated_branch_ref.as_str())
    );
}

#[test]
fn stamped_intent_never_claims_an_external_same_oid_branch() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    let (working, source) = validate_repositories(&binding).unwrap();
    let ownership = BranchOwnership::new(
        "task-stable-identity",
        "request-1",
        &binding,
        &working,
        &source,
    )
    .unwrap();
    persist_ownership(&ownership_path(&working, &binding), &ownership).unwrap();
    git_in(
        &repo,
        &[
            "branch",
            binding
                .dedicated_branch_ref
                .strip_prefix("refs/heads/")
                .unwrap(),
        ],
    );
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert!(validate_cleanup("task-stable-identity", "request-1", &binding).is_err());
    assert_eq!(working.head().unwrap().name(), Some("refs/heads/main"));
    assert!(working
        .find_reference(&binding.dedicated_branch_ref)
        .is_ok());
}

#[test]
fn head_lock_refuses_preparation_then_resumes_without_changing_index() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    let working = git2::Repository::open(&repo).unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let mut external = working.transaction().unwrap();
    external.lock_ref("HEAD").unwrap();
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert!(working
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
    drop(external);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
}

#[test]
fn durable_reservation_callback_uses_stable_collision_fallback() {
    let (temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    binding.receiving_repository =
        crate::reviews::receivers::plan_receiver(&repo, &temp.path().join("receivers"))
            .unwrap()
            .path;
    let occupied = binding.dedicated_branch_ref.clone();
    resolve_receiver_collision_with(&mut binding, "task-stable-identity", |name| {
        Ok(name == occupied)
    })
    .unwrap();
    assert_eq!(
        binding.dedicated_branch_ref,
        format!("{occupied}-{}", short_identity("task-stable-identity"))
    );
}

#[test]
fn interrupted_temporary_ownership_write_does_not_block_preparation() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    let marker_directory = repo.join(".git").join(OWNERSHIP_DIRECTORY);
    fs::create_dir(&marker_directory).unwrap();
    fs::write(
        marker_directory.join(".preparing-interrupted"),
        b"{truncated",
    )
    .unwrap();
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(
        git2::Repository::open(&repo)
            .unwrap()
            .head()
            .unwrap()
            .name(),
        Some("refs/heads/main")
    );
}

#[test]
fn detached_rebase_elsewhere_still_holds_the_owned_branch() {
    let (temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    let other = temp.path().join("other");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            other.to_str().unwrap(),
            &binding.initial_head,
        ],
    );
    let other_git_dir = crate::isolation::checkout_git_dir(&other).unwrap();
    fs::create_dir(other_git_dir.join("rebase-merge")).unwrap();
    fs::write(
        other_git_dir.join("rebase-merge/head-name"),
        &binding.dedicated_branch_ref,
    )
    .unwrap();
    assert!(validate_current(&binding, ReviewPreparationState::BranchCreated).is_err());
    assert!(validate_cleanup("task-stable-identity", "request-1", &binding).is_err());
    assert!(cleanup_branch("task-stable-identity", "request-1", &binding).is_err());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_reference(&binding.dedicated_branch_ref)
        .is_ok());
}

#[cfg(unix)]
fn interrupt_branch_writer(binding: &ReviewBranchBinding, fixture: &Path, restore_head: bool) {
    let path = fixture.join("branch-crash-binding.json");
    fs::write(&path, serde_json::to_vec(binding).unwrap()).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reviews::opening::git::tests::abrupt_branch_writer_death_recovers_only_registered_locks", "--nocapture"])
        .env("BUILD_REVIEW_BRANCH_CRASH_BINDING", path)
        .env("BUILD_REVIEW_BRANCH_CRASH_RESTORE", if restore_head { "1" } else { "0" })
        .status().unwrap();
    assert_eq!(status.code(), Some(23));
}

#[cfg(unix)]
#[test]
fn abrupt_branch_writer_death_recovers_only_registered_locks() {
    if let Some(path) = std::env::var_os("BUILD_REVIEW_BRANCH_CRASH_BINDING") {
        let binding: ReviewBranchBinding =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let working = git2::Repository::open(&binding.working_repository).unwrap();
        let mut locks = refs::ReferenceLocks::acquire(&working, &binding).unwrap();
        if std::env::var("BUILD_REVIEW_BRANCH_CRASH_RESTORE").as_deref() == Ok("1") {
            locks.set_head(&binding, true).unwrap();
        }
        std::process::exit(23);
    }
    let (temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    interrupt_branch_writer(&binding, temp.path(), false);
    assert!(repo.join(".git/HEAD.lock").exists());
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert!(!repo.join(".git/HEAD.lock").exists());
    interrupt_branch_writer(&binding, temp.path(), true);
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(
        git2::Repository::open(&repo)
            .unwrap()
            .head()
            .unwrap()
            .name(),
        Some("refs/heads/main")
    );
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    interrupt_branch_writer(&binding, temp.path(), false);
    fs::remove_file(repo.join(".git/HEAD.lock")).unwrap();
    fs::write(repo.join(".git/HEAD.lock"), b"user lock\n").unwrap();
    assert!(prepare_branch("task-stable-identity", "request-1", &mut binding).is_err());
    assert_eq!(
        fs::read(repo.join(".git/HEAD.lock")).unwrap(),
        b"user lock\n"
    );
    assert!(cleanup_branch("task-stable-identity", "request-1", &binding).is_err());
    assert_eq!(
        fs::read(repo.join(".git/HEAD.lock")).unwrap(),
        b"user lock\n"
    );
    fs::remove_file(repo.join(".git/HEAD.lock")).unwrap();
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
}

#[test]
fn cancellation_removes_exact_packed_branch_and_preserves_other_packed_refs() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    git_in(&repo, &["branch", "user/keep"]);
    git_in(&repo, &["pack-refs", "--all"]);
    assert!(!repo
        .join(".git")
        .join(&binding.dedicated_branch_ref)
        .exists());
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    let opened = git2::Repository::open(&repo).unwrap();
    assert!(opened
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
    assert_eq!(
        opened
            .find_reference("refs/heads/user/keep")
            .unwrap()
            .target()
            .unwrap()
            .to_string(),
        binding.initial_head
    );
    assert_eq!(opened.head().unwrap().name(), Some("refs/heads/main"));
}

#[test]
fn cancellation_removes_loose_and_packed_ref_without_revealing_shadowed_head() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    git_in(&repo, &["branch", "user/keep"]);
    git_in(&repo, &["pack-refs", "--all", "--no-prune"]);
    let packed = repo.join(".git/packed-refs");
    let before = fs::read_to_string(&packed).unwrap();
    let expected = before.replace(
        &format!(
            "{} {}\n",
            binding.initial_head, binding.dedicated_branch_ref
        ),
        "",
    );
    assert!(repo
        .join(".git")
        .join(&binding.dedicated_branch_ref)
        .exists());
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    assert_eq!(fs::read_to_string(packed).unwrap(), expected);
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_reference(&binding.dedicated_branch_ref)
        .is_err());
}

#[test]
fn head_ref_lock_remains_held_through_branch_deletion() {
    let (_temp, repo) = init_repo();
    let mut binding = plan(&workspace(&repo)).remove(0);
    prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
    let working = git2::Repository::open(&repo).unwrap();
    let mut locks = refs::ReferenceLocks::acquire(&working, &binding).unwrap();
    let packed = locks.prepare_removal(&binding).unwrap();
    locks.set_head(&binding, true).unwrap();
    assert!(repo.join(".git/HEAD.lock").exists());
    let mut competing = working.transaction().unwrap();
    assert!(competing.lock_ref("HEAD").is_err());
    locks.remove_branch(&binding, packed.as_deref()).unwrap();
    assert!(repo.join(".git/HEAD.lock").exists());
    drop(locks);
    assert!(!repo.join(".git/HEAD.lock").exists());
    cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
}

#[test]
fn branch_deletion_refuses_replaced_ref_or_packed_lock() {
    for replaced in ["dedicated", "packed"] {
        let (_temp, repo) = init_repo();
        let mut binding = plan(&workspace(&repo)).remove(0);
        prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
        git_in(&repo, &["pack-refs", "--all", "--no-prune"]);
        let packed_path = repo.join(".git/packed-refs");
        let original_packed = fs::read(&packed_path).unwrap();
        let loose_path = repo.join(".git").join(&binding.dedicated_branch_ref);
        let original_loose = fs::read(&loose_path).unwrap();
        let working = git2::Repository::open(&repo).unwrap();
        let mut locks = refs::ReferenceLocks::acquire(&working, &binding).unwrap();
        let packed = locks.prepare_removal(&binding).unwrap();
        let lock_path = if replaced == "dedicated" {
            repo.join(".git")
                .join(format!("{}.lock", binding.dedicated_branch_ref))
        } else {
            repo.join(".git/packed-refs.lock")
        };
        fs::remove_file(&lock_path).unwrap();
        fs::write(&lock_path, b"user lock\n").unwrap();
        assert!(locks.remove_branch(&binding, packed.as_deref()).is_err());
        assert_eq!(fs::read(&loose_path).unwrap(), original_loose);
        assert_eq!(fs::read(&packed_path).unwrap(), original_packed);
        assert_eq!(
            working.head().unwrap().name(),
            Some(binding.dedicated_branch_ref.as_str())
        );
        drop(locks);
        assert_eq!(fs::read(&lock_path).unwrap(), b"user lock\n");
        fs::remove_file(lock_path).unwrap();
        cleanup_branch("task-stable-identity", "request-1", &binding).unwrap();
    }
}

#[test]
fn cancellation_refuses_original_checkout_rebase_and_bisect_holders() {
    for holder in ["checkout", "rebase", "bisect"] {
        let (temp, repo) = init_repo();
        git_in(&repo, &["switch", "-c", "build/work"]);
        let mut binding = plan(&workspace(&repo)).remove(0);
        prepare_branch("task-stable-identity", "request-1", &mut binding).unwrap();
        validate_cleanup("task-stable-identity", "request-1", &binding).unwrap();
        let other = temp.path().join("original-holder");
        git_in(
            &repo,
            &["worktree", "add", other.to_str().unwrap(), "build/work"],
        );
        let other_git_dir = crate::isolation::checkout_git_dir(&other).unwrap();
        if holder != "checkout" {
            git_in(&other, &["switch", "--detach"]);
            let marker = if holder == "rebase" {
                fs::create_dir(other_git_dir.join("rebase-merge")).unwrap();
                "rebase-merge/head-name"
            } else {
                "BISECT_START"
            };
            fs::write(other_git_dir.join(marker), "refs/heads/build/work\n").unwrap();
        }
        let working = git2::Repository::open(&repo).unwrap();
        let head = fs::read(repo.join(".git/HEAD")).unwrap();
        let index = fs::read(repo.join(".git/index")).unwrap();
        let marker = ownership_path(&working, &binding);
        let ownership = fs::read(&marker).unwrap();
        let error = validate_cleanup("task-stable-identity", "request-1", &binding).unwrap_err();
        assert!(
            error.contains("original") && error.contains("build/work"),
            "{error}"
        );
        assert!(
            error.contains(&other_git_dir.to_string_lossy().to_string()),
            "{error}"
        );
        assert!(cleanup_branch("task-stable-identity", "request-1", &binding).is_err());
        assert_eq!(fs::read(repo.join(".git/HEAD")).unwrap(), head);
        assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(marker).unwrap(), ownership);
        assert!(working
            .find_reference(&binding.dedicated_branch_ref)
            .is_ok());
    }
}
