use super::*;
use crate::git_fixture::{git_in, init_repo};
use crate::isolation::{record_branch_teardown, BranchTeardown};
use crate::reviews::opening::{open, OpenReviewRequest, OpenedReview, OpeningHooks};
use crate::tracker::{Actor, Task};
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory, WorkspaceStatus};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) struct Hooks;
impl OpeningHooks for Hooks {
    fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }
    fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }
    fn dispatch_reviewer(&self, _: &Task, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) fn request(home: &Path, source: &Path) -> OpenReviewRequest {
    let root = home.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let checkout = root.join("repo");
    git_in(
        source,
        &[
            "worktree",
            "add",
            "-b",
            "build/work",
            checkout.to_str().unwrap(),
        ],
    );
    record_branch_teardown(&checkout, BranchTeardown::DeletesBranch).unwrap();
    let workspace = Workspace {
        id: "workspace-1".into(),
        project_id: "project-1".into(),
        name: "Changes".into(),
        root,
        status: WorkspaceStatus::Ready,
        archived_at: None,
        isolation: Default::default(),
        managed: true,
        created_by_agent: false,
        locked: false,
        directories: vec![WorkspaceDirectory {
            id: "directory-1".into(),
            source_id: "source-1".into(),
            name: "Source".into(),
            path: checkout,
            source_path: source.into(),
            is_git: true,
            branch: Some("build/work".into()),
            base_branch: "main".into(),
            effective_isolation: Some(Default::default()),
            finished_head: None,
            status: DirectoryStatus::Ready,
            error: None,
        }],
    };
    crate::workspace::persist_review_workspace(&workspace).unwrap();
    OpenReviewRequest {
        project_path: source.to_str().unwrap().into(),
        request_id: "open-1".into(),
        receiver_root: home.join("receivers"),
        request: ReviewOpeningRequest {
            workspace_id: workspace.id.clone(),
            title: "Improve review flow".into(),
            description: "Review committed changes".into(),
            creator: Actor::User,
            reviewer: None,
            directories: vec![ReviewMembership {
                directory_id: "directory-1".into(),
                source_id: "source-1".into(),
                kind: ReviewMembershipKind::Git,
                reason: None,
            }],
            base_branches: BTreeMap::from([("directory-1".into(), "refs/heads/main".into())]),
        },
        workspace,
    }
}

pub(crate) struct Fixture {
    pub(crate) _home: tempfile::TempDir,
    pub(crate) source: PathBuf,
    pub(crate) store: Store,
    pub(crate) request: OpenReviewRequest,
    pub(crate) opened: OpenedReview,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        let (home, source) = init_repo();
        let store = Store::new(home.path().join("db")).unwrap();
        let request = request(home.path(), &source);
        let opened = open(&store, &request, &Hooks).unwrap();
        Self {
            _home: home,
            source,
            store,
            request,
            opened,
        }
    }
    pub(crate) fn checkout(&self) -> &Path {
        &self.request.workspace.directories[0].path
    }
    pub(crate) fn task_id(&self) -> &str {
        &self.opened.task.id
    }
    pub(crate) fn review(&self) -> crate::reviews::records::Review {
        self.store.load_review(self.task_id()).unwrap().unwrap()
    }
    pub(crate) fn sync(&self) -> SyncResult {
        reconcile(&self.store, self.task_id()).unwrap()
    }
    pub(crate) fn commit(&self, name: &str) -> String {
        std::fs::write(self.checkout().join(name), format!("{name}\n")).unwrap();
        git_in(self.checkout(), &["add", name]);
        git_in(self.checkout(), &["commit", "-m", name]);
        oid(self.checkout(), "HEAD")
    }
    pub(crate) fn push(&self) {
        git_in(self.checkout(), &["push"]);
    }
    pub(crate) fn observation(&self) -> ReviewSyncObservation {
        self.store
            .load_review_sync_observations(self.task_id())
            .unwrap()
            .pop()
            .unwrap()
    }
}
fn oid(path: &Path, name: &str) -> String {
    git2::Repository::open(path)
        .unwrap()
        .revparse_single(name)
        .unwrap()
        .id()
        .to_string()
}

