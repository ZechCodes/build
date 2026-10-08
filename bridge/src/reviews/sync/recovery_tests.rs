use super::{reconcile, tests::Fixture};
use crate::reviews::capture::pin_prefix;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const CHILD: &str = "reviews::sync::reconcile::recovery_tests::sync_recovery_process_child";

static CAPTURE_BLOCKED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<String, std::sync::mpsc::Sender<()>>>,
> = std::sync::LazyLock::new(Default::default);

pub(super) fn capture_blocked(task_id: &str) {
    let sender = CAPTURE_BLOCKED.lock().unwrap().remove(task_id);
    if let Some(sender) = sender {
        let _ = sender.send(());
    }
}

struct ContentionSignal {
    task_id: String,
    receiver: std::sync::mpsc::Receiver<()>,
}
impl ContentionSignal {
    fn watch(task_id: &str) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(CAPTURE_BLOCKED
            .lock()
            .unwrap()
            .insert(task_id.into(), sender)
            .is_none());
        Self {
            task_id: task_id.into(),
            receiver,
        }
    }
}
impl Drop for ContentionSignal {
    fn drop(&mut self) {
        CAPTURE_BLOCKED.lock().unwrap().remove(&self.task_id);
    }
}

fn journal(f: &Fixture) -> PathBuf {
    f.opened.review.bindings[0]
        .receiving_repository
        .join("build-review-sync")
        .join(format!("{:x}", Sha256::digest(f.task_id().as_bytes())))
        .join("candidate.json")
}

fn child_command(f: &Fixture, phase: &str) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD, "--nocapture"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("BUILD_REVIEW_SYNC_TEST_DB", f._home.path().join("db"))
        .env("BUILD_REVIEW_SYNC_TEST_TASK", f.task_id())
        .env("BUILD_REVIEW_SYNC_DEATH_PHASE", phase);
    command
}

fn crash(f: &Fixture, phase: &str) {
    let status = child_command(f, phase).status().unwrap();
    assert_eq!(
        status.code(),
        Some(27),
        "child must die at the real sync publication boundary"
    );
}

fn candidate(f: &Fixture) -> String {
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal(f)).unwrap()).unwrap();
    raw["snapshot_id"].as_str().unwrap().into()
}

fn pin_oid(f: &Fixture, snapshot: &str) -> Option<String> {
    let binding = &f.opened.review.bindings[0];
    let repo = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let name = format!(
        "{}/head",
        pin_prefix(f.task_id(), snapshot, &binding.directory_id).unwrap()
    );
    repo.refname_to_id(&name).ok().map(|oid| oid.to_string())
}

