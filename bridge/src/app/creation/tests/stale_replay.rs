use super::admission_recovery::{register_legacy, SimulatedCrash};
use super::*;
use crate::workspace::Workspace;

enum InterruptedStep {
    OwnedRoot,
    Published,
    Cleanup,
}

struct InterruptedWorkspace {
    fixture: Fixture,
    operation: String,
    workspace: Workspace,
    project_config: Vec<u8>,
}

fn interrupt_and_reuse_id(step: InterruptedStep) -> InterruptedWorkspace {
    let f = Fixture::new();
    let config = f._root.path().join("config.json");
    let old_config = std::fs::read(&config).unwrap();
    let home = f._root.path().join("original");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("source"), b"original source").unwrap();
    {
        let mut app = f.state.lock().unwrap();
        assert_eq!(app.add_project(home, "main".into()), "proj-2");
        app.persist();
    }
    let project_config = std::fs::read(&config).unwrap();
    let id = operation(150);
    let record = admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-2","name":"interrupted"}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    match step {
        InterruptedStep::Published => {
            let observer = observe_boundary(|boundary| {
                if boundary == Boundary::WorkspaceProvisioned {
                    std::panic::panic_any(SimulatedCrash);
                }
            });
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
            assert!(interrupted.unwrap_err().is::<SimulatedCrash>());
            drop(observer);
            assert_eq!(f.record(&id).state, "executing");
            assert!(f.record(&id).outcome.is_none());
            assert_eq!(
                f.state
                    .lock()
                    .unwrap()
                    .workspaces
                    .get(&workspace.id)
                    .unwrap()
                    .status,
                crate::workspace::WorkspaceStatus::Ready
            );
        }
        InterruptedStep::OwnedRoot | InterruptedStep::Cleanup => {
            assert!(f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap());
            f.state
                .lock()
                .unwrap()
                .workspaces
                .claim_creation_workspace(&workspace)
                .unwrap();
            if matches!(step, InterruptedStep::Cleanup) {
                let mut evidence = f.record(&id).execution;
                evidence["residue"] =
                    json!({"paths":[workspace.root],"error":"interrupted cleanup"});
                f.store
                    .writer()
                    .park_creation(PRINCIPAL, &id, "reconciling", evidence)
                    .unwrap();
            }
        }
    }
    std::fs::write(workspace.root.join("owned"), b"must remain").unwrap();
    std::fs::write(config, old_config).unwrap();
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    register_legacy(&f.state, f._root.path());
    assert_ne!(
        f.record(&id).execution["project_key"],
        f.state
            .lock()
            .unwrap()
            .projects
            .get("proj-2")
            .unwrap()
            .project_key
    );
    InterruptedWorkspace {
        fixture: f,
        operation: id,
        workspace,
        project_config,
    }
}

fn assert_retained_unknown(interrupted: &InterruptedWorkspace) {
    let InterruptedWorkspace {
        fixture: f,
        operation: id,
        workspace,
        ..
    } = interrupted;
    let evidence = f.record(id).execution;
    let reservations = f.record(id).reservations;
    let legacy = {
        let app = f.state.lock().unwrap();
        app.project_json(app.projects.get("proj-2").unwrap())
    };
    let ownership = f
        .state
        .lock()
        .unwrap()
        .workspaces
        .root()
        .join(".creation-reservations")
        .join(&workspace.id);
    let ownership_bytes = std::fs::read(&ownership).unwrap();
    let legacy_marker = f._root.path().join("legacy/owned");
    std::fs::write(&legacy_marker, b"legacy-owned").unwrap();
    for _ in 0..2 {
        f.run(id);
        let record = f.record(id);
        assert_eq!(record.state, "unknown", "{record:?}");
        assert!(record.outcome.is_none());
        assert_eq!(record.execution, evidence);
        assert_eq!(record.reservations, reservations);
        let app = f.state.lock().unwrap();
        assert!(app.workspace_reserved(&workspace.id));
        assert_eq!(
            app.project_json(app.projects.get("proj-2").unwrap()),
            legacy
        );
        drop(app);
        assert_eq!(
            std::fs::read(workspace.root.join("owned")).unwrap(),
            b"must remain"
        );
        assert_eq!(std::fs::read(&ownership).unwrap(), ownership_bytes);
        assert_eq!(std::fs::read(&legacy_marker).unwrap(), b"legacy-owned");
    }
    let command_id = f.record(id).command_id;
    let connection = f.store.writer_for_test();
    let terminal: i64 = connection
        .query_row(
            "SELECT count(*) FROM mutation_outbox WHERE command_id=?1",
            [command_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terminal, 0, "unknown must not confirm a terminal result");
    let active: i64 = connection
        .query_row(
            "SELECT count(*) FROM mutation_owner_order WHERE command_id=?1 AND active=1",
            [command_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 1);
    drop(connection);
    assert!(
        admit(
            &f.state,
            "workspace.create",
            f.record(id).params,
            WriteMetadata {
                operation_id: operation(151),
                after: None,
            }
        )
        .is_err(),
        "unresolved intent must block a conflicting retry"
    );
    let mut continued = f.record(id).params;
    continued["continues"] = json!(id);
    assert!(
        admit(
            &f.state,
            "workspace.create",
            continued,
            WriteMetadata {
                operation_id: operation(152),
                after: None,
            }
        )
        .is_err(),
        "unknown must not offer a continuation"
    );
}

#[test]
fn stale_key_replay_keeps_owned_root_unknown_and_reserved() {
    let interrupted = interrupt_and_reuse_id(InterruptedStep::OwnedRoot);
    assert_retained_unknown(&interrupted);
}

#[test]
fn stale_key_replay_keeps_published_workspace_unknown_and_reserved() {
    let interrupted = interrupt_and_reuse_id(InterruptedStep::Published);
    let workspace_before = interrupted
        .fixture
        .state
        .lock()
        .unwrap()
        .workspaces
        .get(&interrupted.workspace.id)
        .unwrap()
        .clone();
    assert_retained_unknown(&interrupted);
    assert_eq!(
        interrupted
            .fixture
            .state
            .lock()
            .unwrap()
            .workspaces
            .get(&interrupted.workspace.id)
            .unwrap(),
        &workspace_before
    );
    assert_eq!(
        std::fs::read(workspace_before.directories[0].path.join("source")).unwrap(),
        b"original source"
    );
}

#[test]
fn stale_key_cleanup_keeps_residue_unknown_and_reserved() {
    let interrupted = interrupt_and_reuse_id(InterruptedStep::Cleanup);
    assert_retained_unknown(&interrupted);
    // Restoring the original authority must resume cleanup, not provisioning.
    let InterruptedWorkspace {
        fixture: f,
        operation: id,
        workspace,
        project_config,
    } = interrupted;
    std::fs::write(f._root.path().join("config.json"), project_config).unwrap();
    let f = f.reopen();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert!(record.execution.get("residue").is_none());
    assert!(!workspace.root.exists());
    assert!(!f.state.lock().unwrap().workspace_reserved(&workspace.id));
    assert!(f.state.lock().unwrap().workspaces.list(None).is_empty());
}
