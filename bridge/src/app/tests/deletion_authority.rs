//! Admission claims and owed receipts survive unrelated publication and mounts.
use super::environment_deletion::call;
use super::project_agent::{rooted, workspace};
use super::tracker::{filed, tracked};
use super::workflow::lifecycle_offlock::frame_on_a_thread;
use super::*;

fn restart(root: &Path) -> AppState {
    rooted(root)
        .with_config(root.join("config.json"))
        .unwrap()
        .with_task_store(root.join("store"))
        .unwrap()
}

#[test]
fn project_deletion_provisional_claim_survives_sibling_publication_after_abandonment() {
    for fail_record in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let sibling = app.add_environment_sibling_for_test(&project);
        let claimant = app.add_environment_sibling_for_test(&project);
        let task = filed(&mut app, &project, "Fresh deletion scope");
        let id = workspace(&mut app, &project, "Rollback work");
        let owned = app.workspaces.get(&id).unwrap().root.clone();
        let extra = root.path().join("extra-source");
        std::fs::create_dir(&extra).unwrap();
        app.config_path = Some(root.path().join("config.json"));
        app.persist();
        app.project_deletion_failure = Some("checkout");
        let params = json!({"project_id":project,"confirm":true});
        assert_eq!(
            app.handle(req("project.delete", params.clone()))["ok"],
            false
        );
        // Model rollback work; the actual startup restores the same project ID
        // and abandons the old plan, without resuming its owned cleanup.
        std::fs::write(owned.join("rollback-work"), b"preserve").unwrap();
        drop(app);
        let mut app = restart(root.path());
        assert!(app.project(&project).is_some());
        assert_eq!(
            app.project_deletion_records().unwrap()[0]
                .tombstone
                .as_deref(),
            Some("abandoned")
        );
        call(
            &mut app,
            "tasks.comment",
            json!({"task_id":task["id"],"body":"Before admission"}),
        );
        let (gate, handle) = OffLockGate::new();
        app.off_lock_gate = Some(gate);
        app.project_deletion_failure = fail_record.then_some("record");
        let shared = app.shared();
        let fresh = frame_on_a_thread(&shared, "fresh", "project.delete", params);
        handle.wait_for_arrival();
        {
            let mut app = shared.lock().unwrap();
            app.off_lock_gate = None;
            app.project_deletion_failure = None;
        }
        let sibling_deleted = frame_on_a_thread(
            &shared,
            "sibling",
            "project.delete",
            json!({"project_id":sibling,"confirm":true}),
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
        assert_eq!(sibling_deleted["ok"], true, "{sibling_deleted}");
        let comment = frame_on_a_thread(
            &shared,
            "write",
            "tasks.comment",
            json!({"task_id":task["id"],"body":"After admission"}),
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
        let source = frame_on_a_thread(
            &shared,
            "source",
            "project.add_source",
            json!({"project_id":project,"path":extra,"name":"After admission"}),
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
        let owned_source = frame_on_a_thread(
            &shared,
            "owned-source",
            "project.add_source",
            json!({"project_id":claimant,"path":owned,"name":"Cannot acquire deleting scope"}),
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
        handle.release();
        let finished = fresh.recv_timeout(Duration::from_secs(30)).unwrap();
        for reply in [comment, source, owned_source] {
            assert_eq!(reply["ok"], false, "provisional claim lost: {reply}");
            assert!(
                reply["error"]
                    .as_str()
                    .unwrap()
                    .contains("deletion-pending"),
                "{reply}"
            );
        }
        assert_eq!(finished["ok"], !fail_record, "{finished}");
        if fail_record {
            let reply = frame_on_a_thread(
                &shared,
                "after-failure",
                "tasks.comment",
                json!({"task_id":task["id"],"body":"Usable after admission failed"}),
            )
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
            assert_eq!(
                reply["ok"], true,
                "failed admission must release its claim: {reply}"
            );
            assert_eq!(
                std::fs::read(owned.join("rollback-work")).unwrap(),
                b"preserve"
            );
        } else {
            assert!(!owned.exists());
        }
    }
}

fn pinned_review(app: &mut AppState, project: &str) -> (String, crate::workspace::Workspace) {
    let id = workspace(app, project, "Unavailable pin repository");
    let opened = call(
        app,
        "tasks.review.open",
        json!({"workspace_id":id,
        "request_id":"unavailable-repository","title":"Pinned","description":"Review"}),
    );
    let mut w = app.workspaces.get(&id).unwrap().clone();
    w.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&w).unwrap();
    app.workspaces.reload().unwrap();
    (opened["task"]["id"].as_str().unwrap().into(), w)
}

#[test]
fn project_deletion_unavailable_pin_repository_retains_receipt_until_restart() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (_task, w) = pinned_review(&mut app, &project);
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    app.project_deletion_failure = Some("pins");
    let failed = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(failed["ok"], false, "{failed}");
    let receipt = app.project_deletion_records().unwrap().pop().unwrap();
    assert_eq!(receipt.phase, "history_removed");
    let pins: Vec<_> = receipt
        .pending_pins
        .iter()
        .flat_map(|p| &p.refs)
        .cloned()
        .collect();
    assert!(!pins.is_empty());
    let repos: std::collections::BTreeSet<_> = pins.iter().map(|p| p.repository.clone()).collect();
    // An old binary creates work after files were removed. Recovery must
    // abandon stale file authority while retaining the independent pin debt.
    std::fs::create_dir_all(&w.root).unwrap();
    std::fs::write(w.root.join("rollback-work"), b"keep").unwrap();
    drop(app);
    let unavailable = root.path().join("unavailable");
    std::fs::create_dir(&unavailable).unwrap();
    let mut moved = Vec::new();
    for (index, repo) in repos.into_iter().enumerate() {
        let saved = unavailable.join(index.to_string());
        std::fs::rename(&repo, &saved).unwrap();
        moved.push((repo, saved));
    }
    let app = restart(root.path());
    let pending = app.project_deletion_records().unwrap().pop().unwrap();
    assert_eq!(pending.tombstone.as_deref(), Some("abandoned"));
    assert!(
        !pending.pending_pins.is_empty(),
        "unavailable repositories were falsely drained"
    );
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
    drop(app);
    for (repo, saved) in moved {
        std::fs::rename(saved, repo).unwrap();
    }
    let app = restart(root.path());
    assert!(app.project_deletion_records().unwrap()[0]
        .pending_pins
        .is_empty());
    for pin in pins {
        let repo = git2::Repository::open_bare(&pin.repository).unwrap();
        assert_eq!(
            repo.find_reference(&pin.name).err().unwrap().code(),
            git2::ErrorCode::NotFound
        );
    }
    assert_eq!(
        std::fs::read(w.root.join("rollback-work")).unwrap(),
        b"keep"
    );
}

#[test]
fn history_pin_preflight_records_unavailable_repository_debt() {
    for placeholder in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let (task, _) = pinned_review(&mut app, &project);
        let review = app
            .store
            .as_ref()
            .unwrap()
            .load_review(&task)
            .unwrap()
            .unwrap();
        let plans = crate::reviews::history_pins::plan(&review).unwrap();
        let repository = &plans.refs[0].repository;
        let saved = root.path().join("unavailable-repository");
        std::fs::rename(repository, &saved).unwrap();
        if placeholder {
            std::fs::create_dir(repository).unwrap();
        }
        let planned = crate::reviews::history_pins::plan(&review);
        if placeholder {
            std::fs::remove_dir(repository).unwrap();
        }
        std::fs::rename(saved, repository).unwrap();
        let planned = serde_json::to_value(planned.unwrap()).unwrap();
        assert!(
            planned["deferred_snapshots"]
                .as_array()
                .is_some_and(|s| !s.is_empty()),
            "unavailable snapshot target has no deferred receipt: {planned}"
        );
    }
}

