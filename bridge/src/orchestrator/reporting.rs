use crate::diff::DiffError;
use crate::git_process::{run_git, GitError};
use crate::harness::HarnessError;
use crate::mcp::{DoneReport, DoneStatus};
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
    /// reviewer's words are already durable in the thread and delivery adds
    /// them to the prompt, so the run context would be a repeat.
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
    /// reports through the stage pipeline.
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
    ) -> Result<ReportOutcome, OrchestratorError> {
        match report.status {
            // A blocked/failed report from any session — stage build or
            // single-plan — parks the run and disarms run-all.
            DoneStatus::Blocked => {
                if let Err(illegal) = active.run.apply(RunEvent::Blocked) {
                    return Ok(ReportOutcome::OutOfPhase(illegal));
                }
                active.auto_advance = false;
            }
            DoneStatus::Failed => {
                if let Err(illegal) = active.run.apply(RunEvent::Failed) {
                    return Ok(ReportOutcome::OutOfPhase(illegal));
                }
                active.auto_advance = false;
            }
            // A mid-run stage-doc revision does not advance the build: its
            // `done` is a store write-back to the plan, not a build report.
            // Routing it here would let `on_run_stage_session_done` commit the
            // stage as if it were built — reject and point the caller at the
            // cross-entity consumer.
            DoneStatus::Completed if active.revising_stage_id.is_some() => {
                return Err(OrchestratorError::Gate(
                    "this run has a stage-doc revision in flight; route the report to \
                     consume_run_stage_revision (it writes the revision back to the plan store)"
                        .to_string(),
                ));
            }
            DoneStatus::Completed if !plan_stage_docs.is_empty() => {
                if let out_of_phase @ ReportOutcome::OutOfPhase(_) =
                    self.on_run_stage_session_done(active, plan_stage_docs)?
                {
                    return Ok(out_of_phase);
                }
            }
            // Single-doc plan / adopted path: a completed build opens review.
            DoneStatus::Completed => {
                if let Err(illegal) = active.run.apply(RunEvent::BuildReady) {
                    return Ok(ReportOutcome::OutOfPhase(illegal));
                }
            }
        }
        // Only a consumed report leaves a trace (same discipline as plans).
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(ReportOutcome::Applied)
    }
    /// A stage's build session reported done(completed): commit the stage's
    /// boundary and complete it. The final stage opens merge review; any other
    /// parks the run at the stage gate, where the reviewer dispatches the next.
    pub(super) fn on_run_stage_session_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<ReportOutcome, OrchestratorError> {
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_run_done {}: build report for a multi-stage run with no current stage; \
                 ignoring",
                active.run.id.0
            );
            return Ok(ReportOutcome::Applied);
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
            return Ok(ReportOutcome::Applied);
        };
        // Post-review change requests run while the current stage is already
        // complete; their `done` closes the loop exactly as on the single-plan
        // path.
        if active.stages[progress_index].state == StageProgressState::Completed {
            if let Err(illegal) = active.run.apply(RunEvent::BuildReady) {
                return Ok(ReportOutcome::OutOfPhase(illegal));
            }
            return Ok(ReportOutcome::Applied);
        }
        let last_stage = doc_index + 1 == plan_stage_docs.len();
        let completed = RunEvent::StageCompleted { last_stage };
        // Coarse-state legality before ANY mutation: a report landing while
        // the run is Failed or already past its gate is out of phase — the
        // stage advance and commit below would otherwise leave the run and
        // stage machines incoherent.
        if let Err(illegal) = run_transition(&active.run.state, completed) {
            return Ok(ReportOutcome::OutOfPhase(illegal));
        }
        // The agent authors the stage's atomic commits; this is only a safety
        // net (a no-op on a clean tree) — but it stays load-bearing: it
        // GUARANTEES a committed boundary before the next stage captures HEAD.
        self.commit_all_with_message(
            &active.worktree.path,
            &format!("Build: stage {stage_id} — checkpoint (swept by Build)"),
        )?;
        let built_sha = run_git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        // Commit/rev-parse are fallible. Only complete the stage after both
        // succeeded, otherwise the caller could persist a complete stage with
        // no durable boundary.
        let progress = &mut active.stages[progress_index];
        progress.apply(StageProgressEvent::BuildDone)?;
        progress.built_sha = Some(built_sha.clone());
        progress.completion_sha = Some(built_sha);
        progress.invalidation_reason = None;
        progress.publication = crate::run::StagePublication::Local;
        active.run.apply(completed)?;
        Ok(ReportOutcome::Applied)
    }
    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_run_idle(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::WentIdle)?;
        Ok(())
    }
}
