use super::*;
use crate::orchestrator::PlanWorkspace;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct CountImplementationRefusal(Arc<AtomicUsize>);
impl ImplementationCaller for CountImplementationRefusal {
    fn opened(self: Box<Self>, _state: &mut AppState, _run_id: &str) -> Result<Value, String> {
        unreachable!()
    }

    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> String {
        self.0.fetch_add(1, Ordering::SeqCst);
        error
    }
}

struct CountPlanRefusal(Arc<AtomicUsize>);
impl PlanSessionOpening for CountPlanRefusal {
    fn open(
        self: Box<Self>,
        _state: &mut AppState,
        _workspace: PlanWorkspace,
    ) -> Result<Value, String> {
        unreachable!()
    }

    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> Result<Value, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"refused": error}))
    }
}

#[test]
fn public_implementation_refusal_forwards_exactly_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Box::new(crate::app::ImplementationRefused {
        error: "no checkout".into(),
        caller: Box::new(CountImplementationRefusal(calls.clone())),
    })
    .apply(&mut state);
    assert_eq!(result.unwrap_err(), "no checkout");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn public_planning_refusal_forwards_exactly_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Box::new(crate::app::PlanWorkspaceRefused {
        error: "no workspace".into(),
        opening: Box::new(CountPlanRefusal(calls.clone())),
    })
    .apply(&mut state)
    .unwrap();
    assert_eq!(result, json!({"refused": "no workspace"}));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// Compile coverage for every public success carrier's complete field surface.
// Runtime behavior is exercised through the two counting refusal tests above;
// these constructors catch compatibility drift without fabricating git state.
#[allow(dead_code, clippy::too_many_arguments)]
fn public_success_carriers_typecheck(
    prepared: crate::orchestrator::PreparedImplementation,
    checkout: crate::orchestrator::AdoptableCheckout,
    workspace: crate::orchestrator::PlanWorkspace,
    worktree: crate::worktree::Worktree,
    checkouts: crate::lifecycle::holders::ProjectCheckouts,
    caller_a: Box<dyn crate::app::ImplementationCaller>,
    caller_b: Box<dyn crate::app::ImplementationCaller>,
    opening: Box<dyn crate::app::PlanSessionOpening>,
    routed_a: Option<crate::app::RoutedCapture>,
    routed_b: Option<crate::app::RoutedCapture>,
    model_choice: crate::models::ModelChoice,
) {
    use crate::app::*;
    use crate::orchestrator::AdoptionScope;
    use std::path::PathBuf;

    let adopted = RunAdopted {
        project_id: "project".into(),
        run_id: "run".into(),
        base_branch: "main".into(),
        checkout: checkout.clone(),
        scope: AdoptionScope::ExternalWorktree,
        model_choice: model_choice.clone(),
    };
    let _ = Box::new(ImplementationOpened {
        project_id: "project".into(),
        issue_id: "issue".into(),
        run_id: "run".into(),
        prepared,
        model_choice: model_choice.clone(),
        caller: caller_a,
        downgrade: None,
    });
    let _ = Box::new(ImplementationAdopted {
        project_id: "project".into(),
        issue_id: "issue".into(),
        run_id: "run".into(),
        base_sha: "base".into(),
        adopted: Some(adopted),
        model_choice: model_choice.clone(),
        caller: caller_b,
    });
    let _ = Box::new(RunAdoptionSettled {
        adopted: RunAdopted {
            project_id: "project".into(),
            run_id: "run".into(),
            base_branch: "main".into(),
            checkout: checkout.clone(),
            scope: AdoptionScope::ExternalWorktree,
            model_choice: model_choice.clone(),
        },
        detail: crate::thread::ThreadDetail::Digest,
    });
    let _ = Box::new(RestoredCheckout {
        issue_id: "issue".into(),
        run_id: "run".into(),
        checkout_stood: false,
        restored: Ok(worktree),
        caller: Box::new(CountImplementationRefusal(Arc::new(AtomicUsize::new(0)))),
        downgrade: None,
    });
    let _ = Box::new(BranchDispatched {
        adopted: RunAdopted {
            project_id: "project".into(),
            run_id: "run".into(),
            base_branch: "main".into(),
            checkout,
            scope: AdoptionScope::ExternalWorktree,
            model_choice: model_choice.clone(),
        },
        instruction: "work".into(),
        routed: routed_a,
        checkouts: checkouts.clone(),
        downgrade: None,
    });
    let _ = Box::new(BranchJoined {
        project_id: "project".into(),
        run_id: "run".into(),
        branch: "build/work".into(),
        root: PathBuf::new(),
        instruction: "work".into(),
        model_choice,
        explicit_choice: true,
        routed: routed_b,
        checkouts: checkouts.clone(),
    });
    let _ = Box::new(WorktreeCreated {
        project_id: "project".into(),
        placeholder_id: "pending".into(),
        worktree_id: "worktree".into(),
        branch: "build/work".into(),
        name: "work".into(),
        path: PathBuf::new(),
        branch_was_cut: true,
        checkouts,
        isolation: None,
        downgrade: None,
    });
    let _ = Box::new(PlanWorkspaceOpened { workspace, opening });
    let _ = Box::new(ProjectAdded {
        path: PathBuf::new(),
        base: "main".into(),
        remote: None,
        created_checkout: None,
    });
    let _ = Box::new(ProjectRemoteSet {
        project_id: "project".into(),
        remote: None,
    });

    let _: fn(Box<WorktreeCreated>, &mut AppState) -> Result<Value, String> =
        WorktreeCreated::apply;
    let _: fn(Box<ImplementationOpened>, &mut AppState) -> Result<Value, String> =
        ImplementationOpened::apply;
    let _: fn(Box<ImplementationAdopted>, &mut AppState) -> Result<Value, String> =
        ImplementationAdopted::apply;
    let _: fn(Box<RunAdoptionSettled>, &mut AppState) -> Result<Value, String> =
        RunAdoptionSettled::apply;
    let _: fn(Box<RestoredCheckout>, &mut AppState) -> Result<Value, String> =
        RestoredCheckout::apply;
    let _: fn(Box<BranchDispatched>, &mut AppState) -> Result<Value, String> =
        BranchDispatched::apply;
    let _: fn(Box<BranchJoined>, &mut AppState) -> Result<Value, String> = BranchJoined::apply;
    let _: fn(Box<PlanWorkspaceOpened>, &mut AppState) -> Result<Value, String> =
        PlanWorkspaceOpened::apply;
    let _: fn(Box<ProjectAdded>, &mut AppState) -> Result<Value, String> = ProjectAdded::apply;
    let _: fn(Box<ProjectRemoteSet>, &mut AppState) -> Result<Value, String> =
        ProjectRemoteSet::apply;
    let _: fn(&RunAdopted, &mut AppState) -> Result<crate::orchestrator::ActiveRun, String> =
        RunAdopted::open_run;
}
