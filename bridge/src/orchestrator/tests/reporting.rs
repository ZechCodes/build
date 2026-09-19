use super::plans::{approved_multi_stage_plan, dispatch_planned_run, multi_stage_plan_in_review};
use super::runs::{dispatch_single_stage_run, last_commit_subject, orchestrator, split_store};
use super::workspace::worktree_head;
use crate::git_fixture::init_repo;
use crate::mcp::{DoneReport, DoneStatus};
use crate::orchestrator::{OrchestratorError, ReportOutcome};
use crate::plan::PlanState;
use crate::run::{RunState, StageProgressState};

pub(super) fn done(status: DoneStatus) -> DoneReport {
    DoneReport::new(status, "summary")
}
/// A Complete report on a plan already in review, with no stage revision in
/// flight, is not a second plan: it is rejected and moves nothing.
#[tokio::test]
async fn a_plan_complete_with_no_revision_in_flight_is_rejected_from_review() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 1);

    let err = orch
        .on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .expect_err("no stage revision is in flight");
    assert!(matches!(err, OrchestratorError::PlanTransition(_)), "{err}");
    assert_eq!(plan.plan.state, PlanState::PlanReview);
}
/// A stage's Complete report is the whole stage: Build commits the
/// checkpoint and the stage completes on its built commit. Mid-plan, the run
/// parks at the stage gate for the reviewer to dispatch the next stage.
#[tokio::test]
async fn a_stage_complete_commits_and_completes_the_stage_at_the_gate() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    let outcome = orch
        .on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(outcome, ReportOutcome::Applied);
    assert_eq!(run.run.state, RunState::StageGate);
    assert_eq!(run.stages[0].state, StageProgressState::Completed);
    let head = worktree_head(&run.worktree.path);
    assert_eq!(run.stages[0].built_sha.as_deref(), Some(head.as_str()));
    assert_eq!(
        run.stages[0].completion_sha.as_deref(),
        Some(head.as_str()),
        "the stage completes on the commit it was built on"
    );
    let subject = last_commit_subject(&run.worktree.path);
    assert!(
        subject.contains("stage first"),
        "stage work committed at the boundary: {subject:?}"
    );
    assert_eq!(run.last_summary.as_deref(), Some("summary"));
}
/// The last stage's Complete opens merge review instead of a stage gate.
#[tokio::test]
async fn the_last_stage_complete_opens_review() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 1);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::Review);
    assert_eq!(run.stages[0].state, StageProgressState::Completed);
    assert_eq!(
        run.stages[0].completion_sha.as_deref(),
        Some(worktree_head(&run.worktree.path).as_str())
    );
}
/// Blocking asked for help; it never closed the session. A completion
/// arriving after the run was blocked is honored exactly like the
/// idle-unreported precedent: the stage checkpoints and completes — a
/// blocked run never vetoes the agent's own progress.
#[tokio::test]
async fn run_blocked_then_late_done_is_still_honored() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    run.auto_advance = true;

    orch.on_run_done(&mut run, &plan.stages, done(DoneStatus::Blocked))
        .unwrap();
    assert_eq!(run.run.state, RunState::Blocked);
    assert_eq!(run.stages[0].state, StageProgressState::Building);
    assert!(!run.auto_advance, "blocked disarms run-all");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    let outcome = orch
        .on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .expect("a late completion is honored");
    assert_eq!(outcome, ReportOutcome::Applied);
    assert_eq!(run.stages[0].state, StageProgressState::Completed);
    assert_eq!(run.run.state, RunState::StageGate);
}
#[tokio::test]
async fn run_idle_then_late_done_is_still_honored() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");

    orch.on_run_idle(&mut run).unwrap();
    assert_eq!(run.run.state, RunState::IdleUnreported);
    orch.on_run_done(&mut run, &[], done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::Review);
}
/// A persistent agent outlives the phase it was dispatched for: talk to it
/// at a review gate and it will report `done` from a state the run machine
/// does not accept. That report is out of phase, not a failure — nothing
/// moves, nothing is rejected, and the caller is told so it can record the
/// report on the conversation instead of a bogus failure event.
#[tokio::test]
async fn an_out_of_phase_done_moves_nothing_and_is_not_an_error() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    // Single-doc run parked at review: BuildReady is not legal from there.
    let mut single = dispatch_single_stage_run(&orch, &store, "run-late", "late report");
    orch.on_run_done(&mut single, &[], done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(single.run.state, RunState::Review);
    single.last_summary = Some("the report that opened review".into());

    let outcome = orch
        .on_run_done(&mut single, &[], done(DoneStatus::Completed))
        .expect("an out-of-phase report is not an error");
    assert!(
        matches!(outcome, ReportOutcome::OutOfPhase(_)),
        "{outcome:?}"
    );
    assert_eq!(single.run.state, RunState::Review, "nothing moved");
    assert_eq!(
        single.last_summary.as_deref(),
        Some("the report that opened review"),
        "an unconsumed report leaves no trace on the run"
    );

    // A multi-stage run at the between-stages gate: same rule, through the
    // stage pipeline (the stage must not advance either).
    let plan = approved_multi_stage_plan(&orch, &store, "plan-late", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-staged");
    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::StageGate);
    let stage_state = run.stage_progress("first").unwrap().state;

    let outcome = orch
        .on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .expect("an out-of-phase stage report is not an error");
    assert!(
        matches!(outcome, ReportOutcome::OutOfPhase(_)),
        "{outcome:?}"
    );
    assert_eq!(run.run.state, RunState::StageGate, "nothing moved");
    assert_eq!(
        run.stage_progress("first").unwrap().state,
        stage_state,
        "the stage machine did not move either"
    );
}
#[tokio::test]
async fn run_finishers_commit_merge_and_report_conflicts() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let to_review = |id: &str, file: &str, contents: &str| {
        let mut run = dispatch_single_stage_run(&orch, &store, id, "same file");
        std::fs::write(run.worktree.path.join(file), contents).unwrap();
        orch.on_run_done(&mut run, &[], done(DoneStatus::Completed))
            .unwrap();
        assert_eq!(run.run.state, RunState::Review);
        run
    };

    // Both runs branch from the same base tip and touch the same file.
    let mut first = to_review("run-1", "result.txt", "first\n");
    let mut second = to_review("run-2", "result.txt", "second\n");

    // Commit keeps the worktree and makes an honest commit.
    orch.run_commit(&first).unwrap();
    assert_eq!(
        last_commit_subject(&first.worktree.path),
        "Build: same file"
    );

    // Approve & merge → Merged, base branch tracks the file, worktree kept
    // until the caller prunes.
    orch.run_approve_merge(&mut first).unwrap();
    assert_eq!(first.run.state, RunState::Merged);
    assert!(repo.join("result.txt").exists());
    assert!(
        first.worktree.path.exists(),
        "merge leaves cleanup to the caller"
    );

    // The second run now conflicts: it reports merge_failed and stays in review.
    let err = orch
        .run_approve_merge(&mut second)
        .expect_err("the second write conflicts");
    assert!(err.to_string().starts_with("merge_failed:"), "{err}");
    assert!(
        err.to_string().contains("CONFLICT"),
        "a failed merge carries git's own words: {err}"
    );
    assert!(
        !err.to_string().contains("git command failed"),
        "the banner carries git's words alone, not the façade's prefix: {err}"
    );
    assert_eq!(second.run.state, RunState::Review);
    assert!(
        !repo.join(".git/MERGE_HEAD").exists(),
        "a failed merge is aborted"
    );
}