#[test]
fn sync_recovery_process_child() {
    let Some(db) = std::env::var_os("BUILD_REVIEW_SYNC_TEST_DB") else {
        return;
    };
    let task = std::env::var("BUILD_REVIEW_SYNC_TEST_TASK").unwrap();
    let store = crate::store::Store::new(db).unwrap();
    let result = reconcile(&store, &task).unwrap();
    if let Some(path) = std::env::var_os("BUILD_REVIEW_SYNC_TEST_RESULT") {
        std::fs::write(
            path,
            serde_json::to_vec(
                &serde_json::json!({"persisted": result.persisted, "retry": result.retry}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn dead_precommit_capture_is_reaped_without_touching_published_or_unknown_refs() {
    let f = Fixture::new();
    let head = f.commit("received.txt");
    f.push();
    crash(&f, "before-db");
    let abandoned = candidate(&f);
    assert_eq!(pin_oid(&f, &abandoned).as_deref(), Some(head.as_str()));
    assert_eq!(f.review().snapshots.len(), 1);
    let binding = &f.opened.review.bindings[0];
    let unknown = binding
        .receiving_repository
        .join("build-review-snapshots/unknown.json");
    std::fs::write(&unknown, "foreign marker").unwrap();
    let repo = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    repo.reference(
        "refs/build/reviews/unknown/head",
        git2::Oid::from_str(&head).unwrap(),
        false,
        "foreign",
    )
    .unwrap();
    assert!(f.sync().persisted);
    assert!(pin_oid(&f, &abandoned).is_none());
    assert!(!journal(&f).exists());
    assert_eq!(f.review().snapshots.len(), 2);
    assert_eq!(
        pin_oid(&f, &f.opened.review.snapshots[0].id),
        Some(binding.initial_head.clone())
    );
    assert_eq!(std::fs::read_to_string(unknown).unwrap(), "foreign marker");
    assert_eq!(
        repo.refname_to_id("refs/build/reviews/unknown/head")
            .unwrap()
            .to_string(),
        head
    );
}

#[test]
fn dead_postcommit_capture_keeps_published_pins_and_only_clears_candidate_journal() {
    let f = Fixture::new();
    let head = f.commit("published.txt");
    f.push();
    crash(&f, "after-db");
    let published = candidate(&f);
    assert_eq!(f.review().snapshots.len(), 2);
    assert_eq!(pin_oid(&f, &published).as_deref(), Some(head.as_str()));
    f.sync();
    assert!(!journal(&f).exists());
    assert_eq!(f.review().snapshots.len(), 2);
    assert_eq!(pin_oid(&f, &published).as_deref(), Some(head.as_str()));
}

fn marker(f: &Fixture, candidate: &str) -> PathBuf {
    let binding = &f.opened.review.bindings[0];
    let identity = format!("{}\0{}\0{}", f.task_id(), candidate, binding.directory_id);
    binding
        .receiving_repository
        .join("build-review-snapshots")
        .join(format!("{:x}.json", Sha256::digest(identity.as_bytes())))
}

#[test]
fn abandoned_candidate_with_replaced_pin_is_preserved_for_safe_retry() {
    let f = Fixture::new();
    f.commit("candidate.txt");
    f.push();
    crash(&f, "before-db");
    let abandoned = candidate(&f);
    let binding = &f.opened.review.bindings[0];
    let repo = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let pin = format!(
        "{}/head",
        pin_prefix(f.task_id(), &abandoned, &binding.directory_id).unwrap()
    );
    repo.reference(
        &pin,
        git2::Oid::from_str(&binding.initial_head).unwrap(),
        true,
        "foreign replacement",
    )
    .unwrap();
    let old_marker = std::fs::read(marker(&f, &abandoned)).unwrap();
    assert!(f.sync().retry);
    assert_eq!(
        f.observation().health,
        crate::reviews::model::ReviewSyncHealth::Interrupted
    );
    assert!(journal(&f).exists());
    assert_eq!(pin_oid(&f, &abandoned), Some(binding.initial_head.clone()));
    assert_eq!(std::fs::read(marker(&f, &abandoned)).unwrap(), old_marker);
    assert_eq!(f.review().snapshots.len(), 1);
}

#[cfg(unix)]
#[test]
fn malformed_or_symbolic_candidate_journal_is_never_followed_or_removed() {
    let f = Fixture::new();
    f.commit("candidate.txt");
    f.push();
    crash(&f, "before-db");
    let path = journal(&f);
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, "foreign journal").unwrap();
    assert!(f.sync().retry);
    assert_eq!(
        f.observation().health,
        crate::reviews::model::ReviewSyncHealth::Interrupted
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "foreign journal");
    std::fs::remove_file(&path).unwrap();
    let foreign = f._home.path().join("foreign-journal.json");
    std::fs::write(&foreign, &original).unwrap();
    std::os::unix::fs::symlink(&foreign, &path).unwrap();
    assert!(f.sync().retry);
    assert_eq!(
        f.observation().health,
        crate::reviews::model::ReviewSyncHealth::Interrupted
    );
    assert!(std::fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(foreign).unwrap(), original);
}

#[test]
fn closed_review_with_dead_candidate_remains_discoverable_until_recovery() {
    let f = Fixture::new();
    f.commit("candidate.txt");
    f.push();
    crash(&f, "before-db");
    let abandoned = candidate(&f);
    f.store
        .complete_review(
            f.task_id(),
            1,
            &crate::tracker::Actor::User,
            "closed during capture",
        )
        .unwrap();
    assert_eq!(
        f.store.list_active_review_sync_tasks(None, 64).unwrap(),
        vec![f.task_id().to_string()]
    );
    f.sync();
    assert!(pin_oid(&f, &abandoned).is_none());
    assert!(!journal(&f).exists());
    assert!(f
        .store
        .list_active_review_sync_tasks(None, 64)
        .unwrap()
        .is_empty());
    assert_eq!(f.review().snapshots.len(), 1);
    assert_eq!(
        f.review().state,
        crate::reviews::records::ReviewState::Completed
    );
}

#[test]
fn live_writer_lock_excludes_recovery_of_its_candidate_without_health_churn() {
    let f = Fixture::new();
    let head = f.commit("live.txt");
    f.push();
    let mut owner =
        super::recovery::CaptureJournal::acquire(f.task_id(), &f.opened.review.bindings).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    owner.begin(&f.store, &id).unwrap();
    let received: Vec<_> = f
        .opened
        .review
        .bindings
        .iter()
        .map(crate::reviews::publication::observe_received)
        .collect::<Result<_, _>>()
        .unwrap();
    super::capture(&f.opened.review, &id, &received).unwrap();
    assert!(super::reconcile_background(&f.store, f.task_id())
        .unwrap_err()
        .starts_with("busy:"));
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
    assert!(journal(&f).exists());
    assert!(f
        .store
        .load_review_sync_observations(f.task_id())
        .unwrap()
        .is_empty());
    drop(owner);
    f.sync();
    assert!(pin_oid(&f, &id).is_none());
    assert!(!journal(&f).exists());
}

#[test]
fn explicit_reconcile_waits_for_a_short_live_capture_and_recovers_its_candidate() {
    let f = Fixture::new();
    f.commit("short-capture.txt");
    f.push();
    let mut owner =
        super::recovery::CaptureJournal::acquire(f.task_id(), &f.opened.review.bindings).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    owner.begin(&f.store, &id).unwrap();
    let received = f
        .opened
        .review
        .bindings
        .iter()
        .map(crate::reviews::publication::observe_received)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    super::capture(&f.opened.review, &id, &received).unwrap();
    let blocked = ContentionSignal::watch(f.task_id());
    let result = std::thread::scope(|scope| {
        let release = scope.spawn(move || {
            blocked
                .receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            drop(owner);
        });
        let result = reconcile(&f.store, f.task_id());
        release.join().unwrap();
        result
    });
    assert!(
        result.is_ok(),
        "explicit reconcile must join a short live capture: {result:?}"
    );
    assert!(result.unwrap().persisted);
    assert!(pin_oid(&f, &id).is_none());
    assert!(!journal(&f).exists());
    assert_eq!(f.review().snapshots.len(), 2);
}

#[test]
fn explicit_capture_wait_times_out_without_candidate_or_observation_churn() {
    let f = Fixture::new();
    let head = f.commit("busy-timeout.txt");
    f.push();
    let mut owner =
        super::recovery::CaptureJournal::acquire(f.task_id(), &f.opened.review.bindings).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    owner.begin(&f.store, &id).unwrap();
    let received = f
        .opened
        .review
        .bindings
        .iter()
        .map(crate::reviews::publication::observe_received)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    super::capture(&f.opened.review, &id, &received).unwrap();
    let wait = std::time::Duration::from_millis(30);
    let started = std::time::Instant::now();
    let result = super::reconcile_with_wait(&f.store, f.task_id(), wait);
    assert!(result.unwrap_err().starts_with("busy:"));
    assert!(
        started.elapsed() >= wait,
        "explicit admission must honor its wait budget"
    );
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
    assert_eq!(
        f.store
            .load_review_sync_candidate(f.task_id())
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
    assert!(journal(&f).exists());
    assert!(f
        .store
        .load_review_sync_observations(f.task_id())
        .unwrap()
        .is_empty());
    assert_eq!(f.review().snapshots.len(), 1);
}

#[test]
#[cfg(target_os = "linux")]
fn ended_capture_releases_its_lease_even_while_a_forked_child_retains_the_directory() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = Fixture::new();
    f.commit("inherited-capture.txt");
    f.push();
    let mut owner =
        super::recovery::CaptureJournal::acquire(f.task_id(), &f.opened.review.bindings).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    owner.begin(&f.store, &id).unwrap();
    let received = f
        .opened
        .review
        .bindings
        .iter()
        .map(crate::reviews::publication::observe_received)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    super::capture(&f.opened.review, &id, &received).unwrap();
    let lock_path = journal(&f).parent().unwrap().to_path_buf();
    let child = InheritedCapture::hold();
    assert!(!directory_descriptors(std::process::id(), &lock_path).is_empty());
    assert!(!directory_descriptors(child.0 as u32, &lock_path).is_empty());
    drop(owner);
    assert!(directory_descriptors(std::process::id(), &lock_path).is_empty());
    let child_descriptors = directory_descriptors(child.0 as u32, &lock_path);
    assert!(
        !child_descriptors.is_empty(),
        "child retains the ended writer's FD"
    );
    eprintln!(
        "ended capture actual holder pid={} descriptors={child_descriptors:?} parent={} path={}",
        child.0,
        std::process::id(),
        lock_path.display()
    );

    let result = reconcile(&f.store, f.task_id());
    drop(child);
    assert!(
        result.is_ok(),
        "ended capture must not block explicit reconcile: {result:?}"
    );
    assert!(result.unwrap().persisted);
    assert!(pin_oid(&f, &id).is_none());
    assert!(!journal(&f).exists());
    assert_eq!(f.review().snapshots.len(), 2);
}

#[cfg(target_os = "linux")]
fn directory_descriptors(pid: u32, path: &std::path::Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .flatten()
        .filter(|entry| std::fs::read_link(entry.path()).is_ok_and(|target| target == path))
        .map(|entry| entry.file_name())
        .collect()
}

#[cfg(target_os = "linux")]
struct InheritedCapture(libc::pid_t);

#[cfg(target_os = "linux")]
impl InheritedCapture {
    fn hold() -> Self {
        use std::os::fd::FromRawFd;
        let mut ready = [0; 2];
        // SAFETY: pipe2 receives two writable integer slots.
        assert_eq!(
            unsafe { libc::pipe2(ready.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: the fork child uses only async-signal-safe libc functions.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::close(ready[0]);
                libc::write(ready[1], b"x".as_ptr().cast(), 1);
                libc::close(ready[1]);
                loop {
                    libc::pause();
                }
            }
        }
        let child = Self(pid);
        // SAFETY: the parent closes its writer and owns its reader exactly once.
        unsafe { libc::close(ready[1]) };
        let _reader = unsafe { std::fs::File::from_raw_fd(ready[0]) };
        let mut poll = libc::pollfd {
            fd: ready[0],
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one valid pollfd with a bounded deadline.
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1);
        assert_ne!(poll.revents & libc::POLLIN, 0);
        child
    }
}

#[cfg(target_os = "linux")]
impl Drop for InheritedCapture {
    fn drop(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        // SAFETY: this unreaped PID is our own forked child.
        unsafe { libc::kill(self.0, libc::SIGKILL) };
        while std::time::Instant::now() < deadline {
            // SAFETY: waitpid operates only on our own child.
            let result = unsafe { libc::waitpid(self.0, std::ptr::null_mut(), libc::WNOHANG) };
            if result == self.0
                || (result < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !std::thread::panicking() {
            panic!("inherited capture child did not reap within its deadline");
        }
    }
}

#[test]
fn registry_written_before_receiver_journal_survives_death_and_closed_task_discovery() {
    let f = Fixture::new();
    f.commit("candidate.txt");
    f.push();
    crash(&f, "before-journal");
    assert!(f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .is_some());
    assert!(!journal(&f).exists());
    f.store
        .complete_review(f.task_id(), 1, &crate::tracker::Actor::User, "closed")
        .unwrap();
    assert_eq!(
        f.store.list_active_review_sync_tasks(None, 64).unwrap(),
        vec![f.task_id().to_string()]
    );
    f.sync();
    assert!(f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .is_none());
    assert_eq!(f.review().snapshots.len(), 1);
}

#[test]
fn death_after_journal_clear_retains_published_pins_and_registry_discovery() {
    let f = Fixture::new();
    let head = f.commit("published.txt");
    f.push();
    crash(&f, "after-journal-clear");
    let id = f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .unwrap();
    assert!(!journal(&f).exists());
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
    assert!(
        f.sync().persisted,
        "recovered prior commit still needs invalidation"
    );
    assert!(f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .is_none());
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
}

#[test]
fn candidate_registry_and_journal_uuid_mismatch_preserves_ownership() {
    let f = Fixture::new();
    let head = f.commit("candidate.txt");
    f.push();
    crash(&f, "before-db");
    let id = candidate(&f);
    let path = journal(&f);
    let mut altered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    altered["snapshot_id"] = uuid::Uuid::new_v4().to_string().into();
    let bytes = serde_json::to_vec(&altered).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(f.sync().retry);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
    assert_eq!(
        f.store
            .load_review_sync_candidate(f.task_id())
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn missing_receiver_reports_unavailable_without_losing_published_history() {
    let f = Fixture::new();
    let binding = &f.opened.review.bindings[0];
    let held = f._home.path().join("held-receiver.git");
    std::fs::rename(&binding.receiving_repository, &held).unwrap();
    assert!(f.sync().retry);
    assert_eq!(
        f.observation().health,
        crate::reviews::model::ReviewSyncHealth::Unavailable
    );
    assert_eq!(f.review(), f.opened.review);
    std::fs::rename(held, &binding.receiving_repository).unwrap();
    f.sync();
    assert_eq!(
        f.observation().health,
        crate::reviews::model::ReviewSyncHealth::Current
    );
}

#[test]
fn journal_cleanup_refusal_after_commit_preserves_invalidation_and_published_pins() {
    let f = Fixture::new();
    let head = f.commit("published.txt");
    f.push();
    let output = f._home.path().join("sync-result.json");
    let status = child_command(&f, "replace-journal")
        .env("BUILD_REVIEW_SYNC_TEST_JOURNAL", journal(&f))
        .env("BUILD_REVIEW_SYNC_TEST_RESULT", &output)
        .status()
        .unwrap();
    assert!(status.success());
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"persisted": true, "retry": true})
    );
    let id = f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .unwrap();
    assert_eq!(pin_oid(&f, &id).as_deref(), Some(head.as_str()));
    assert_eq!(
        std::fs::read_to_string(journal(&f)).unwrap(),
        "foreign replacement journal"
    );
    assert_eq!(f.review().snapshots.len(), 2);
}

#[test]
fn pr_without_git_bindings_is_noop_but_an_invalid_pending_candidate_is_preserved() {
    let f = Fixture::new();
    let conn = rusqlite::Connection::open(f._home.path().join("db/build.db")).unwrap();
    conn.execute(
        "DELETE FROM review_branch_bindings WHERE task_id = ?1",
        [f.task_id()],
    )
    .unwrap();
    conn.execute("UPDATE reviews SET record = json_set(record, '$.pull_request.directories[0].kind', 'live') WHERE task_id = ?1", [f.task_id()]).unwrap();
    let result = f.sync();
    assert!(!result.retry);
    assert!(!result.persisted);
    let id = uuid::Uuid::new_v4().to_string();
    f.store
        .register_review_sync_candidate(f.task_id(), &id)
        .unwrap();
    assert!(reconcile(&f.store, f.task_id()).is_err());
    assert_eq!(
        f.store.load_review_sync_candidate(f.task_id()).unwrap(),
        Some(id)
    );
}
