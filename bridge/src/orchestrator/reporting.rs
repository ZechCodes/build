use crate::diff::DiffError;
use crate::git_process::{run_git, GitError};
use crate::harness::HarnessError;
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::plan::{
    IllegalPlanTransition, IllegalStageDocTransition, StageDoc, StageDocState, StageManifestEntry,
};
use crate::run::{
    run_transition, IllegalRunTransition, IllegalStageProgressTransition, RunEvent,
    StageProgressEvent, StageProgressState,
};
use crate::store::StoreError;
use crate::worktree::WorktreeError;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Harness(#[from] HarnessError),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    PlanTransition(#[from] IllegalPlanTransition),
    #[error(transparent)]
    RunTransition(#[from] IllegalRunTransition),
    #[error(transparent)]
    StageDoc(#[from] IllegalStageDocTransition),
    #[error(transparent)]
    StageProgress(#[from] IllegalStageProgressTransition),
    /// A store operation hit during a lifecycle move (the transactional
    /// plan-doc ingest, run-dispatch materialization) failed; the move never
    /// happened.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A rejected stage-gate precondition; the message is surfaced verbatim over RPC.
    #[error("{0}")]
    Gate(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("git command failed: {0}")]
    Git(String),
    /// A merge approval whose git work failed (conflict, wrong base checkout,
    /// nothing to commit). The `merge_failed:` prefix is the cross-stream contract
    /// the web client keys on to show the reason in the task banner; the state
    /// stays in review because the merge never happened.
    #[error("merge_failed: {0}")]
    MergeFailed(String),
}

impl From<GitError> for OrchestratorError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Unstartable(io) => OrchestratorError::Io(io),
            GitError::Failed(detail) => OrchestratorError::Git(detail),
        }
    }
}

/// Convert any git failure hit during a merge approval into [`OrchestratorError::MergeFailed`]
/// so the RPC message carries the contract's `merge_failed:` prefix.
pub(super) fn as_merge_failure(error: OrchestratorError) -> OrchestratorError {
    match error {
        OrchestratorError::Git(reason) => OrchestratorError::MergeFailed(reason),
        OrchestratorError::Worktree(WorktreeError::Command(reason)) => {
            OrchestratorError::MergeFailed(reason)
        }
        already @ OrchestratorError::MergeFailed(_) => already,
        other => OrchestratorError::MergeFailed(other.to_string()),
    }
}

/// Merge a fresh manifest echo into the plan's stage docs by id: an id that
/// already exists keeps its review sub-state (`Planned`/`Approved`) and takes
/// the new `title`/`path`/`summary`; new ids append as `Planned`; ids missing
/// from the echo are dropped. Run-side execution progress lives on the run and
/// is never deleted by a re-plan, so the plan side only carries doc review and
/// can drop freely. On the first plan the merge is trivially "all new".
pub(super) fn merge_stage_docs(stages: &mut Vec<StageDoc>, entries: &[StageManifestEntry]) {
    let mut leftover = std::mem::take(stages);
    let mut merged: Vec<StageDoc> = Vec::with_capacity(entries.len());
    for entry in entries {
        match leftover.iter().position(|doc| doc.id == entry.id) {
            Some(position) => {
                let mut existing = leftover.remove(position);
                existing.title = entry.title.clone();
                existing.path = entry.path.clone();
                existing.summary = entry.summary.clone();
                merged.push(existing);
            }
            None => merged.push(StageDoc {
                id: entry.id.clone(),
                title: entry.title.clone(),
                path: entry.path.clone(),
                summary: entry.summary.clone(),
                state: StageDocState::Planned,
            }),
        }
    }
    *stages = merged;
}

