//! Bounded owner association keys are exact UTF-8 paths, separate from the
//! descriptive identity JSON and the unbounded canonical owner record.
use super::*;

#[test]
fn summary_identity_owner_path_preserves_the_unicode_byte_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = support::run_record("unicode-owner", None, support::NOW);
    run.worktree_path = "é".repeat(2048);
    let agent = run.agents[0].id.clone();
    store.save_run(&run).unwrap();
    let (identity, path) = store
        .task_summary_identity_with_owner(&agent)
        .unwrap()
        .unwrap();
    assert_eq!(path.as_deref(), Some(run.worktree_path.as_str()));
    assert_eq!(path.unwrap().len(), 4096);
    assert_eq!(identity.agent_id, agent);
    assert_eq!(
        identity.workspace_name.as_deref(),
        Some(run.worktree_name.as_str())
    );
}

#[test]
fn summary_identity_owner_path_overflow_is_an_explicit_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = support::run_record("oversize-owner", None, support::NOW);
    run.worktree_path = format!("{}x", "é".repeat(2048));
    let agent = run.agents[0].id.clone();
    // Derived limits do not reject or silently alter the canonical record.
    store.save_run(&run).unwrap();
    assert_eq!(
        support::reload_run(&store, &run.id).worktree_path,
        run.worktree_path
    );
    let error = store.task_summary_identity_with_owner(&agent).unwrap_err();
    assert!(matches!(&error, StoreError::TaskSummaryInvalid(_)));
    assert!(error.to_string().contains("4096 bytes"));
    assert!(store.task_summary_identity(&agent).is_err());
    run.worktree_path = "/repaired-owner".into();
    store.save_run(&run).unwrap();
    let (_, path) = store
        .task_summary_identity_with_owner(&agent)
        .unwrap()
        .unwrap();
    assert_eq!(path.as_deref(), Some("/repaired-owner"));
}

#[test]
fn summary_identity_owner_path_updates_and_rolls_back_with_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = support::run_record("changing-owner", None, support::NOW);
    run.worktree_path = "/before".into();
    let agent = run.agents[0].id.clone();
    store.save_run(&run).unwrap();
    run.worktree_path = "/committed".into();
    store.save_run(&run).unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_owner_path BEFORE UPDATE ON tracker_task_summary_identities BEGIN SELECT RAISE(ABORT,'owner path failure'); END;").unwrap();
    run.worktree_path = "/rolled-back".into();
    assert!(store.save_run(&run).is_err());
    assert_eq!(
        support::reload_run(&store, &run.id).worktree_path,
        "/committed"
    );
    let (_, path) = store
        .task_summary_identity_with_owner(&agent)
        .unwrap()
        .unwrap();
    assert_eq!(path.as_deref(), Some("/committed"));
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_owner_path")
        .unwrap();
    store.delete_run(&run.id).unwrap();
    assert!(store
        .task_summary_identity_with_owner(&agent)
        .unwrap()
        .is_none());
}
