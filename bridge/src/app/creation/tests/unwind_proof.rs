use super::review_followup::git_fixture;
use super::settlement::{git, workspace_attempt};
use super::*;
use crate::workspace::Workspace;
use crate::worktree::workspace_checkout_test::{self, Boundary as CutBoundary};
use crate::worktree::WorktreeError;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    Checkout,
    Delete,
    Lookup,
}
fn inject_checkout_failure(
    boundary: CutBoundary,
    path: &std::path::Path,
    fault: Fault,
    captured: &AtomicBool,
) {
    if fault != Fault::Checkout {
        return;
    }
    match boundary {
        CutBoundary::BeforeRemove => {
            if path.is_dir() {
                std::fs::remove_dir_all(path).unwrap();
            }
            std::fs::write(path, "cannot remove a file as a checkout directory").unwrap();
        }
        CutBoundary::AfterRemove(removed) => {
            assert!(!removed, "the backend removal must really fail");
            captured.store(true, Ordering::SeqCst);
            std::fs::remove_file(path).unwrap();
        }
        _ => {}
    }
}
fn inject_ref_failure(
    boundary: CutBoundary,
    source: &std::path::Path,
    branch: &str,
    fault: Fault,
) -> Result<(), WorktreeError> {
    if boundary != CutBoundary::BeforeRefRemoval {
        return Ok(());
    }
    if fault == Fault::Lookup {
        return Err(WorktreeError::Git(git2::Error::new(
            git2::ErrorCode::Locked,
            git2::ErrorClass::Reference,
            "injected lookup failure",
        )));
    }
    let repository = git2::Repository::open(source).unwrap();
    let lock = repository
        .commondir()
        .join(format!("refs/heads/{branch}.lock"));
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(lock, "held by another writer").unwrap();
    Ok(())
}

fn fail_unwind(at: CutBoundary, fault: Fault) {
    let (f, repo, project) = git_fixture(false);
    let id = operation(200);
    let record = workspace_attempt(&f, &id, &project);
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    let removal_failed = Arc::new(AtomicBool::new(false));
    let captured = removal_failed.clone();
    let source = repo.clone();
    let observer = workspace_checkout_test::observe(move |boundary, path, branch| {
        if boundary == at {
            return Err(WorktreeError::Command("injected materializer error".into()));
        }
        inject_checkout_failure(boundary, path, fault, &captured);
        inject_ref_failure(boundary, &source, branch, fault)
    });
    f.run(&id);
    drop(observer);
    if fault == Fault::Checkout {
        assert!(removal_failed.load(Ordering::SeqCst));
    }
    let record = f.record(&id);
    assert!(!workspace.directories[0].path.exists());
    assert!(
        !git(&repo, &["branch", "--list", "build/*"])
            .trim()
            .is_empty(),
        "unwind must leave a real branch behind"
    );
    assert!(
        matches!(record.state.as_str(), "unknown" | "reconciling"),
        "{record:?}"
    );
    assert_eq!(record.execution["effect_state"], "started");
    assert!(record.outcome.is_none());
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
    let f = f.reopen();
    f.run(&id);
    assert!(f.record(&id).outcome.is_none());
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
}

#[test]
fn materializer_branch_delete_failure_keeps_fence_with_no_checkout() {
    fail_unwind(CutBoundary::Materialize, Fault::Delete);
}
#[test]
fn teardown_branch_delete_failure_keeps_fence_with_no_checkout() {
    fail_unwind(CutBoundary::Teardown, Fault::Delete);
}
#[test]
fn materializer_checkout_removal_failure_keeps_fence() {
    fail_unwind(CutBoundary::Materialize, Fault::Checkout);
}
#[test]
fn teardown_checkout_removal_failure_keeps_fence() {
    fail_unwind(CutBoundary::Teardown, Fault::Checkout);
}
#[test]
fn materializer_ref_lookup_error_keeps_fence() {
    fail_unwind(CutBoundary::Materialize, Fault::Lookup);
}
#[test]
fn teardown_ref_lookup_error_keeps_fence() {
    fail_unwind(CutBoundary::Teardown, Fault::Lookup);
}

#[test]
fn complete_git_unwind_settles_initial_and_not_started_replay() {
    for replay in [false, true] {
        for at in [CutBoundary::Materialize, CutBoundary::Teardown] {
            let (f, repo, project) = git_fixture(false);
            let id = operation(201);
            let record = workspace_attempt(&f, &id, &project);
            let workspace: Workspace =
                serde_json::from_value(record.execution["workspace"].clone()).unwrap();
            let f = if replay {
                f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
                let f = f.reopen();
                f.store.writer().recover_mutations().unwrap();
                assert_eq!(f.record(&id).state, "unknown");
                assert_eq!(f.record(&id).execution["effect_state"], "not_started");
                f
            } else {
                f
            };
            let observer = workspace_checkout_test::observe(move |boundary, _, _| {
                if boundary == at {
                    return Err(WorktreeError::Command("injected materializer error".into()));
                }
                Ok(())
            });
            f.run(&id);
            drop(observer);
            let record = f.record(&id);
            assert_eq!(record.state, "failed", "{record:?}");
            assert_eq!(record.execution["effect_state"], "settled");
            assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
            assert!(!workspace.root.exists());
            assert_eq!(
                git(&repo, &["branch", "--format=%(refname:short)"]).trim(),
                "main"
            );
        }
    }
}

#[test]
fn started_checkpoint_precedes_workspace_effects_initial_and_replay() {
    for replay in [false, true] {
        let (f, repo, project) = git_fixture(false);
        let id = operation(202);
        let record = workspace_attempt(&f, &id, &project);
        let workspace: Workspace =
            serde_json::from_value(record.execution["workspace"].clone()).unwrap();
        let f = if replay {
            f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
            let f = f.reopen();
            f.store.writer().recover_mutations().unwrap();
            assert_eq!(f.record(&id).state, "unknown");
            f
        } else {
            f
        };
        let store = f.store.clone();
        let reached = Arc::new(AtomicBool::new(false));
        let saved = reached.clone();
        let operation_id = id.clone();
        let observer = observe_boundary(move |boundary| {
            if boundary == Boundary::WorkspaceStarted {
                let durable = store
                    .writer()
                    .creation_record(PRINCIPAL, &operation_id)
                    .unwrap()
                    .unwrap();
                assert_eq!(durable.execution["effect_state"], "started");
                assert_eq!(durable.execution["effect_step"], "workspace");
                assert!(
                    !workspace.root.exists(),
                    "root creation must follow the durable start"
                );
                assert_eq!(
                    git(&repo, &["branch", "--format=%(refname:short)"]).trim(),
                    "main"
                );
                saved.store(true, Ordering::SeqCst);
                std::panic::panic_any(SimulatedCrash);
            }
        });
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
        assert!(stopped.unwrap_err().is::<SimulatedCrash>());
        assert!(reached.load(Ordering::SeqCst));
        drop(observer);
    }
}