#[test]
fn project_delete_missing_source_completes_with_owed_pin_receipts() {
    for placeholder in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let (task, w) = pinned_review(&mut app, &project);
        let review = app
            .store
            .as_ref()
            .unwrap()
            .load_review(&task)
            .unwrap()
            .unwrap();
        let pins = crate::reviews::history_pins::plan(&review).unwrap();
        assert!(!pins.refs.is_empty());
        // The user has already removed the checkout; its review history remains.
        call(&mut app, "workspace.delete", json!({"workspace_id":w.id}));
        let home = app.project(&project).unwrap().home_path.clone();
        app.config_path = Some(root.path().join("config.json"));
        app.persist();
        let saved_home = root.path().join("offline-source");
        std::fs::rename(&home, &saved_home).unwrap();
        let repositories: std::collections::BTreeSet<_> =
            pins.refs.iter().map(|p| p.repository.clone()).collect();
        let mut moved = vec![];
        for (index, repository) in repositories.iter().enumerate() {
            if !repository.starts_with(&home) {
                let saved = root.path().join(format!("offline-receiver-{index}"));
                std::fs::rename(repository, &saved).unwrap();
                if placeholder {
                    std::fs::create_dir(repository).unwrap();
                }
                moved.push((repository.clone(), saved));
            }
        }
        assert!(!home.exists());
        let reply = app.handle(req(
            "project.delete",
            json!({"project_id":project,"confirm":true,"tasks":"delete"}),
        ));
        assert_eq!(
            reply["ok"], true,
            "missing source must not block deletion: {reply}"
        );
        assert!(app.project(&project).is_none());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(&task)
            .unwrap()
            .is_none());
        let record = app.project_deletion_records().unwrap().pop().unwrap();
        assert_eq!(record.tombstone.as_deref(), Some("deleted"));
        assert!(
            !record.pending_pins.is_empty(),
            "unavailable pins were discarded after successful deletion"
        );
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
        drop(app);
        std::fs::rename(saved_home, home).unwrap();
        for (repository, saved) in moved {
            if placeholder {
                std::fs::remove_dir(&repository).unwrap();
            }
            std::fs::rename(saved, repository).unwrap();
        }
        let app = restart(root.path());
        assert!(app.project(&project).is_none());
        assert!(app.project_deletion_records().unwrap()[0]
            .pending_pins
            .is_empty());
        for pin in pins.refs {
            let repository = git2::Repository::open_bare(&pin.repository).unwrap();
            assert_eq!(
                repository.find_reference(&pin.name).err().unwrap().code(),
                git2::ErrorCode::NotFound
            );
        }
    }
}
