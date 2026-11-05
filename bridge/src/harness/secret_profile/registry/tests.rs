#[test]
fn registry_creates_only_private_leaf_and_preserves_normal_trusted_parent_links() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("state");
    std::fs::create_dir(&parent).unwrap();
    let link = root.path().join("state-link");
    std::os::unix::fs::symlink(&parent, &link).unwrap();
    let registry = super::Registry::open(&link.join("secret-profiles")).unwrap();
    assert_eq!(registry.directory.metadata().unwrap().mode() & 0o777, 0o700);
    let replacement = root.path().join("registry-link");
    std::os::unix::fs::symlink(parent.join("secret-profiles"), &replacement).unwrap();
    assert!(super::Registry::open(&replacement).is_err());
    assert!(
        super::Registry::open(&root.path().join("absent-parent").join("secret-profiles")).is_err()
    );
    assert!(!root.path().join("absent-parent").exists());
}
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

fn fixture() -> (tempfile::TempDir, Registry, std::path::PathBuf, File) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let directory = File::open(&home).unwrap();
    let registry = Registry::open(&root.path().join("registry")).unwrap();
    (root, registry, home, directory)
}
fn tracked(registry: &Registry, home: &Path, directory: &File) -> (RegistryGuard, File, String) {
    let name = format!("build-secret-{}.config.toml", uuid::Uuid::new_v4());
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(home.join(&name))
        .unwrap();
    let guard = registry
        .record(home, directory, &name, &file, None)
        .unwrap();
    (guard, file, name)
}
fn crash(guard: RegistryGuard, file: File) {
    guard
        .removed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    drop((guard, file));
}
fn records(registry: &Registry) -> usize {
    registry.names().unwrap().len()
}

#[test]
fn crash_leftover_is_removed_but_unregistered_user_profile_is_untouched() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    let user = home.join(format!("build-secret-{}.config.toml", uuid::Uuid::new_v4()));
    std::fs::write(&user, "user profile").unwrap();
    crash(guard, file);
    registry.sweep().unwrap();
    assert!(!home.join(name).exists());
    assert_eq!(std::fs::read_to_string(user).unwrap(), "user profile");
    assert_eq!(records(&registry), 0);
}
#[test]
fn same_name_replacement_is_preserved_and_record_dropped() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    std::fs::rename(home.join(&name), home.join("old-profile")).unwrap();
    std::fs::write(home.join(&name), "replacement").unwrap();
    crash(guard, file);
    registry.sweep().unwrap();
    assert_eq!(
        std::fs::read_to_string(home.join(name)).unwrap(),
        "replacement"
    );
    assert_eq!(records(&registry), 0);
}
#[test]
fn configured_home_symlink_switch_preserves_new_target() {
    let (root, registry, home, directory) = fixture();
    let link = root.path().join("linked-codex");
    symlink(&home, &link).unwrap();
    let (guard, file, name) = tracked(&registry, &link, &directory);
    let replacement = root.path().join("replacement");
    std::fs::create_dir(&replacement).unwrap();
    std::fs::write(replacement.join(&name), "replacement").unwrap();
    std::fs::remove_file(&link).unwrap();
    symlink(&replacement, &link).unwrap();
    crash(guard, file);
    registry.sweep().unwrap();
    assert_eq!(
        std::fs::read_to_string(replacement.join(name)).unwrap(),
        "replacement"
    );
    assert_eq!(records(&registry), 0);
}
#[test]
fn missing_profile_drops_only_record_and_sweep_never_creates_home() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    std::fs::remove_file(home.join(name)).unwrap();
    crash(guard, file);
    std::fs::remove_dir(&home).unwrap();
    registry.sweep().unwrap();
    assert!(!home.exists());
    assert_eq!(records(&registry), 0);
}
#[test]
fn live_profiles_hold_their_records_and_quota_prunes_only_stale_entries() {
    let (_root, registry, home, directory) = fixture();
    let (first, file1, name1) = tracked(&registry, &home, &directory);
    let (second, file2, name2) = tracked(&registry, &home, &directory);
    assert!(registry.admit(2).is_err());
    assert!(home.join(&name1).exists() && home.join(&name2).exists());
    crash(first, file1);
    registry.admit(2).unwrap();
    assert!(!home.join(name1).exists());
    assert!(home.join(name2).exists());
    assert_eq!(records(&registry), 1);
    drop((second, file2));
}
#[test]
fn pending_and_final_registration_share_exact_inode_and_guard_cleanup() {
    let (_root, registry, home, directory) = fixture();
    let id = uuid::Uuid::new_v4();
    let pending = format!(".build-secret-{id}.pending");
    let final_name = format!("build-secret-{id}.config.toml");
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(home.join(&pending))
        .unwrap();
    let guard = registry
        .record(&home, &directory, &final_name, &file, Some(&pending))
        .unwrap();
    assert_eq!(records(&registry), 1);
    std::fs::rename(home.join(&pending), home.join(&final_name)).unwrap();
    guard.remove().unwrap();
    assert!(!home.join(final_name).exists());
    assert_eq!(records(&registry), 0);
}

