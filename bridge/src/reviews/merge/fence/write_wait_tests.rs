use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::native_retry::{assert_continuous, collect, Observer};

fn contested_refs(
    root: &Path,
) -> (
    OwnedGitFileLock,
    BTreeMap<PathBuf, BTreeMap<String, Option<String>>>,
) {
    let git = root.join("git");
    std::fs::create_dir(&git).unwrap();
    let (holder, _) =
        receivers::locks::acquire_git_lock(&git, Path::new("refs/heads/main.lock")).unwrap();
    let refs = BTreeMap::from([(git, BTreeMap::from([("refs/heads/main".into(), None)]))]);
    (holder, refs)
}

#[test]
fn merge_ref_retries_retain_one_wait_until_owned_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let (holder, refs) = contested_refs(root.path());
    let tracked = TrackedWrite::new("merge-ref-retries");
    let context = tracked.context();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _context = context.bind_sync();
        let _observer = Observer::bind(sender);
        lock_refs(&refs)
    });
    let attempts = collect(&receiver, 3);
    drop(holder);
    let guards = worker.join().unwrap().unwrap();
    let owned = guards[0].verify_owned();
    let path = guards[0].path().to_owned();
    let exists_while_owned = path.exists();
    let excluded = receivers::locks::acquire_git_lock(
        &root.path().join("git"),
        Path::new("refs/heads/main.lock"),
    )
    .is_err();
    drop(guards);
    assert_continuous(&tracked, &attempts, "review.merge.refs");
    assert!(owned.is_ok() && exists_while_owned && excluded);
    assert!(!path.exists(), "owned ref reservation was not released");
    tracked.complete();
}

#[test]
fn merge_ref_retry_timeout_preserves_the_existing_two_second_deadline() {
    let root = tempfile::tempdir().unwrap();
    let (holder, refs) = contested_refs(root.path());
    let tracked = TrackedWrite::new("merge-ref-timeout");
    let context = tracked.context();
    let (sender, receiver) = std::sync::mpsc::channel();
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        let _context = context.bind_sync();
        let _observer = Observer::bind(sender);
        lock_refs(&refs)
    });
    let result = worker.join().unwrap();
    let elapsed = started.elapsed();
    let owned = holder.verify_owned();
    drop(holder);
    let attempts = receiver.try_iter().collect::<Vec<_>>();
    assert!(result.err().unwrap().contains("locked"));
    assert!(elapsed >= REF_WAIT, "production timeout was shortened");
    assert!(owned.is_ok(), "timeout changed the original native lock");
    assert_continuous(&tracked, &attempts, "review.merge.refs");
    tracked.complete();
}