#[test]
fn received_push_publishes_explicit_oid_without_a_client_and_noops_dedupe() {
    let f = Fixture::new();
    assert!(f.sync().persisted, "startup persists first observation");
    assert!(
        !f.sync().persisted,
        "identical observations are not new writes"
    );
    let published = f.commit("published.txt");
    assert!(f.sync().persisted);
    assert_eq!(
        f.review().snapshots.len(),
        1,
        "commit without push stays unpublished"
    );
    f.push();
    f.commit("unpublished.txt");
    assert!(f.sync().persisted);
    let review = f.review();
    assert_eq!(review.snapshots.len(), 2);
    assert_eq!(
        review.snapshots[1].directories[0].head.as_deref(),
        Some(published.as_str())
    );
    assert_eq!(review.snapshots[1].author, Actor::Build);
    assert_eq!(f.observation().pending_commits, Some(1));
    assert!(!f.sync().persisted);
    git_in(
        f.checkout(),
        &[
            "push",
            "build-review",
            &format!("{published}:{}", review.bindings[0].receiving_ref),
        ],
    );
    assert!(!f.sync().persisted);
}

#[test]
fn restart_and_packed_refs_recover_latest_push_without_an_event() {
    let f = Fixture::new();
    f.commit("first.txt");
    f.push();
    let latest = f.commit("latest.txt");
    f.push();
    let binding = &f.opened.review.bindings[0];
    git_in(
        &binding.receiving_repository,
        &["pack-refs", "--all", "--prune"],
    );
    let store = Store::new(f._home.path().join("db")).unwrap();
    assert!(reconcile(&store, f.task_id()).unwrap().persisted);
    let review = f.review();
    assert_eq!(review.snapshots.len(), 2);
    assert_eq!(
        review.snapshots[1].directories[0].head.as_deref(),
        Some(latest.as_str())
    );
}

#[test]
fn target_only_movement_updates_observation_without_snapshot_then_merge_base_changes() {
    let f = Fixture::new();
    let head = f.commit("review.txt");
    f.push();
    f.sync();
    std::fs::write(f.source.join("target.txt"), "target only\n").unwrap();
    git_in(&f.source, &["add", "target.txt"]);
    git_in(&f.source, &["commit", "-m", "target"]);
    assert!(f.sync().persisted);
    assert_eq!(f.review().snapshots.len(), 2);
    assert_eq!(
        f.observation().target_head.as_deref(),
        Some(oid(&f.source, "HEAD").as_str())
    );
    git_in(&f.source, &["reset", "--hard", &head]);
    f.sync();
    let review = f.review();
    assert_eq!(review.snapshots.len(), 3);
    assert_eq!(
        review.snapshots[2].publication.as_ref().unwrap().reason,
        ReviewPublicationReason::BaseChanged
    );
    assert_eq!(
        review.snapshots[2].directories[0]
            .base
            .as_ref()
            .unwrap()
            .oid,
        head
    );
}

#[test]
fn force_with_lease_records_rewrite_and_keeps_old_pins_and_opinions_stale() {
    let f = Fixture::new();
    let old = f.commit("old.txt");
    f.push();
    f.sync();
    let previous = f.review().snapshots[1].clone();
    git_in(f.checkout(), &["commit", "--amend", "-m", "rewritten"]);
    assert_eq!(f.review().snapshots.len(), 2);
    assert!(f.sync().persisted);
    assert_eq!(
        f.review().snapshots.len(),
        2,
        "rebase without push stays unpublished"
    );
    let binding = &f.opened.review.bindings[0];
    git_in(
        f.checkout(),
        &[
            "push",
            &format!("--force-with-lease={}:{}", binding.receiving_ref, old),
        ],
    );
    f.sync();
    let review = f.review();
    assert_eq!(review.snapshots.len(), 3);
    assert!(
        review.snapshots[2]
            .publication
            .as_ref()
            .unwrap()
            .directories[0]
            .rewritten
    );
    assert_eq!(review.snapshots[1], previous);
    let pin = crate::reviews::capture::pin_prefix(f.task_id(), &previous.id, &binding.directory_id)
        .unwrap();
    assert_eq!(
        oid(&binding.receiving_repository, &format!("{pin}/head")),
        old
    );
}

#[test]
fn deleted_ref_marks_unavailable_and_restore_recovers_without_new_snapshot() {
    let f = Fixture::new();
    f.sync();
    let binding = &f.opened.review.bindings[0];
    git_in(
        f.checkout(),
        &[
            "push",
            "build-review",
            &format!(":{}", binding.receiving_ref),
        ],
    );
    assert!(f.sync().retry);
    assert_eq!(f.observation().health, ReviewSyncHealth::Unavailable);
    assert_eq!(f.review().snapshots.len(), 1);
    assert!(!f.sync().persisted);
    f.push();
    f.sync();
    assert_eq!(f.observation().health, ReviewSyncHealth::Current);
    assert_eq!(f.review().snapshots.len(), 1);
}

