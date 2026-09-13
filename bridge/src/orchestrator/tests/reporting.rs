use super::plans::{approved_multi_stage_plan, dispatch_planned_run, multi_stage_plan_in_review};
use super::runs::{
    classified, dispatch_single_stage_run, dispatch_turn_halves, last_commit_subject, orchestrator,
    split_store,
};
use super::workspace::worktree_head;
use crate::git_fixture::init_repo;
use crate::mcp::{DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::orchestrator::{triage_is_due, OrchestratorError, ReportOutcome};
use crate::plan::PlanState;
use crate::run::{
    RunState, StageProgressState, TriageHunk, TriageLevel, TriageReport, ValidationReport,
};

pub(super) fn done(phase: DonePhase, status: DoneStatus, plan_path: Option<&str>) -> DoneReport {
    DoneReport {
        phase,
        status,
        summary: "summary".into(),
        outputs: DoneOutputs {
            plan_path: plan_path.map(String::from),
            ..DoneOutputs::default()
        },
    }
}
pub(super) fn done_validate(passed: bool, findings: &str, notes: &str) -> DoneReport {
    DoneReport {
        phase: DonePhase::Validate,
        status: DoneStatus::Completed,
        summary: "validated".into(),
        outputs: DoneOutputs {
            validation: Some(ValidationReport {
                passed,
                findings: findings.into(),
                notes_for_next_stage: notes.into(),
            }),
            ..DoneOutputs::default()
        },
    }
}
#[tokio::test]
async fn stray_revise_report_with_no_revision_in_flight_is_rejected() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 1);

    let err = orch
        .on_plan_done(
            &mut plan,
            &store,
            done(DonePhase::Revise, DoneStatus::Completed, None),
        )
        .expect_err("no stage revision is in flight");
    assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
    assert_eq!(plan.plan.state, PlanState::PlanReview);
}
pub(super) fn done_build_saying(summary: &str) -> DoneReport {
    DoneReport {
        phase: DonePhase::Build,
        status: DoneStatus::Completed,
        summary: summary.into(),
        outputs: DoneOutputs::default(),
    }
}
pub(super) fn done_triage(based_on: &str, hunks: Vec<TriageHunk>) -> DoneReport {
    DoneReport {
        phase: DonePhase::Triage,
        status: DoneStatus::Completed,
        summary: "the crypto change carries the risk".into(),
        outputs: DoneOutputs {
            triage: Some(TriageReport {
                based_on: based_on.into(),
                hunks,
                overrides: Vec::new(),
            }),
            ..DoneOutputs::default()
        },
    }
}
/// A build that produced a diff is followed by a pass that orders it, and
/// the pass is told the hunks by name plus what the builder said about them.
#[tokio::test]
async fn a_completed_build_with_a_diff_asks_for_a_triage_pass() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");

    std::fs::write(run.worktree.path.join("crypto.rs"), "fn derive() {}\n").unwrap();
    let account = "Reworked crypto.rs — key derivation.\n\nRisks: untested on rotation.";
    let build_report = done_build_saying(account);
    let consumed = orch
        .on_run_done(&mut run, &[], build_report.clone())
        .unwrap();
    assert!(consumed.next.is_none(), "nothing else is being said");
    assert!(
        triage_is_due(&build_report, consumed.next.is_some()),
        "the diff wants ordering for review"
    );

    let patch = orch.run_diff(&run).unwrap().patch().to_string();
    let turn = orch
        .triage_turn(&run, &patch, "revision-sha-1", account)
        .expect("a diff with hunks gets a triage turn");
    assert_eq!(turn.phase, "triage");
    let prompt = dispatch_turn_halves(&turn, "triage");
    for hunk_id in crate::diff::hunk_ids(&patch) {
        assert!(prompt.contains(&hunk_id), "{hunk_id} missing from {prompt}");
    }
    assert!(prompt.contains("crypto.rs — key derivation"), "{prompt}");
    assert!(prompt.contains("untested on rotation"), "{prompt}");
    assert!(prompt.contains("revision-sha-1"), "{prompt}");
    assert!(
        prompt.contains(run.base_sha.as_deref().unwrap()),
        "the pass is told what the diff is taken against: {prompt}"
    );
}
/// Nothing changed, nothing to order: the turn is not spent.
#[tokio::test]
async fn an_empty_diff_gets_no_triage_turn() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
    assert!(orch.triage_turn(&run, "", "revision-sha-1", "").is_none());
}
/// The agent hears one thing at a time: a stage that has just been asked to
/// validate itself is not also asked to triage. The verdict is when the
/// stage's diff finally holds still, so that is when triage is asked for —
/// and only when the verdict passed, since a failed one is about to change.
#[tokio::test]
async fn a_stage_is_triaged_after_its_verdict_not_beside_its_validation() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    let built = orch
        .on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
    assert!(built.next.is_some(), "the stage hands itself to validation");
    assert!(
        !triage_is_due(
            &done(DonePhase::Build, DoneStatus::Completed, None),
            built.next.is_some()
        ),
        "triage waits for the turn after the validation hand-off"
    );

    let failed = orch
        .on_run_done(&mut run, &plan.stages, done_validate(false, "- nope", ""))
        .unwrap();
    assert!(
        !triage_is_due(&done_validate(false, "- nope", ""), failed.next.is_some()),
        "a stage sent back for fixes has a diff about to change"
    );

    // Fix it, validate again, and the passing verdict asks for the pass.
    orch.fix_run_stage(&mut run, &plan.stages, "first", "")
        .unwrap();
    std::fs::write(run.worktree.path.join("first.txt"), "one\ntwo\n").unwrap();
    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    let passed = orch
        .on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
        .unwrap();
    assert!(
        triage_is_due(&done_validate(true, "- ok", ""), passed.next.is_some()),
        "the stage's diff now holds still"
    );
}
/// Triage is presentational: the report lands on the run and moves nothing —
/// not the state, not the card's summary.
#[tokio::test]
async fn a_triage_report_is_kept_on_the_run_and_gates_nothing() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");

    std::fs::write(run.worktree.path.join("crypto.rs"), "fn derive() {}\n").unwrap();
    orch.on_run_done(
        &mut run,
        &[],
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(run.run.state, RunState::Review);
    let summary_before_triage = run.last_summary.clone();

    let ids = crate::diff::hunk_ids(orch.run_diff(&run).unwrap().patch());
    let consumed = orch
        .on_run_done(
            &mut run,
            &[],
            done_triage(
                "revision-sha-1",
                vec![classified(&ids[0], TriageLevel::Critical)],
            ),
        )
        .unwrap();
    assert_eq!(consumed.outcome, ReportOutcome::Applied);
    assert!(
        !triage_is_due(&done_triage("revision-sha-1", Vec::new()), false),
        "a triage does not triage itself"
    );
    assert_eq!(
        run.run.state,
        RunState::Review,
        "triage moves no lifecycle state"
    );
    assert_eq!(
        run.last_summary, summary_before_triage,
        "the card still says what the build said"
    );
    let triage = run.triage.as_ref().expect("the pass is kept on the run");
    assert_eq!(triage.based_on, "revision-sha-1");
    assert_eq!(triage.hunks[0].level, TriageLevel::Critical);
}
/// The id vocabulary is the patch's. An invented id fails the whole report
/// — a half-landed triage would order the review by a rule nobody stated —
/// and the refusal names the ids that were available.
#[tokio::test]
async fn a_triage_naming_a_hunk_that_is_not_in_the_diff_is_refused() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");

    std::fs::write(run.worktree.path.join("crypto.rs"), "fn derive() {}\n").unwrap();
    orch.on_run_done(
        &mut run,
        &[],
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    let ids = crate::diff::hunk_ids(orch.run_diff(&run).unwrap().patch());

    let error = orch
        .on_run_done(
            &mut run,
            &[],
            done_triage(
                "revision-sha-1",
                vec![
                    classified(&ids[0], TriageLevel::Low),
                    classified("hnotinthisdiff", TriageLevel::Critical),
                ],
            ),
        )
        .expect_err("an invented hunk id is refused");
    let message = error.to_string();
    assert!(message.contains("hnotinthisdiff"), "{message}");
    assert!(message.contains(&ids[0]), "{message}");
    assert!(
        run.triage.is_none(),
        "a refused report leaves no partial ordering behind"
    );
}
/// A triage report that lost its payload on the way (a raw daemon-socket
/// writer, a version-skewed mcp binary) is rejected, never unwrapped.
#[tokio::test]
async fn a_triage_report_with_no_triage_payload_is_rejected() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
    std::fs::write(run.worktree.path.join("crypto.rs"), "fn derive() {}\n").unwrap();

    let error = orch
        .on_run_done(
            &mut run,
            &[],
            done(DonePhase::Triage, DoneStatus::Completed, None),
        )
        .expect_err("a triage report without outputs.triage is rejected");
    assert!(error.to_string().contains("outputs.triage"), "{error}");
    assert!(run.triage.is_none());
}
#[tokio::test]
async fn run_stage_build_done_commits_and_hands_off_to_validation() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    let consumed = orch
        .on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
    assert_eq!(run.run.state, RunState::Building, "validation is running");
    assert_eq!(run.stages[0].state, StageProgressState::Validating);
    let candidate = worktree_head(&run.worktree.path);
    assert_eq!(run.stages[0].built_sha.as_deref(), Some(candidate.as_str()));
    assert_eq!(run.stages[0].completion_sha, None);
    let subject = last_commit_subject(&run.worktree.path);
    assert!(
        subject.contains("stage first"),
        "stage work committed before validation: {subject:?}"
    );
    // The hand-off is a turn for the SAME agent, not a new process.
    let hand_off = consumed
        .next
        .expect("a built stage hands itself to validation");
    let prompt = dispatch_turn_halves(&hand_off, "validate");
    let start_sha = run.stages[0].start_sha.clone().unwrap();
    assert!(prompt.contains("VALIDATION"), "{prompt}");
    assert!(prompt.contains(&start_sha), "{prompt}");
    assert!(
        prompt.contains(".build/plan/02-second.md"),
        "next stage doc is in the validation prompt: {prompt}"
    );
}
#[tokio::test]
async fn stray_run_reports_are_ignored_or_rejected_not_promoted() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    // A single-stage run must ignore a validate report outright.
    let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
    orch.on_run_done(&mut single, &[], done_validate(true, "- ok", ""))
        .unwrap();
    assert_eq!(single.run.state, RunState::Building, "ignored");

    // A run session misusing phase=plan is rejected: plan reports belong
    // to plans, and consuming one here would smuggle manifest edits.
    let err = orch
        .on_run_done(
            &mut single,
            &[],
            done(DonePhase::Plan, DoneStatus::Completed, None),
        )
        .expect_err("plan reports belong to plans");
    assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
    assert_eq!(single.run.state, RunState::Building);

    // A multi-stage run mid-validation must ignore a stray build report —
    // otherwise a rogue done(build) would skip the validation gate.
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(run.stages[0].state, StageProgressState::Validating);
    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(run.run.state, RunState::Building);
    assert_eq!(run.stages[0].state, StageProgressState::Validating);
}
/// Blocking asked for help; it never closed the session. A completion
/// arriving after the run was blocked is honored exactly like the
/// idle-unreported precedent: the stage checkpoints, hands to validation,
/// and the verdict still lands — a blocked run never vetoes the agent's
/// own progress.
#[tokio::test]
async fn run_blocked_then_late_done_is_still_honored() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    run.auto_advance = true;

    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Blocked, None),
    )
    .unwrap();
    assert_eq!(run.run.state, RunState::Blocked);
    assert_eq!(run.stages[0].state, StageProgressState::Building);
    assert!(!run.auto_advance, "blocked disarms run-all");

    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    let outcome = orch
        .on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .expect("a late completion is honored");
    assert!(
        matches!(outcome.outcome, ReportOutcome::Applied),
        "{outcome:?}"
    );
    assert_eq!(run.stages[0].state, StageProgressState::Validating);
    assert!(
        outcome.next.is_some(),
        "the stage hand-off dispatches validation"
    );

    // The verdict is honored from Blocked too — mid-plan pass parks the
    // run at the stage gate as usual.
    orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
        .unwrap();
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
    orch.on_run_done(
        &mut run,
        &[],
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(run.run.state, RunState::Review);
}
#[tokio::test]
async fn validate_done_without_a_validation_report_is_rejected_not_a_panic() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(
        run.stage_progress("first").unwrap().state,
        StageProgressState::Validating
    );

    // The daemon socket deserializes reports as raw JSON — a
    // validate/completed with no outputs.validation must be rejected
    // with zero mutation, never unwrapped.
    let err = orch
        .on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Validate, DoneStatus::Completed, None),
        )
        .expect_err("a report without outputs.validation is rejected");
    assert!(err.to_string().contains("no outputs.validation"), "{err}");
    assert_eq!(
        run.stage_progress("first").unwrap().state,
        StageProgressState::Validating,
        "the stage still awaits a real verdict"
    );
    assert_eq!(run.run.state, RunState::Building);
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
    orch.on_run_done(
        &mut single,
        &[],
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    assert_eq!(single.run.state, RunState::Review);
    single.last_summary = Some("the report that opened review".into());

    let outcome = orch
        .on_run_done(
            &mut single,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .expect("an out-of-phase report is not an error");
    assert!(
        matches!(outcome.outcome, ReportOutcome::OutOfPhase(_)),
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
    orch.on_run_done(
        &mut run,
        &plan.stages,
        done(DonePhase::Build, DoneStatus::Completed, None),
    )
    .unwrap();
    orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
        .unwrap();
    assert_eq!(run.run.state, RunState::StageGate);
    let stage_state = run.stage_progress("first").unwrap().state;

    let outcome = orch
        .on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .expect("an out-of-phase stage report is not an error");
    assert!(
        matches!(outcome.outcome, ReportOutcome::OutOfPhase(_)),
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
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
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
