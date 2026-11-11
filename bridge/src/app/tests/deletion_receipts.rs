//! Committed pin release survives a rollback and stale-plan abandonment.
use super::environment_deletion::call;
use super::project_agent::{added_project, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;

fn restart(root: &Path) -> AppState {
    rooted(root)
        .with_config(root.join("config.json"))
        .unwrap()
        .with_task_store(root.join("store"))
        .unwrap()
}

#[test]
fn project_deletion_return_trip_drains_committed_pin_receipts() {
    for blocked in [false, true] {
        committed_pin_receipts_survive_return_trip(blocked);
    }
}

fn committed_pin_receipts_survive_return_trip(blocked: bool) {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut app, project) = tracked(root.path());
    let id = workspace(&mut app, &project, "Rollback pin receipt");
    let review = call(
        &mut app,
        "tasks.review.open",
        json!({
            "workspace_id":id,"request_id":"rollback-receipt","title":"Pinned","description":"Review"
        }),
    );
    let task = review["task"]["id"].as_str().unwrap();
    let mut w = app.workspaces.get(&id).unwrap().clone();
    w.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&w).unwrap();
    app.workspaces.reload().unwrap();
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    app.project_deletion_failure = Some("pins");
    let failed = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    let receipt = assert_committed_history_receipt(&app, &failed, task);
    let pins: Vec<_> = receipt
        .pending_pins
        .iter()
        .flat_map(|p| &p.refs)
        .cloned()
        .collect();
    assert!(!pins.is_empty());
    for pin in &pins {
        assert!(git2::Repository::open_bare(&pin.repository)
            .unwrap()
            .find_reference(&pin.name)
            .is_ok());
    }
    // Old readers ignore this internal record kind. Hide only its principal
    // across the real close/open; no pin or history effect is simulated.
    app.store.as_ref().unwrap().writer_for_test().execute(
    "UPDATE mutation_commands SET principal='compatibility-old-reader' WHERE principal='internal/project-deletion'",[]).unwrap();
    drop(app);
    let mut old = restart(root.path());
    let readded = added_project(&mut old, &repo.path().join("repo"));
    let kept = filed(&mut old, &readded, "Keep rollback work");
    old.store.as_ref().unwrap().writer_for_test().execute(
    "UPDATE mutation_commands SET principal='internal/project-deletion' WHERE principal='compatibility-old-reader'",[]).unwrap();
    drop(old);
    let lock = pins[0].repository.join(format!("{}.lock", pins[0].name));
    if blocked {
        std::fs::write(&lock, b"busy pin release").unwrap();
    }
    let mut app = restart(root.path());
    assert!(app.project(&readded).is_some());
    assert!(app.project_deletion_fences.is_empty());
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":kept["id"],"body":"Preserved"}),
    );
    assert_eq!(
        app.project_deletion_records().unwrap()[0]
            .tombstone
            .as_deref(),
        Some("abandoned")
    );
    if blocked {
        assert_undrained_receipt_survives_collection(&app);
        drop(app);
        std::fs::remove_file(lock).unwrap();
        app = restart(root.path());
    }
    for pin in pins {
        assert!(
            git2::Repository::open_bare(&pin.repository)
                .unwrap()
                .find_reference(&pin.name)
                .is_err(),
            "committed receipt abandoned without draining {}",
            pin.name
        );
    }
    assert!(app.project_deletion_records().unwrap()[0]
        .pending_pins
        .is_empty());
    drop(app);
    assert!(restart(root.path()).project_deletion_fences.is_empty());
}

fn assert_committed_history_receipt(
    app: &AppState,
    failed: &Value,
    task: &str,
) -> crate::store::ProjectDeletion {
    assert_eq!(failed["ok"], false, "{failed}");
    let receipt = app.project_deletion_records().unwrap().pop().unwrap();
    assert_eq!(receipt.phase, "history_removed");
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_review(task)
        .unwrap()
        .is_none());
    receipt
}

fn assert_undrained_receipt_survives_collection(app: &AppState) {
    assert!(!app.project_deletion_records().unwrap()[0]
        .pending_pins
        .is_empty());
    app.store
        .as_ref()
        .unwrap()
        .writer_for_test()
        .execute("UPDATE mutation_commands SET terminal_at=0", [])
        .unwrap();
    app.store
        .as_ref()
        .unwrap()
        .writer()
        .collect_project_deletion_tombstones()
        .unwrap();
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .writer()
            .project_deletions()
            .unwrap()
            .len(),
        1
    );
}