#[test]
fn crashed_metadata_preserves_live_profile_flock_then_retires_after_close() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    guard.removed.store(true, Ordering::SeqCst);
    drop(guard);
    registry.sweep().unwrap();
    assert!(home.join(&name).exists());
    assert_eq!(records(&registry), 1);
    drop(file);
    registry.sweep().unwrap();
    assert!(!home.join(name).exists());
    assert_eq!(records(&registry), 0);
}
#[test]
fn crash_before_pending_publication_retires_exact_staged_inode() {
    let (_root, registry, home, directory) = fixture();
    let id = uuid::Uuid::new_v4();
    let pending = format!(".build-secret-{id}.pending");
    let final_name = format!("build-secret-{id}.config.toml");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(home.join(&pending))
        .unwrap();
    let guard = registry
        .record(&home, &directory, &final_name, &file, Some(&pending))
        .unwrap();
    crash(guard, file);
    registry.sweep().unwrap();
    assert!(!home.join(pending).exists());
    assert_eq!(records(&registry), 0);
}
#[test]
fn guard_uses_held_home_after_configured_symlink_changes() {
    let (root, registry, home, directory) = fixture();
    let link = root.path().join("linked-home");
    symlink(&home, &link).unwrap();
    let (guard, file, name) = tracked(&registry, &link, &directory);
    let replacement = root.path().join("replacement");
    std::fs::create_dir(&replacement).unwrap();
    std::fs::write(replacement.join(&name), "user profile").unwrap();
    std::fs::remove_file(&link).unwrap();
    symlink(&replacement, &link).unwrap();
    guard.remove().unwrap();
    assert!(!home.join(&name).exists());
    assert_eq!(
        std::fs::read_to_string(replacement.join(name)).unwrap(),
        "user profile"
    );
    assert_eq!(records(&registry), 0);
    drop(file);
}
#[test]
fn wrong_owner_record_never_authorizes_profile_deletion() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    let registry_name = guard.name.clone();
    crash(guard, file);
    let mut record: serde_json::Value = serde_json::from_reader(File::from(
        fs::openat(
            &registry.directory,
            registry_name.as_str(),
            READ_FLAGS,
            Mode::empty(),
        )
        .unwrap(),
    ))
    .unwrap();
    record["uid"] = serde_json::json!(uid().wrapping_add(1));
    let mut metadata: File = fs::openat(
        &registry.directory,
        registry_name.as_str(),
        OFlags::WRONLY | OFlags::TRUNC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .unwrap()
    .into();
    serde_json::to_writer(&mut metadata, &record).unwrap();
    drop(metadata);
    registry.sweep().unwrap();
    assert!(home.join(name).exists());
    assert_eq!(records(&registry), 0);
}
#[test]
fn registry_root_must_be_private_and_profiles_must_have_private_owned_metadata() {
    let (root, registry, home, directory) = fixture();
    std::fs::set_permissions(
        root.path().join("registry"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(Registry::open(&root.path().join("registry")).is_err());
    let name = format!("build-secret-{}.config.toml", uuid::Uuid::new_v4());
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o644)
        .open(home.join(&name))
        .unwrap();
    assert!(registry
        .record(&home, &directory, &name, &file, None)
        .is_err());
    assert_eq!(records(&registry), 0);
}

#[test]
fn home_lookup_failures_are_isolated_using_current_permissions() {
    for mode in [0o000, 0o400, 0o500] {
        let (root, registry, home, directory) = fixture();
        let (guard, file, name) = tracked(&registry, &home, &directory);
        crash(guard, file);
        let healthy = root.path().join("healthy-codex");
        std::fs::create_dir(&healthy).unwrap();
        let healthy_directory = File::open(&healthy).unwrap();
        let (guard, file, healthy_name) = tracked(&registry, &healthy, &healthy_directory);
        crash(guard, file);
        std::fs::write(home.join("permission-probe"), "").unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(mode)).unwrap();
        // Root can legitimately bypass these mode restrictions. Probe the real
        // operation so every assertion also runs when tests execute as root.
        let deletion_allowed =
            fs::unlinkat(&directory, "permission-probe", AtFlags::empty()).is_ok();
        let result = registry.sweep();
        // Restore access before assertions so TempDir always removes the fixture.
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            result.is_ok(),
            "home mode {mode:o} must not block registry cleanup: {result:?}"
        );
        assert_eq!(
            home.join(name).exists(),
            !deletion_allowed,
            "home mode {mode:o}: preserve inaccessible profiles, clean accessible ones"
        );
        assert!(
            !healthy.join(healthy_name).exists(),
            "other registered homes are still cleaned"
        );
        assert_eq!(
            records(&registry),
            0,
            "inaccessible stale metadata cannot consume quota"
        );
    }
}

