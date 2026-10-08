use super::*;

#[cfg(unix)]
#[test]
fn cleanup_removes_durable_authority_before_unlinking_the_backing_inode() {
    let directory = tempfile::tempdir().unwrap();
    let (guard, _file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    let observed = std::cell::Cell::new(false);
    cleanup_registered_files(
        &guard.path,
        &guard.backing,
        &guard.marker,
        &guard.owner,
        || {
            assert!(!guard.path.exists());
            assert!(!guard.marker.exists());
            assert!(same_lock_file(&guard.backing, &guard.owner));
            assert!(file_has_identity(&guard.inode_pin, &guard.owner));
            observed.set(true);
        },
    )
    .unwrap();
    assert!(observed.get());
    assert!(!guard.backing.exists());
}

#[cfg(unix)]
#[test]
fn changed_authority_keeps_the_backing_inode_reserved() {
    let directory = tempfile::tempdir().unwrap();
    let (guard, _file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    fs::write(&guard.marker, "user marker\n").unwrap();
    let error = cleanup_registered_files(
        &guard.path,
        &guard.backing,
        &guard.marker,
        &guard.owner,
        || panic!("changed authority must block backing unlink"),
    )
    .unwrap_err();
    assert!(error.contains("marker changed"));
    assert!(same_lock_file(&guard.backing, &guard.owner));
    assert_eq!(fs::read_to_string(&guard.marker).unwrap(), "user marker\n");
}

#[cfg(unix)]
#[test]
fn a_stale_marker_without_its_backing_cannot_claim_a_reused_user_inode() {
    let directory = tempfile::tempdir().unwrap();
    let (guard, _file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    fs::remove_file(&guard.path).unwrap();
    fs::remove_file(&guard.backing).unwrap();
    fs::write(&guard.path, "user lock\n").unwrap();
    // Model the stale identity after inode reuse deterministically; the absent
    // backing must invalidate authority even when the real lock identity agrees.
    let (device, inode) = lock_file_identity(&guard.path).unwrap();
    let mut stale = guard.owner.clone();
    stale.device = Some(device);
    stale.inode = Some(inode);
    stale.boot = Some("an earlier machine boot".into());
    fs::write(&guard.marker, serde_json::to_vec(&stale).unwrap()).unwrap();
    recover_git_locks(directory.path()).unwrap();
    assert_eq!(fs::read_to_string(&guard.path).unwrap(), "user lock\n");
    assert!(guard.marker.exists());
}

#[cfg(unix)]
#[test]
fn overlapping_reapers_keep_the_original_inode_pinned_until_cleanup_ends() {
    let directory = tempfile::tempdir().unwrap();
    let (guard, _file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    let second_reaper = open_backing_inode(&guard.backing, &guard.owner)
        .unwrap()
        .unwrap();
    cleanup_registered_files(
        &guard.path,
        &guard.backing,
        &guard.marker,
        &guard.owner,
        || {},
    )
    .unwrap();
    fs::write(&guard.path, "new user lock\n").unwrap();
    assert!(file_has_identity(&second_reaper, &guard.owner));
    assert!(!same_lock_file(&guard.path, &guard.owner));
    cleanup_registered_files(
        &guard.path,
        &guard.backing,
        &guard.marker,
        &guard.owner,
        || {},
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&guard.path).unwrap(), "new user lock\n");
}

#[cfg(unix)]
#[test]
fn concurrent_recovery_waits_until_old_authority_is_removed() {
    use std::sync::mpsc;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let (guard, _file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    let mut dead = guard.owner.clone();
    dead.boot = Some("an earlier machine boot".into());
    fs::write(&guard.marker, serde_json::to_vec(&dead).unwrap()).unwrap();
    let recovery = RecoveryDirectoryLock::acquire(directory.path()).unwrap();
    let directory_path = directory.path().to_path_buf();
    let (started_sender, started_receiver) = mpsc::channel();
    let (finished_sender, finished_receiver) = mpsc::channel();
    let reaper = std::thread::spawn(move || {
        started_sender.send(()).unwrap();
        finished_sender
            .send(recover_git_locks(&directory_path))
            .unwrap();
    });
    started_receiver.recv().unwrap();
    assert!(matches!(
        finished_receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    cleanup_registered_files(&guard.path, &guard.backing, &guard.marker, &dead, || {
        fs::write(&guard.path, "new user lock\n").unwrap();
    })
    .unwrap();
    drop(recovery);
    finished_receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    reaper.join().unwrap();
    assert_eq!(fs::read_to_string(&guard.path).unwrap(), "new user lock\n");
}

#[cfg(unix)]
#[test]
fn published_file_keeps_its_real_git_lock_until_guard_release() {
    use std::io::Write;

    let directory = tempfile::tempdir().unwrap();
    let (guard, mut file) = acquire_git_lock(directory.path(), Path::new("config.lock")).unwrap();
    file.write_all(b"published config\n").unwrap();
    file.sync_all().unwrap();
    let target = directory.path().join("config");
    guard.publish_retaining_lock(&target).unwrap();
    assert!(same_lock_file(&guard.path, &guard.owner));
    assert!(same_lock_file(&target, &guard.owner));
    assert!(acquire_git_lock(directory.path(), Path::new("config.lock"))
        .err()
        .unwrap()
        .contains("prove it owns"));
    drop(guard);
    assert!(!directory.path().join("config.lock").exists());
    assert_eq!(fs::read_to_string(target).unwrap(), "published config\n");
}
