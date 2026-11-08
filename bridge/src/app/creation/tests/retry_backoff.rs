use super::super::service::{reconcile_pending, RetryBackoff};
use super::*;
use std::time::{Duration, Instant};

fn unknown_workspace(f: &Fixture, id: &str) {
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-1","name":"retry"}),
        WriteMetadata {
            operation_id: id.into(),
            after: None,
        },
    )
    .unwrap();
    f.store.writer().claim_mutation(PRINCIPAL, id).unwrap();
    let record = f.record(id);
    f.store
        .writer()
        .park_creation(PRINCIPAL, id, "unknown", record.execution)
        .unwrap();
    f.state.lock().unwrap().projects.remove("proj-1");
}

#[test]
fn unknown_record_retry_backs_off() {
    let f = Fixture::new();
    let id = operation(210);
    unknown_workspace(&f, &id);
    let mut retries = RetryBackoff::default();
    let writer = f.store.writer();
    let mut now = Instant::now();
    let mut delay = 30;
    // Exercise exponential growth, the cap, and explicit wakeups before due.
    for _ in 0..10 {
        let before = f.record(&id).revision;
        reconcile_pending(&f.state, &writer, &mut retries, now).unwrap();
        assert!(f.record(&id).revision > before);
        assert_eq!(f.record(&id).state, "unknown");
        let after = f.record(&id).revision;
        reconcile_pending(
            &f.state,
            &writer,
            &mut retries,
            now + Duration::from_secs(delay - 1),
        )
        .unwrap();
        assert_eq!(
            f.record(&id).revision,
            after,
            "retried before {delay}s delay"
        );
        now += Duration::from_secs(delay);
        delay = (delay * 2).min(1800);
    }
    // A new unrelated command must not inherit this record's backoff.
    let other = f.admit(&operation(211));
    reconcile_pending(
        &f.state,
        &writer,
        &mut retries,
        now - Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(f.record(&other.operation_id).state, "succeeded");
}

#[test]
fn recover_existing_ignores_retry_backoff() {
    let f = Fixture::new();
    let id = operation(212);
    f.admit(&id);
    let observer = observe_boundary(|boundary| {
        if boundary == Boundary::WorkspaceProvisioned {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)))
            .unwrap_err()
            .is::<SimulatedCrash>()
    );
    drop(observer);
    let workspace: crate::workspace::Workspace =
        serde_json::from_value(f.record(&id).execution["workspace"].clone()).unwrap();
    std::fs::write(workspace.directories[0].path.join("source"), b"user edits").unwrap();
    f.store.writer().recover_mutations().unwrap();
    f.state.lock().unwrap().projects.remove("proj-2");
    // Hide the landed project's config as well as its live binding. Its
    // environment is temporarily unavailable, preventing recreation by key.
    f.state.lock().unwrap().projects = crate::app::projects::ProjectRegistry::new();
    let now = Instant::now();
    let mut retries = RetryBackoff::default();
    reconcile_pending(&f.state, &f.store.writer(), &mut retries, now).unwrap();
    assert_eq!(f.record(&id).state, "unknown");
    let revision = f.record(&id).revision;
    reconcile_pending(
        &f.state,
        &f.store.writer(),
        &mut retries,
        now + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(f.record(&id).revision, revision);
    // Reopening restores project authority and starts a fresh worker schedule.
    let f = f.reopen();
    let mut restarted_retries = RetryBackoff::default();
    reconcile_pending(
        &f.state,
        &f.store.writer(),
        &mut restarted_retries,
        now + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(
        std::fs::read(workspace.directories[0].path.join("source")).unwrap(),
        b"user edits"
    );
    assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
}