#[test]
fn guard_drops_registry_record_even_when_profile_permissions_change() {
    let (_root, registry, home, directory) = fixture();
    let (guard, file, name) = tracked(&registry, &home, &directory);
    std::fs::write(home.join("permission-probe"), "").unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o500)).unwrap();
    let deletion_allowed = fs::unlinkat(&directory, "permission-probe", AtFlags::empty()).is_ok();
    let result = guard.remove();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.is_ok(), deletion_allowed);
    assert_eq!(home.join(name).exists(), !deletion_allowed);
    assert_eq!(
        records(&registry),
        0,
        "session end always retires private metadata"
    );
    assert!(guard.removed.load(Ordering::Acquire));
    drop(file);
}

#[test]
fn crash_after_final_hardlink_publication_retires_both_exact_names() {
    let (_root, registry, home, directory) = fixture();
    let id = uuid::Uuid::new_v4();
    let pending = format!(".build-secret-{id}.pending");
    let final_name = format!("build-secret-{id}.config.toml");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(home.join(&pending))
        .unwrap();
    let guard = registry
        .record(&home, &directory, &final_name, &file, Some(&pending))
        .unwrap();
    fs::linkat(
        &directory,
        pending.as_str(),
        &directory,
        final_name.as_str(),
        AtFlags::empty(),
    )
    .unwrap();
    let pending_metadata = std::fs::metadata(home.join(&pending)).unwrap();
    let final_metadata = std::fs::metadata(home.join(&final_name)).unwrap();
    assert_eq!(pending_metadata.ino(), final_metadata.ino());
    assert_eq!(pending_metadata.nlink(), 2);
    crash(guard, file);
    registry.sweep().unwrap();
    assert!(!home.join(pending).exists());
    assert!(!home.join(final_name).exists());
    assert_eq!(records(&registry), 0);
}