/// Whether a `done` report should be followed by a triage pass.
///
/// Triage reads a finished diff, so it follows a completed code-changing phase
/// — and a stage's validation verdict, which is where a multi-stage run's diff
/// finally holds still. `already_speaking` is the hand-off turn the report
/// already produced (a stage handing itself to validation): the agent hears one
/// thing at a time, so triage waits for the turn after it. A passed verdict
/// only: a stage sent back for fixes has a diff about to change.
///
/// Deliberately blind to whether the run's lifecycle accepted the report. An
/// agent reporting done at a review gate moves no state — and still leaves a
/// changed diff the reviewer has to read. Triage gates nothing, so what needs
/// ordering is decided by the diff, not by the state machine.
pub fn triage_is_due(report: &DoneReport, already_speaking: bool) -> bool {
    if already_speaking || report.status != DoneStatus::Completed {
        return false;
    }
    match report.phase {
        DonePhase::Build | DonePhase::Revise => true,
        DonePhase::Validate => report
            .outputs
            .validation
            .as_ref()
            .is_some_and(|validation| validation.passed),
        _ => false,
    }
}

/// The builder's account as the triage prompt reads it: its `done` summary,
/// which is the whole report. An empty one says so, because "(nothing)" is
/// information and a blank is not.
fn agent_report_for_triage(summary: &str) -> &str {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        "(the agent reported nothing)"
    } else {
        trimmed
    }
}

/// What a lifecycle move wants said to the worktree's agent.
///
/// The orchestrator owns lifecycle, thread, stages, base_sha, and worktree; it
/// does not own the process the words travel to. A transition that used to end
/// a session and spawn a replacement now applies the transition and returns the
/// turn; the caller, which owns the worktree's agent tab, delivers it — picking
/// `cold` or `warm` from whether it had to spawn a harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurn {
    /// The full rendered run prompt, for an agent with no context: it has
    /// nothing to read the reviewer's messages into.
    pub cold: String,
    /// The bare instruction, for an agent already in the conversation. The
    /// reviewer's words are already durable in the thread and the agent pulls
    /// them with `read_unread_messages`, so the run context would be a repeat.
    pub warm: String,
    /// The phase label recorded on the thread's session lineage when a COLD
    /// delivery starts a new agent process.
    pub phase: &'static str,
}

impl AgentTurn {
    /// A turn whose whole content is the rendered prompt — a dispatch, a
    /// resume, a validation hand-off, a stage fix. The instruction travels
    /// either way; only the conversation protocol and the catch-up packet are
    /// cold-only, because a warm agent has already lived them.
    pub(super) fn dispatched(rendered: String, phase: &'static str) -> AgentTurn {
        AgentTurn {
            cold: conversation_prompt(&rendered),
            warm: rendered,
            phase,
        }
    }

    /// A turn whose content is already durable on the thread — a change
    /// request, a message, a batch of notes. A warm agent is told to read it
    /// (`nudge`); a cold one gets the same instruction wrapped in the run/plan
    /// context it has no way to reconstruct.
    pub(super) fn posted(rendered: String, nudge: &str, phase: &'static str) -> AgentTurn {
        AgentTurn {
            cold: conversation_prompt(&rendered),
            warm: nudge.to_string(),
            phase,
        }
    }
}

/// What a `done` report did to the run, and what Build says next because of it.
#[derive(Debug)]
pub struct ReportConsumed {
    pub outcome: ReportOutcome,
    /// The build→validate hand-off's turn: the same worktree agent that just
    /// reported its stage built is asked to validate it. `None` for every other
    /// report — no other `done` starts a phase.
    pub next: Option<AgentTurn>,
}

impl ReportConsumed {
    /// A report that moved the run (or was absorbed by the stage pipeline) with
    /// nothing more to say.
    fn applied() -> ReportConsumed {
        ReportConsumed {
            outcome: ReportOutcome::Applied,
            next: None,
        }
    }

    /// A report the run's state does not accept: nothing moved, nothing is said.
    fn out_of_phase(illegal: IllegalRunTransition) -> ReportConsumed {
        ReportConsumed {
            outcome: ReportOutcome::OutOfPhase(illegal),
            next: None,
        }
    }
}