#[test]
fn workspace_and_source_removal_leave_snapshot_pins_readable() {
    let f = Fixture::new();
    f.commit("saved.txt");
    f.push();
    f.sync();
    let saved = f.review().snapshots[1].directories[0].clone();
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    std::fs::remove_dir_all(&f.source).unwrap();
    assert!(f.sync().retry);
    assert_eq!(f.observation().health, ReviewSyncHealth::Unavailable);
    assert_eq!(f.review().snapshots.len(), 2);
    git_in(
        &f.opened.review.bindings[0].receiving_repository,
        &["gc", "--prune=now"],
    );
    let result = crate::reviews::read::read(
        &saved,
        &crate::reviews::read::ReviewReadRequest {
            mode: crate::reviews::read::ReviewReadMode::Blob,
            path: Some("saved.txt".into()),
            paths: Vec::new(),
            range: None,
            patch: false,
        },
    )
    .unwrap();
    let crate::reviews::read::ReviewReadResult::Blob(blob) = result else {
        panic!("blob")
    };
    assert_eq!(blob.content_b64, crate::encoding::b64encode(b"saved.txt\n"));
}

#[test]
fn received_push_after_workspace_removal_still_captures_from_registered_receiver() {
    let f = Fixture::new();
    let clone = f._home.path().join("publisher");
    git_in(
        f._home.path(),
        &["clone", f.source.to_str().unwrap(), clone.to_str().unwrap()],
    );
    crate::git_fixture::configure_repo(&clone);
    std::fs::write(clone.join("after-removal.txt"), "received from clone\n").unwrap();
    git_in(&clone, &["add", "after-removal.txt"]);
    git_in(&clone, &["commit", "-m", "from clone"]);
    let binding = &f.opened.review.bindings[0];
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    git_in(
        &clone,
        &[
            "push",
            binding.receiving_repository.to_str().unwrap(),
            &format!("HEAD:{}", binding.receiving_ref),
        ],
    );
    let result = f.sync();
    assert!(result.persisted);
    assert!(
        result.retry,
        "missing working checkout remains visibly unavailable"
    );
    let saved = f.review();
    assert_eq!(saved.snapshots.len(), 2);
    assert_eq!(
        saved.snapshots[1].directories[0].head.as_deref(),
        Some(oid(&clone, "HEAD").as_str())
    );
    assert_eq!(f.observation().health, ReviewSyncHealth::Unavailable);
}

#[test]
fn unknown_receiver_lock_retains_snapshot_and_releases_only_failed_candidates_pins() {
    let f = Fixture::new();
    f.sync();
    f.commit("blocked.txt");
    f.push();
    let binding = &f.opened.review.bindings[0];
    let before = private_refs(&binding.receiving_repository);
    let lock = binding
        .receiving_repository
        .join(format!("{}.lock", binding.receiving_ref));
    std::fs::write(&lock, b"unrelated writer").unwrap();
    assert!(f.sync().retry);
    assert_eq!(f.observation().health, ReviewSyncHealth::Interrupted);
    assert_eq!(f.review().snapshots.len(), 1);
    assert_eq!(private_refs(&binding.receiving_repository), before);
    assert_eq!(std::fs::read(&lock).unwrap(), b"unrelated writer");
    std::fs::remove_file(lock).unwrap();
    f.sync();
    assert_eq!(f.review().snapshots.len(), 2);
}

#[test]
fn stale_publication_writer_releases_its_pins_and_preserves_winning_snapshot() {
    let f = Fixture::new();
    let stale = f.review();
    f.commit("winning.txt");
    f.push();
    f.sync();
    let winning = f.review();
    let binding = &winning.bindings[0];
    let before = private_refs(&binding.receiving_repository);
    let received = publication::observe_received(binding).unwrap();
    assert!(publish(&f.store, &stale, &[received]).is_err());
    assert_eq!(f.review(), winning);
    assert_eq!(private_refs(&binding.receiving_repository), before);
}

fn private_refs(path: &Path) -> BTreeMap<String, String> {
    let repo = git2::Repository::open_bare(path).unwrap();
    repo.references_glob("refs/build/reviews/*")
        .unwrap()
        .map(|reference| {
            let reference = reference.unwrap();
            (
                reference.name().unwrap().into(),
                reference.target().unwrap().to_string(),
            )
        })
        .collect()
}
