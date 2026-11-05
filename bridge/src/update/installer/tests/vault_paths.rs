use super::*;

#[test]
fn legacy_job_cannot_backup_a_task_tree_containing_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let mut job = fixture(dir.path());
    job.tasks_dir = job.home.join(".build");
    let vault = job.tasks_dir.join("secrets");
    fs::create_dir(&vault).unwrap();
    let key = b"synthetic vault fixture only";
    fs::write(vault.join("vault.key"), key).unwrap();
    let before = fs::read(&job.installed_binary).unwrap();
    let mut called_service = false;
    let outcome = install_with(
        &job,
        dir.path(),
        &job.installed_binary,
        &mut |_| {
            called_service = true;
            Ok(())
        },
        &mut |_, _| Ok(()),
    );
    assert!(outcome.unwrap_err().message().contains("vault"));
    assert!(!called_service);
    assert_eq!(fs::read(&job.installed_binary).unwrap(), before);
    assert_eq!(fs::read(vault.join("vault.key")).unwrap(), key);
    assert!(!job
        .tasks_dir
        .with_extension("bridge-update-attempt")
        .exists());
    assert!(!dir.path().join("previous-build-bridge").exists());
}

#[test]
fn new_jobs_and_old_job_recovery_refuse_vault_overlap_before_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let mut job = fixture(dir.path());
    job.tasks_dir = job.home.join(".build/secrets");
    fs::create_dir(&job.tasks_dir).unwrap();
    let result = create_job(
        &job.home,
        &job.tasks_dir,
        &job.installed_binary,
        "9.9.9",
        "00000000-0000-4000-8000-000000000001",
        false,
    );
    assert!(result.unwrap_err().contains("vault"));
    assert!(!updates_dir(&job.home).exists());
    let outcome = recover(&job, dir.path()).unwrap_err();
    assert!(outcome.pending());
    assert!(outcome.message().contains("vault"));
    assert!(job.tasks_dir.is_dir());
}