/// What a `done` report did to the run.
///
/// A persistent agent outlives the phase it was dispatched for: talk to it at a
/// review gate and it will report `done` from a state the run machine does not
/// accept. Build's rule is enforcement by observation, not permission — such a
/// report moves nothing and is handed back to the caller to record on the
/// conversation, rather than rejected as a failure the human never caused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportOutcome {
    /// The report was consumed: it moved the run, or the stage pipeline took it.
    Applied,
    /// The report arrived from a state that does not accept it. Nothing moved —
    /// not the run, not the stage, not `last_summary`.
    OutOfPhase(IllegalRunTransition),
}

use super::workspace::conversation_prompt;
use super::{ActiveRun, Orchestrator};

impl Orchestrator {
    /// Consume a build-side agent's `done` report for a run. `plan_stage_docs`
    /// is the owning plan's stage-doc manifest (the caller joins by `plan_id`;
    /// empty for adopted runs and single-doc plans), which routes multi-stage
    /// reports through the stage pipeline and supplies the validation prompt's
    /// stage metadata.
    ///
    /// PERIPHERY: a mid-run stage-revision `done` (`revising_stage_id` set)
    /// must ingest the revised docs back into the store and reset the
    /// plan-side doc state — that write-back lands with the stage flows.
    ///
    /// A report the run's current state does not accept is
    /// [`ReportOutcome::OutOfPhase`], not an error: Build's agent is persistent,
    /// so it reports whenever it finishes a turn — including turns the human
    /// started at a review gate. Such a report moves nothing and leaves no
    /// trace on the run; the caller records it on the conversation.
    pub fn on_run_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        report: DoneReport,
    ) -> Result<ReportConsumed, OrchestratorError> {
        let mut next = None;
        match (report.phase, report.status) {
            // A blocked/failed report from any session — stage build, fix,
            // validation, or single-plan — parks the run and disarms run-all.
            (_, DoneStatus::Blocked) => {
                if let Err(illegal) = active.run.apply(RunEvent::Blocked) {
                    return Ok(ReportConsumed::out_of_phase(illegal));
                }
                active.auto_advance = false;
            }
            (_, DoneStatus::Failed) => {
                if let Err(illegal) = active.run.apply(RunEvent::Failed) {
                    return Ok(ReportConsumed::out_of_phase(illegal));
                }
                active.auto_advance = false;
            }
            (DonePhase::Recover, DoneStatus::Completed) => {
                return Err(OrchestratorError::Gate(
                    "recovery reports are verified by the app recovery journal".to_string(),
                ));
            }
            (DonePhase::Plan, DoneStatus::Completed) => {
                // Plan reports belong to plans; consuming one here would let
                // any in-flight build session smuggle manifest/doc edits.
                return Err(OrchestratorError::Gate(
                    "a run session reported phase=plan; plan reports belong to plans".to_string(),
                ));
            }
            (DonePhase::Route, DoneStatus::Completed) => {
                // A routing report belongs to a router session, which owns no
                // checkout and therefore no run.
                return Err(OrchestratorError::Gate(
                    "a run session reported phase=route; routing reports belong to router \
                     sessions"
                        .to_string(),
                ));
            }
            // A mid-run stage-doc revision does not advance the build: its
            // `done` is a store write-back to the plan, not a build report.
            // Routing it here would let `on_run_stage_session_done` commit and
            // validate as if the stage were built — reject and point the caller
            // at the cross-entity consumer.
            (DonePhase::Revise, DoneStatus::Completed) if active.revising_stage_id.is_some() => {
                return Err(OrchestratorError::Gate(
                    "this run has a stage-doc revision in flight; route the report to \
                     consume_run_stage_revision (it writes the revision back to the plan store)"
                        .to_string(),
                ));
            }
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed)
                if !plan_stage_docs.is_empty() =>
            {
                let consumed = self.on_run_stage_session_done(active, plan_stage_docs)?;
                if matches!(consumed.outcome, ReportOutcome::OutOfPhase(_)) {
                    return Ok(consumed);
                }
                next = consumed.next;
            }
            (DonePhase::Validate, DoneStatus::Completed) => {
                if let out_of_phase @ ReportOutcome::OutOfPhase(_) =
                    self.on_run_validation_done(active, plan_stage_docs, &report)?
                {
                    return Ok(ReportConsumed {
                        outcome: out_of_phase,
                        next: None,
                    });
                }
            }
            // A triage pass reported its classification. It moves no lifecycle
            // event and leaves no summary on the card: triage is an overlay on
            // the diff, and the diff's own report is what the reviewer reads.
            (DonePhase::Triage, DoneStatus::Completed) => {
                self.on_run_triage_done(active, &report)?;
                return Ok(ReportConsumed::applied());
            }
            // Single-doc plan / adopted path: a completed build opens review.
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed) => {
                if let Err(illegal) = active.run.apply(RunEvent::BuildReady) {
                    return Ok(ReportConsumed::out_of_phase(illegal));
                }
            }
        }
        // Only a consumed report leaves a trace (same discipline as plans).
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(ReportConsumed {
            outcome: ReportOutcome::Applied,
            next,
        })
    }
    /// A triage pass reported: check its ids against the diff it claims to
    /// describe, then keep it on the run.
    ///
    /// The ids are the one thing the tool boundary cannot check — the
    /// vocabulary belongs to a patch, and the patch lives here. An invented id
    /// fails the whole report rather than being dropped quietly: a triage that
    /// half-landed would order the review by a rule nobody stated.
    pub(super) fn on_run_triage_done(
        &self,
        active: &mut ActiveRun,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        // The mcp layer guarantees outputs.triage on triage/completed, but
        // reports also arrive over the daemon socket as raw JSON — a missing
        // one is rejected, never unwrapped.
        let Some(mut triage) = report.outputs.triage.clone() else {
            return Err(OrchestratorError::Gate(
                "triage/completed report carried no outputs.triage; rejected".to_string(),
            ));
        };
        // An override is the reviewer's word about the reviewer's own reading.
        // A report claiming to carry one is claiming to have been the human, so
        // it is refused whole rather than quietly stripped.
        if !triage.overrides.is_empty() {
            return Err(OrchestratorError::Gate(
                "triage/completed report carried overrides; only the reviewer writes those"
                    .to_string(),
            ));
        }
        let diff = self.run_diff(active)?;
        crate::mcp::check_triage_hunk_ids(&triage, &crate::diff::hunk_ids(diff.patch()))
            .map_err(|error| OrchestratorError::Gate(error.to_string()))?;
        // The reviewer's disagreements outlive the pass they were aimed at: a
        // hunk id is content-derived, so a hunk the new pass still names is
        // literally the same hunk, and what the reviewer said about it still
        // holds. One the new pass does not name is gone from the diff, and the
        // override with it — the durable record of that disagreement is the
        // project's review rules, not this list.
        if let Some(previous) = &active.triage {
            let classified: std::collections::HashSet<&str> = triage
                .hunks
                .iter()
                .map(|hunk| hunk.hunk_id.as_str())
                .collect();
            triage.overrides = previous
                .overrides
                .iter()
                .filter(|disagreement| classified.contains(disagreement.hunk_id.as_str()))
                .cloned()
                .collect();
        }
        active.triage = Some(triage);
        Ok(())
    }
    /// The triage turn for a run whose diff was just reported done: the same
    /// worktree agent, asked to say how much review each hunk needs.
    ///
    /// `patch` is the diff the reviewer will see and `revision_sha` names it, so
    /// a classification that arrives after the diff moved can be labelled stale
    /// rather than believed. `None` when the patch has no hunks — there is
    /// nothing to order, and a turn spent saying so is a turn wasted.
    pub fn triage_turn(
        &self,
        active: &ActiveRun,
        patch: &str,
        revision_sha: &str,
        agent_report: &str,
    ) -> Option<AgentTurn> {
        let hunks = crate::diff::patch_hunks(patch);
        if hunks.is_empty() {
            return None;
        }
        let diff_summary = hunks
            .iter()
            .map(|hunk| format!("{}  {}  {}", hunk.hunk_id, hunk.path, hunk.header))
            .collect::<Vec<_>>()
            .join("\n");
        let diff_ref = active
            .base_sha
            .clone()
            .unwrap_or_else(|| active.worktree.base_branch.clone());
        let rendered = crate::templates::render(
            &self.templates.triage,
            &crate::templates::Vars {
                goal: &active.run.goal,
                base_branch: &active.worktree.base_branch,
                diff_summary: &diff_summary,
                agent_report: agent_report_for_triage(agent_report),
                diff_ref: &diff_ref,
                revision_sha,
                ..crate::templates::Vars::default()
            },
        );
        Some(AgentTurn::dispatched(rendered, "triage"))
    }
    /// A stage build/fix session reported done(completed): commit the stage's
    /// work and hand the stage to validation. No run-level event — the run
    /// stays `Building` until validation's verdict moves it — and no new
    /// process: the hand-off is a turn for the same worktree agent, returned to
    /// the caller to deliver.
    pub(super) fn on_run_stage_session_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<ReportConsumed, OrchestratorError> {
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_run_done {}: build report for a multi-stage run with no current stage; \
                 ignoring",
                active.run.id.0
            );
            return Ok(ReportConsumed::applied());
        };
        // Resolve both sides of the stage join up front, so a mismatch between
        // the plan's manifest and the run's progress rejects before mutation.
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let Some(progress_index) = active.stage_progress_index(&stage_id) else {
            eprintln!(
                "on_run_done {}: no progress record for stage {stage_id}; ignoring",
                active.run.id.0
            );
            return Ok(ReportConsumed::applied());
        };
        match active.stages[progress_index].state {
            StageProgressState::Building => {
                // Coarse-state legality before ANY mutation: a report landing
                // while the run is Blocked/Failed is out of phase — the stage
                // advance, commit, and hand-off below would otherwise leave the
                // run and stage machines incoherent.
                if let Err(illegal) = run_transition(&active.run.state, RunEvent::BuildReady) {
                    return Ok(ReportConsumed::out_of_phase(illegal));
                }
            }
            // A build report while the validation agent runs would skip the
            // gate; only a `validate` report may move a Validating stage.
            StageProgressState::Validating | StageProgressState::Built => {
                eprintln!(
                    "on_run_done {}: stage {stage_id} is awaiting validation; ignoring a \
                     non-validate report",
                    active.run.id.0
                );
                return Ok(ReportConsumed::applied());
            }
            // Post-review change requests run while the current stage is
            // already validated; their `done` closes the loop exactly as on
            // the single-plan path.
            StageProgressState::Validated { .. } => {
                if let Err(illegal) = active.run.apply(RunEvent::BuildReady) {
                    return Ok(ReportConsumed::out_of_phase(illegal));
                }
                return Ok(ReportConsumed::applied());
            }
        }
        // The agent authors the stage's atomic commits; this is only a safety
        // net (a no-op on a clean tree) — but it stays load-bearing: it
        // GUARANTEES a committed boundary before the validation gate's
        // `git diff {stage_start_sha}` and before the next stage captures HEAD.
        self.commit_all_with_message(
            &active.worktree.path,
            &format!("Build: stage {stage_id} — checkpoint (swept by Build)"),
        )?;
        let built_sha = run_git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        // Commit/rev-parse are fallible. Only move Building → Built after both
        // succeeded, otherwise the caller could persist a Built stage with no
        // durable boundary.
        active.stages[progress_index].apply(StageProgressEvent::BuildDone)?;
        active.stages[progress_index].built_sha = Some(built_sha);
        active.stages[progress_index].completion_sha = None;
        active.stages[progress_index].invalidation_reason = None;
        active.stages[progress_index].publication = crate::run::StagePublication::Local;
        active.stages[progress_index].apply(StageProgressEvent::StartValidation)?;
        let prompt = self.render_run_stage(
            &self.templates.validate,
            active,
            plan_stage_docs,
            doc_index,
            "",
        );
        Ok(ReportConsumed {
            outcome: ReportOutcome::Applied,
            next: Some(AgentTurn::dispatched(prompt, "validate")),
        })
    }
    /// The validation agent's verdict. Pass: the final stage opens merge
    /// review, an inner stage parks the run at the stage gate. Fail: the stage
    /// gate with run-all disarmed; the stored report drives the fix session.
    ///
    /// PERIPHERY: with run-all armed and a mid-plan pass, the fused path
    /// auto-dispatched the next approved stage here; on the split that
    /// dispatch (StageGate → Building) lands with the stage flows.
    pub(super) fn on_run_validation_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        report: &DoneReport,
    ) -> Result<ReportOutcome, OrchestratorError> {
        if plan_stage_docs.is_empty() {
            eprintln!(
                "on_run_done {}: validate report for a run without stages; ignoring",
                active.run.id.0
            );
            return Ok(ReportOutcome::Applied);
        }
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_run_done {}: validate report with no current stage; ignoring",
                active.run.id.0
            );
            return Ok(ReportOutcome::Applied);
        };
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let Some(progress_index) = active.stage_progress_index(&stage_id) else {
            eprintln!(
                "on_run_done {}: no progress record for stage {stage_id}; ignoring",
                active.run.id.0
            );
            return Ok(ReportOutcome::Applied);
        };
        if active.stages[progress_index].state != StageProgressState::Validating {
            eprintln!(
                "on_run_done {}: stage {stage_id} is not validating; ignoring a validate report",
                active.run.id.0
            );
            return Ok(ReportOutcome::Applied);
        }
        // The mcp layer guarantees outputs.validation on validate/completed,
        // but reports also arrive over the daemon socket as raw JSON (a
        // version-skewed mcp binary, any local writer) — a missing report is
        // a rejected report, never a panic inside the app mutex.
        let Some(validation) = report.outputs.validation.clone() else {
            return Err(OrchestratorError::Gate(
                "validate/completed report carried no outputs.validation; rejected".to_string(),
            ));
        };
        let built_sha = active.stages[progress_index]
            .built_sha
            .clone()
            .ok_or_else(|| {
                OrchestratorError::Gate(format!(
                    "stage {stage_id} has no pinned built_sha; validation cannot establish a stable boundary"
                ))
            })?;
        let head = run_git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        let dirty = run_git(
            &active.worktree.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
        if head != built_sha || !dirty.trim().is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "validation must be observational: expected clean HEAD {built_sha}, found HEAD {head}{}",
                if dirty.trim().is_empty() {
                    "".to_string()
                } else {
                    format!(" with dirty files ({})", dirty.lines().count())
                }
            )));
        }
        let passed = validation.passed;
        let last_stage = doc_index + 1 == plan_stage_docs.len();
        let verdict = if passed {
            RunEvent::ValidationPassed { last_stage }
        } else {
            RunEvent::ValidationFailed
        };
        // Coarse-state legality before ANY mutation: a verdict landing while
        // the run is Blocked/Failed or already past its gate is out of phase —
        // advancing the stage to its terminal Validated here would strand the
        // run.
        if let Err(illegal) = run_transition(&active.run.state, verdict) {
            return Ok(ReportOutcome::OutOfPhase(illegal));
        }
        active.stages[progress_index].apply(StageProgressEvent::ValidationDone { passed })?;
        active.stages[progress_index].validation = Some(validation);
        if passed {
            active.stages[progress_index].completion_sha = Some(built_sha);
        }
        // The verdict parks the run at a gate; the worktree's agent stays live
        // and idle in its tab, which is the point of the tab — the reviewer
        // arrives at the gate already in conversation with a running process.
        active.run.apply(verdict)?;
        if !passed {
            active.auto_advance = false;
        }
        Ok(ReportOutcome::Applied)
    }
    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_run_idle(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::WentIdle)?;
        Ok(())
    }
}
