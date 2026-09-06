//! The task-spine: where lifecycle, worktrees, PTY sessions, `done` reports, and
//! the diff come together — now split across the two entities of the plan/run
//! model.
//!
//! The orchestrator owns project-level configuration (the repo, where worktrees
//! go, the harness adapter, the prompt templates) and drives the split's two
//! entities through their lifecycles:
//!
//! - An [`ActivePlan`] (project-scoped) is authored by an agent running in the
//!   project's PRIMARY checkout, writing into a scratch docs dir outside the
//!   repo; its canonical docs live in the store. Its seams: `create_plan` +
//!   `prepare_plan_workspace` / `open_plan_drafting`,
//!   `on_plan_done`, the plan-review gates (`approve_plan`, `send_plan_notes`,
//!   the per-stage `approve_plan_stage` / `send_plan_stage_notes`), and the
//!   interaction verbs (`message_plan` / `resume_plan` / `abandon_plan`).
//! - An [`ActiveRun`] (worktree-scoped) is one implementation attempt on a
//!   `build/<slug>` branch. Its seams: `prepare_run_checkout` +
//!   `open_prepared_run` (and `open_adopted_implementation` on a checkout that
//!   already exists), `on_run_done` (build +
//!   validation, plus the sequential stage gate `dispatch_run_stage` /
//!   `fix_run_stage`), `run_diff`, the interaction verbs (`message_run` /
//!   `resume_run` / `run_request_changes`), the git finishers
//!   (`run_approve_merge` / `run_commit` / `run_push` / `run_merge_and_push`),
//!   `abandon_run_keeping_checkout`, and `adopt_run` (a run minted around a
//!   pre-existing worktree, `plan_id` `None` — the only plan-less runs left).
//!
//! The caller owns each active entity and hands it back by `&mut` for each
//! transition, so the orchestrator never hides state. The cross-entity seams —
//! the sequential stage gate (`dispatch_run_stage` consults the plan's
//! stage-doc states) and the mid-run stage-doc write-back
//! (`send_run_stage_notes` / `consume_run_stage_revision`) — take the other
//! entity's read-only view or `&mut` handle as a parameter rather than reaching
//! into any app-level map.
//!
//! The two pipes from the scope are both here: Build → agent is one turn handed
//! to the session ([`AgentSession::send_turn`](crate::harness::AgentSession::send_turn),
//! a framed paste into the warm PTY when that is the carrier); agent → Build is
//! [`on_plan_done`](Orchestrator::on_plan_done)
//! / [`on_run_done`](Orchestrator::on_run_done), the typed events the MCP server
//! forwards (the caller routes each report by owner lookup).

use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::PtySize;

use crate::agent::AgentRoster;
use crate::diff::{diff_against_base, diff_against_merge_base, DiffError, WorktreeDiff};
use crate::harness::HarnessError;
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::{AgentProvider, ModelChoice};
use crate::plan::{
    plan_transition, stage_doc_transition, IllegalPlanTransition, IllegalStageDocTransition, Plan,
    PlanEvent, PlanId, PlanState, StageDoc, StageDocEvent, StageDocState, StageManifestEntry,
};
use crate::pty::HarnessSpec;
use crate::run::{
    run_transition, IllegalRunTransition, IllegalStageProgressTransition, Run, RunEvent, RunId,
    RunState, StageProgress, StageProgressEvent, StageProgressState,
};
use crate::store::{PersistedPlan, PersistedRun, Store, StoreError};
use crate::templates::{self, Templates, Vars, DEFAULT_PLAN_PATH};
use crate::thread::DocComment;
use crate::worktree::{
    configured_remote_for_branch, derive_adoption_goal, slugify, ExternalWorktree, Worktree,
    WorktreeError, WorktreeManager,
};

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

/// Convert any git failure hit during a merge approval into [`OrchestratorError::MergeFailed`]
/// so the RPC message carries the contract's `merge_failed:` prefix.
fn as_merge_failure(error: OrchestratorError) -> OrchestratorError {
    match error {
        OrchestratorError::Git(reason) => OrchestratorError::MergeFailed(reason),
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
fn merge_stage_docs(stages: &mut Vec<StageDoc>, entries: &[StageManifestEntry]) {
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

/// The completion report as the triage prompt reads it: the agent's own account
/// of what it just did, in markdown. Empty lists are left out; a report with
/// nothing in it says so, because "(none)" is information and a blank is not.
fn render_completion_report(report: Option<&crate::thread::CompletionReport>) -> String {
    let Some(report) = report else {
        return "(the agent reported nothing)".to_string();
    };
    let sections = [
        ("Critical files", &report.critical_files),
        ("Risks", &report.risk_notes),
        ("Decisions", &report.decisions),
        ("Deliberately skipped", &report.skips),
    ];
    let rendered = sections
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(title, lines)| {
            let body = lines
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{title}:\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if rendered.is_empty() {
        return "(the agent reported nothing)".to_string();
    }
    rendered
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
    fn dispatched(rendered: String, phase: &'static str) -> AgentTurn {
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
    fn posted(rendered: String, nudge: &str, phase: &'static str) -> AgentTurn {
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

/// Where an issue's planning agent works.
///
/// Planning never gets a worktree: the agent runs in the project's PRIMARY
/// checkout (it reads the code as it stands on the base branch and writes no
/// code at all), and the plan documents it produces go to a scratch docs
/// directory outside the repo, whose path the prompt hands it. `done(plan)`
/// ingests that directory into the store, which is where the canonical docs
/// live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWorkspace {
    /// The project's primary checkout — the agent's working directory.
    pub checkout: PathBuf,
    /// The scratch directory the plan docs are written into, laid out exactly
    /// as the store holds them (`.build/plan/…`), outside the repo so planning
    /// never dirties the primary checkout.
    pub docs_dir: PathBuf,
}

/// One plan in flight: its lifecycle state, its planning workspace (while one
/// is alive), and its warm session. The canonical docs live in the store — the
/// docs dir is throwaway scratch space for the plan agent.
pub struct ActivePlan {
    pub plan: Plan,
    /// Where the planning agent works: the primary checkout plus its scratch
    /// docs dir. `None` before the session starts (an inert issue) and once
    /// the workspace is dropped (approve / abandon) — the store docs are
    /// canonical either way.
    pub workspace: Option<PlanWorkspace>,
    /// The branch the plan is written against — what the primary checkout is
    /// expected to be on, and what a run cuts its worktree from.
    pub base_branch: String,
    /// Where the plan doc lives, docs-dir-relative — convention by default,
    /// updated from `done` outputs (and fenced by the ingest).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    pub stages: Vec<StageDoc>,
    /// The stage a plan-revision session is (or was last) running for. Not
    /// persisted on the plan record — a restart falls back to a full re-plan.
    pub revising_stage_id: Option<String>,
    /// Which model/effort this plan's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// This entity's agents. Each owns its own durable conversation; the
    /// roster reads as the first agent's, which is what entity-level events
    /// speak to. Harness processes are recorded per agent in `sessions`.
    pub agents: AgentRoster,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (unpersisted docs,
    /// harness crash). Cleared whenever the plan advances again.
    pub last_error: Option<String>,
}

impl ActivePlan {
    /// Reattach a plan recovered from the durable store after a daemon
    /// restart: the store docs are canonical and the PTY session is gone. The
    /// workspace is not restored — the next dispatch re-derives it (and
    /// re-materializes the docs) from the store. The caller (boot recovery)
    /// moves a working state to `Interrupted` itself.
    pub fn reattach(record: &PersistedPlan) -> Self {
        ActivePlan {
            plan: Plan {
                id: PlanId::new(record.id.clone()),
                goal: record.goal.clone(),
                state: record.state,
                archived_at: record.archived_at.clone(),
                implementation_intent: record.implementation_intent.clone(),
                implementation_activity: record.implementation_activity.clone(),
            },
            workspace: None,
            base_branch: record.base_branch.clone(),
            plan_path: record.plan_path.clone(),
            stages: record.stages.clone(),
            revising_stage_id: None,
            model_choice: record.model_choice(),
            agents: record.roster(),
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
        }
    }

    /// A plan is multi-stage iff its stage-doc manifest is non-empty.
    pub fn is_multi_stage(&self) -> bool {
        !self.stages.is_empty()
    }

    /// Index of a stage doc in manifest (= execution) order.
    pub fn stage_doc_index(&self, stage_id: &str) -> Result<usize, String> {
        self.stages
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))
    }

    /// Open comments on one stage, insertion order. Read off the Issue
    /// conversation, where the comments live as posts.
    pub fn open_comments_for(&self, stage_id: &str) -> Vec<DocComment> {
        self.agents.sole_thread().open_doc_comments_for(stage_id)
    }
}

/// Which checkout a run is being adopted around. The primary checkout is a
/// worktree like any other to everything downstream of adoption; the two
/// differ only in the gates that apply at the moment of minting and at the
/// lifecycle verbs that would remove a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptionScope {
    ExternalWorktree,
    PrimaryCheckout,
}

/// A checkout that passed every refusal adoption makes. Construction IS the
/// validation, so nothing downstream can refuse a checkout it has already
/// written a checkpoint commit into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptableCheckout {
    /// Git's internal worktree name, so teardown understands the checkout.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// The branch it has checked out. Never detached, never the base branch of
    /// an external worktree, never option-shaped.
    pub branch: String,
    /// HEAD commit subject. UNTRUSTED display text, and what names the run when
    /// the branch name says nothing.
    pub head_subject: String,
}

impl AdoptableCheckout {
    /// The three refusals, all of them pure. A checkout that fails one is
    /// refused with nothing on disk touched and nothing persisted.
    pub fn judge(
        checkout: &ExternalWorktree,
        base_branch: &str,
        scope: AdoptionScope,
    ) -> Result<AdoptableCheckout, OrchestratorError> {
        let Some(branch) = checkout.branch.clone() else {
            return Err(OrchestratorError::Gate(
                "cannot adopt a detached-HEAD worktree — check out a branch first".to_string(),
            ));
        };
        // A worktree sitting on the base branch is a mistake to adopt; the
        // primary checkout sitting on it is the normal case (it is the base
        // checkout), which is why the scopes are told apart here at all.
        if scope == AdoptionScope::ExternalWorktree && branch == base_branch {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree with the base branch {base_branch:?} checked out"
            )));
        }
        // The branch name is an EXTERNAL, untrusted string handed to `git merge`
        // / `git push` as a bare argv element later; a leading `-` would be read
        // as an option (arbitrary code execution). Native branches are always
        // `build/<slug>` and can never trip this.
        if branch.starts_with('-') {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree whose branch name {branch:?} looks like a command-line \
                 option — rename the branch first"
            )));
        }
        Ok(AdoptableCheckout {
            name: checkout.name.clone(),
            path: checkout.path.clone(),
            branch,
            head_subject: checkout.head_subject.clone(),
        })
    }

    /// The checkout as Build records it. One shape, read by the scaffold and by
    /// the run alike, so the two can never disagree about what was adopted.
    pub fn worktree(&self, base_branch: &str) -> Worktree {
        Worktree {
            name: self.name.clone(),
            path: self.path.clone(),
            recorded_branch: self.branch.clone(),
            base_branch: base_branch.to_string(),
        }
    }
}

/// One run in flight: one implementation attempt — a worktree on a
/// `build/<slug>` branch, its lifecycle state, per-stage execution progress,
/// and the warm session. An adopted run is one whose `run.plan_id` is `None`.
pub struct ActiveRun {
    pub run: Run,
    pub worktree: Worktree,
    /// The "plan: <goal>" materialization commit recorded at dispatch — the
    /// baseline of the run's review diff, keeping the materialized docs out of
    /// review noise. `None` for adopted/migrated runs (the diff falls
    /// back to the merge-base).
    pub base_sha: Option<String>,
    /// Worktree-relative path the build prompts point at: the owning plan's
    /// `plan_path`, or the convention default for adopted runs. In-memory only —
    /// the caller re-derives it from the plan record on reattach.
    pub plan_path: String,
    /// Run-side per-stage execution progress, keyed by the plan's stage ids.
    /// A progress record exists only once its stage has been dispatched.
    pub stages: Vec<StageProgress>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    pub current_stage_id: Option<String>,
    /// The stage a mid-run revision session is running for (disambiguates
    /// store write-back from post-review changes on `done(revise)`).
    /// PERIPHERY: the mid-run revision flow itself lands with the stage flows.
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    ///
    /// Read by the prune rules, boot-recovery parking, the release verb and the
    /// SPA's option sets — and by nothing about conversations: an adopted
    /// branch's first agent starts a conversation of its own like any other,
    /// because the one the human was already having is one Build never heard
    /// and cannot show.
    pub adopted: bool,
    /// The last triage pass over this run's diff, with the revision it read.
    /// Presentational: nothing in the lifecycle reads it, and a stale one still
    /// ships (the SPA labels it) until the re-triage lands.
    pub triage: Option<crate::run::TriageReport>,
    /// Durable nonce-bound branch recovery attempt, if one is active or last
    /// completed. The app layer owns verification and lifecycle events.
    pub recovery: Option<crate::run::RecoveryAttempt>,
    /// Push/merge write-ahead intent, retained across a crash until repository
    /// refs independently prove or disprove publication.
    pub publication_attempt: Option<crate::run::PublicationAttempt>,
    /// Which model/effort this run's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// This entity's agents — see [`ActivePlan::agents`]. A branch carries as
    /// many as the human adds; they share the one worktree.
    pub agents: AgentRoster,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (merge failure,
    /// harness crash). Cleared whenever the run advances again.
    pub last_error: Option<String>,
}

impl ActiveRun {
    /// Reattach a run recovered from the durable store after a daemon restart:
    /// the worktree survived on disk, the PTY session did not. `plan_path` is
    /// re-derived by the caller from the owning plan's record (adopted runs pass
    /// the convention default). The caller (boot recovery) moves a working
    /// state to `Interrupted` itself.
    pub fn reattach(record: &PersistedRun, plan_path: String) -> Self {
        ActiveRun {
            run: Run {
                id: RunId::new(record.id.clone()),
                plan_id: record.plan_id.clone().map(PlanId::new),
                goal: record.goal.clone(),
                state: record.state,
            },
            worktree: Worktree {
                name: record.worktree_name.clone(),
                path: PathBuf::from(&record.worktree_path),
                recorded_branch: record.branch.clone(),
                base_branch: record.base_branch.clone(),
            },
            base_sha: record.base_sha.clone(),
            plan_path,
            stages: record.stages.clone(),
            current_stage_id: record.current_stage_id.clone(),
            revising_stage_id: record.revising_stage_id.clone(),
            auto_advance: record.auto_advance,
            adopted: record.adopted,
            triage: record.triage.clone(),
            recovery: record.recovery.clone(),
            publication_attempt: record.publication_attempt.clone(),
            model_choice: record.model_choice(),
            agents: record.roster(),
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
        }
    }

    /// This run's progress record for one stage, if the stage was dispatched.
    pub fn stage_progress(&self, stage_id: &str) -> Option<&StageProgress> {
        self.stages.iter().find(|p| p.stage_id == stage_id)
    }

    fn stage_progress_index(&self, stage_id: &str) -> Option<usize> {
        self.stages.iter().position(|p| p.stage_id == stage_id)
    }
}

/// What a run implements: an approved plan, whose docs are materialized from the
/// store into the run's worktree. Every dispatched run has one — an unplanned
/// coding session is an agent terminal the human drives, not a run.
pub struct RunSource<'a> {
    pub plan: &'a ActivePlan,
    /// The caller's active-runs view (the orchestrator holds no app-level maps):
    /// `true` when the plan already has a non-terminal run, which rejects the
    /// dispatch — the single-active-writer rule.
    pub has_active_run: bool,
}

/// The checkout an implementation opens in, as the git left it: on its own
/// branch, scaffolded, with the Issue's canonical docs committed. `base_sha` is
/// that commit — the baseline the review diff is read against.
pub struct PreparedImplementation {
    pub worktree: Worktree,
    pub base_sha: String,
}

/// An Issue cleared to have an implementation opened for it, and everything
/// opening one needs before any git runs: the words its checkout is named
/// after, and the plan whose canonical docs are committed into it as the
/// review baseline.
///
/// Construction IS the gate — a plan that is not ready, one somebody else is
/// already writing for, or one whose first stage the human has not approved
/// never becomes one — so nothing downstream can cut a checkout for work that
/// was refused.
pub struct ImplementableIssue {
    plan_id: String,
    goal: String,
    slug: String,
}

impl ImplementableIssue {
    /// What every implementation of an Issue must be true of before any
    /// checkout is touched, whichever worktree it is going to run in: the plan
    /// is ready, nobody else is writing for it, and the stage the first session
    /// would build is one the human approved.
    pub fn judge(source: RunSource<'_>) -> Result<ImplementableIssue, OrchestratorError> {
        let RunSource {
            plan: plan_link,
            has_active_run,
        } = source;
        if plan_link.plan.state != PlanState::Approved {
            return Err(OrchestratorError::Gate(format!(
                "only an approved plan can be implemented (plan {} is {:?})",
                plan_link.plan.id.0, plan_link.plan.state
            )));
        }
        if has_active_run {
            return Err(OrchestratorError::Gate(format!(
                "plan {} already has an active run — a second concurrent run is \
                 rejected (single-active-writer)",
                plan_link.plan.id.0
            )));
        }
        // Dispatch spawns the first stage's build session immediately, so its doc
        // must carry a live approval. `approve_plan` already guarantees this for
        // natively approved plans; migrated plans (and revision-staled docs on a
        // re-run) are re-gated here.
        if let Some(first_stage) = plan_link.stages.first() {
            if first_stage.state != StageDocState::Approved {
                return Err(OrchestratorError::Gate(format!(
                    "cannot implement plan {}: stage {:?} is not approved",
                    plan_link.plan.id.0, first_stage.id
                )));
            }
        }
        Ok(ImplementableIssue {
            plan_id: plan_link.plan.id.0.clone(),
            goal: plan_link.plan.goal.clone(),
            slug: slugify(&plan_link.plan.goal),
        })
    }

    /// What the checkout this implementation cuts is named after — the board's
    /// placeholder id is hashed from the path it makes, so the decide phase and
    /// the git that follows it must read the slug from one place.
    pub fn slug(&self) -> &str {
        &self.slug
    }
}

/// Per-spawn context an interactive harness builder may honor.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Resume the harness's own most-recent conversation for this cwd (claude:
    /// `--continue`, codex: `resume --last`) — a GUESS, since what it reopens
    /// is the newest conversation in the checkout whoever was having it. Set
    /// for a respawn of an agent with recorded history, which is that agent
    /// continuing its own conversation, and never for a brand-new agent.
    pub continue_session: bool,
    /// Resume the conversation the agent's last session NAMED (claude:
    /// `--resume <id>`, codex: `resume <SESSION_ID>`), when one was recorded
    /// and the provider still holds it.
    ///
    /// An alternative to `continue_session`, never a companion: this names the
    /// exact conversation Build was speaking to, and `--continue` guesses the
    /// newest one in the cwd. Every carrier can carry a name: a protocol
    /// announces it, while a terminal either knows it at launch or locates it
    /// from durable harness records.
    pub resume_session_id: Option<String>,
    /// Entity whose per-session MCP server receives the terminal `done` report.
    pub owner_id: String,
    /// Unlogged capability for this exact harness process. The daemon rotates it
    /// whenever the worktree's agent tab is replaced, preventing another local
    /// process from forging reports with only a known entity id.
    pub mcp_session_token: String,
    /// The worktree the harness will run in. Providers gate an interactive
    /// session behind a workspace-trust dialog for a directory they have not
    /// seen before, and Build mints a fresh worktree per run — so the adapter
    /// needs the path to pre-trust it, or the dialog eats the injected prompt.
    pub cwd: PathBuf,
}

/// Builds an interactive harness command for a rendered prompt + model + context.
///
/// The prompt is supplied so test and custom adapters can inspect the turn being
/// dispatched, but it is always submitted through the tab's PTY, never baked
/// into argv.
pub type WarmBuilder = std::sync::Arc<
    dyn Fn(&str, &ModelChoice, &SpawnOptions) -> Result<HarnessSpec, HarnessError> + Send + Sync,
>;

/// Whether the harness has an existing conversation transcript for a worktree
/// cwd — the one question `--continue` turns on. Its whole job is picking a
/// conversation back up that Build did not start in this process: a tab
/// respawned after a daemon restart or a crash, or an agent the user ran in
/// the worktree by hand before Build ever looked at it. Injectable so tests
/// never touch the real home directory.
pub type TranscriptProbe = std::sync::Arc<dyn Fn(&Path, AgentProvider) -> bool + Send + Sync>;

/// Builds the watcher that will name the conversation a session about to open
/// in a worktree cwd is having. Called at the spawn reservation, BEFORE the
/// child exists, so what the harness already wrote there can be told from what
/// the child writes. `None` for a provider whose session names its own
/// conversation. Injectable for the same reason the probe is: tests never read
/// the developer's real transcript tree.
pub type SessionLocatorFactory = std::sync::Arc<
    dyn Fn(&Path, AgentProvider) -> Option<Box<dyn crate::harness::SessionLocator>> + Send + Sync,
>;

/// Whether the conversation a recorded id names is still in the provider's
/// tree — asked before the id is spent, so a dead one costs zero restarts
/// rather than one. The sibling of [`TranscriptProbe`], injectable for the same
/// reason.
pub type ResumeIdProbe = std::sync::Arc<dyn Fn(&Path, AgentProvider, &str) -> bool + Send + Sync>;

const THREAD_NOTIFICATION: &str = "New reviewer messages are available. Call `read_unread_messages` now and act on every unread message.";

/// How long a failed prompt write waits for the harness's exit status to
/// become reapable before the failure is treated as fatal. Long enough to
/// cover the kernel's close-fds-then-reap lag for a harness that exited
/// under the write; short enough that a genuinely wedged PTY still surfaces
/// its write error promptly.
pub(crate) const PROMPT_WRITE_EXIT_GRACE: std::time::Duration =
    std::time::Duration::from_millis(250);

/// How long a fresh spawn waits for the harness's first output before writing
/// the prompt into its PTY. Real harnesses are interactive TUIs: injecting the
/// prompt before the TUI has started servicing the PTY risks it landing on a
/// startup screen. First output is the readiness signal; when the grace
/// expires the prompt is written anyway — a spawn that silently never delivers
/// its prompt is worse than one that races the startup screen.
/// Upper bound on waiting for a harness to become ready. Must comfortably
/// exceed a real TUI's full startup or the wait expires and the prompt is
/// written into a still-painting screen, which is the failure it exists to
/// prevent. claude 2.1.219 settled at ~1.8s; 2.1.223 (statusline hooks, MCP
/// config load) does not enable bracketed paste until ~4s on an idle machine,
/// so the old 6s bound left no margin at all under load — and an expired wait
/// writes into the startup screen, where the alternate-screen clear eats it.
pub(crate) const HARNESS_READY_GRACE: std::time::Duration = std::time::Duration::from_millis(20000);

/// How long a checkout's removal waits for the agents that were writing into it
/// to die.
///
/// `remove_dir_all` walking a directory a child is still creating files in
/// fails the walk, so kill, reap, THEN remove is the order that makes the
/// removal reliable. A SIGKILLed harness reaps in milliseconds; this is the
/// bound on one wedged in uninterruptible I/O, after which the removal is
/// attempted anyway — best-effort, as it has always been.
pub(crate) const CHECKOUT_REAP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How many messages a resumed agent's catch-up packet carries.
///
/// The limit counts messages, never items (§6.1), so a session that emitted
/// hundreds of tool calls still hands its replacement what the human said. The
/// 12 KB byte bound below is the real cap on how much that is.
pub const CATCH_UP_MESSAGES: usize = 40;

/// The cold prompt: the rendered instruction and the conversation protocol
/// every new agent process needs before it touches anything.
///
/// What it deliberately does NOT carry is the durable conversation — the
/// catch-up packet and the previous completion report. Those are composed onto
/// this at delivery ([`append_durable_conversation`]), because only the daemon
/// draining the queue can read the conversation's history out of the store,
/// and because a packet baked when the turn was queued misses whatever was
/// said while it waited for the lock.
pub(crate) fn conversation_prompt(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len() + 2048);
    out.push_str(prompt);
    // This block is the canonical reply policy. The `post_thread_message` tool
    // description in mcp.rs and NEW_THREAD_MESSAGES_PROMPT in app.rs defer to
    // it by reference — never restate these bullets elsewhere, restated copies
    // drift. The ambiguity rule stays above the silent-directive allowance so
    // an in-order reader hits the carve-out before committing to silence.
    out.push_str(
        "\n\nBuild conversation protocol:\n\
         - Before acting, call `read_unread_messages` and process every unread Issue message.\n\
         - When Build says new reviewer messages are available, call `read_unread_messages`.\n\
         - If a reviewer message reads as either a question or a directive, post a one-line clarifying reply via `post_thread_message` instead of silently changing code.\n\
         - You may implement an unambiguous directive without replying; the next revision is its acknowledgment.\n\
         - Call `post_thread_message` only for a question, necessary pushback or clarification, or an explicit request for a response.\n\
         - Do not post acknowledgments or diff recaps.\n\
         - When the reply you need is a choice you can enumerate, send `options` with the message: each is a chip the reviewer presses, and what comes back is an ordinary reviewer message. Write each option's `message` as the full instruction it stands for, not a repeat of its label — that text is what a later session sees. Anything said afterwards closes the offer.\n\
         - A message may carry files (`attachments`, each with a `path`). Open every one before acting on that message: the reviewer attached it because the words alone do not carry what they mean.\n",
    );
    out
}

/// Close a cold prompt with the durable conversation: the catch-up packet the
/// caller assembled, and the structured report the last session ended on.
///
/// Kept newest-first inside the byte bound — a packet clipped from the front
/// loses the oldest lines rather than the ones that just happened.
pub(crate) fn append_durable_conversation(
    mut out: String,
    catch_up: &str,
    thread: &crate::thread::Thread,
) -> String {
    if !catch_up.is_empty() {
        out.push_str("\nCatch-up packet from the durable conversation (oldest to newest):\n");
        if catch_up.len() <= 12_000 {
            out.push_str(catch_up);
        } else {
            let mut boundary = catch_up.len() - 12_000;
            while !catch_up.is_char_boundary(boundary) {
                boundary += 1;
            }
            out.push_str(&catch_up[boundary..]);
        }
        out.push('\n');
    }
    if let Some(report) = &thread.last_completion {
        out.push_str("\nPrevious structured completion report:\n");
        out.push_str(&serde_json::to_string(report).unwrap_or_default());
        out.push('\n');
    }
    out
}

fn append_stage_catalog(
    mut prompt: String,
    stages: &[StageDoc],
    status_for: impl Fn(&str) -> String,
) -> String {
    prompt.push_str("\n\nOrdered Issue stage-plan catalog (authoritative order):\n");
    if stages.is_empty() {
        prompt.push_str("- No stage plans exist yet.\n");
    } else {
        for stage in stages {
            prompt.push_str(&format!(
                "- {} — {} — {} — approval: {:?}; predecessor/execution status: {}\n",
                stage.id,
                stage.title,
                stage.path,
                stage.state,
                status_for(&stage.id)
            ));
        }
    }
    prompt
}

/// The scratch docs dir a plan's prompts point its agent at. Empty when the
/// plan has no workspace — no session is being rendered for it either.
fn plan_docs_dir_display(active: &ActivePlan) -> String {
    active
        .workspace
        .as_ref()
        .map(|workspace| workspace.docs_dir.display().to_string())
        .unwrap_or_default()
}

/// What a stage revision needs to be legal, and the doc it is against.
/// Pure, and checked before any disk work: nothing is scaffolded for a
/// revise that will be refused.
pub fn gate_plan_stage_notes(
    active: &ActivePlan,
    stage_id: &str,
) -> Result<usize, OrchestratorError> {
    let index = active
        .stage_doc_index(stage_id)
        .map_err(OrchestratorError::Gate)?;
    plan_transition(&active.plan.state, PlanEvent::SendNotes)
        .map_err(|e| OrchestratorError::Gate(format!("cannot send stage notes: {e}")))?;
    if active.open_comments_for(stage_id).is_empty() {
        return Err(OrchestratorError::Gate(format!(
            "no open comments on stage {stage_id}"
        )));
    }
    Ok(index)
}

/// What a freeform message costs the plan machine: nothing while it is
/// drafting, a `Reply` out of a parked state. Pure, and the refusals are
/// made here — before any disk work, and before the caller has anything to
/// persist but the message itself.
pub fn gate_plan_message(
    active: &ActivePlan,
    message: &str,
) -> Result<Option<PlanEvent>, OrchestratorError> {
    if message.trim().is_empty() {
        return Err(OrchestratorError::Gate("message must not be empty".into()));
    }
    use crate::plan::PlanState as S;
    let event = match active.plan.state {
        S::Drafting => None,
        S::Blocked | S::Failed | S::IdleUnreported | S::Interrupted => Some(PlanEvent::Reply),
        S::PlanReview => {
            return Err(OrchestratorError::Gate(
                "the plan is at the review gate — use send notes there".into(),
            ))
        }
        S::Created | S::Approved | S::Abandoned => {
            return Err(OrchestratorError::Gate(
                "no plan agent session to message".into(),
            ))
        }
    };
    if let Some(event) = event {
        plan_transition(&active.plan.state, event)?;
    }
    Ok(event)
}

/// Whether a directory holds at least one file, at any depth. A scratch docs
/// dir that holds nothing is one the canonical docs must be restored into.
fn dir_holds_a_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| match entry.file_type() {
        Ok(kind) if kind.is_dir() => dir_holds_a_file(&entry.path()),
        Ok(kind) => kind.is_file(),
        Err(_) => false,
    })
}

/// The worktree-relative name of one owner's MCP config. Every agent gets its
/// own, because the file names the owner the harness reports `done` for.
pub fn mcp_config_path(owner_id: &str) -> String {
    format!(".build/{}", mcp_config_name(owner_id))
}

fn mcp_config_name(owner_id: &str) -> String {
    // Agent ids are Crockford base32 with a fixed prefix, so this is always a
    // plain file name; anything else (a legacy entity id) is sanitized to one.
    let safe: String = owner_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("mcp-{safe}.json")
}

/// How the orchestrator launches an agent for a phase.
#[derive(Clone)]
pub enum Agent {
    /// A fixed warm interactive session: spawn the binary, then write the prompt
    /// to its PTY.
    Warm(HarnessSpec),
    /// A provider/model-aware warm interactive session. The builder supplies
    /// argv and environment; Build still injects the prompt through the PTY.
    WarmBuilder(WarmBuilder),
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String, OrchestratorError> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect();
        return Err(OrchestratorError::Git(format!(
            "git {args:?}: {}",
            detail.join("\n")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Owned inputs for constructing one agent process. Cloning this under the app
/// lock lets every filesystem and provider setup step run after that lock is
/// released.
#[derive(Clone)]
pub(crate) struct AgentLaunch {
    repo_path: PathBuf,
    bridge_exe: PathBuf,
    agent: Agent,
    pty_size: PtySize,
}

#[derive(Debug)]
pub(crate) struct PreparedAgentLaunch {
    pub(crate) spec: HarnessSpec,
    pub(crate) pty_size: PtySize,
}

impl AgentLaunch {
    /// Make the worktree launch-ready and return everything the app needs to
    /// spawn it. The ordered scaffold/spec boundary stays inside this operation.
    pub(crate) fn prepare(
        &self,
        owner_id: &str,
        cwd: &Path,
        model_choice: &ModelChoice,
        continue_session: bool,
        resume_session_id: Option<String>,
        mcp_session_token: &str,
    ) -> Result<PreparedAgentLaunch, OrchestratorError> {
        self.scaffold_agent_worktree(cwd, owner_id)?;
        let options = SpawnOptions {
            continue_session,
            resume_session_id,
            owner_id: owner_id.to_string(),
            mcp_session_token: mcp_session_token.to_string(),
            cwd: cwd.to_path_buf(),
        };
        let spec = match &self.agent {
            Agent::Warm(spec) => Ok(spec.clone()),
            Agent::WarmBuilder(build) => build("", model_choice, &options),
        }?;
        Ok(PreparedAgentLaunch {
            spec,
            pty_size: self.pty_size,
        })
    }

    fn scaffold_agent_worktree(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.write_build_dir(worktree_path, owner_id)
    }

    fn is_primary_checkout(&self, path: &Path) -> bool {
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical(path) == canonical(&self.repo_path)
    }

    fn exclude_build_machinery_repo_locally(&self) -> Result<(), OrchestratorError> {
        const RULES: [&str; 2] = [".build/mcp*.json", ".build/attachments/"];
        let git_dir = run_git(&self.repo_path, &["rev-parse", "--git-common-dir"])?
            .trim()
            .to_string();
        let git_dir = self.repo_path.join(git_dir);
        let exclude_path = git_dir.join("info").join("exclude");
        let existing = match std::fs::read_to_string(&exclude_path) {
            Ok(existing) => existing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let missing: Vec<&str> = RULES
            .into_iter()
            .filter(|rule| !existing.lines().any(|line| line.trim() == *rule))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(git_dir.join("info"))?;
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("# Build's machine-local agent plumbing\n");
        for rule in missing {
            updated.push_str(rule);
            updated.push('\n');
        }
        crate::store::write_file_atomically(&exclude_path, &updated)?;
        Ok(())
    }

    fn write_build_dir(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        let build_dir = worktree_path.join(".build");
        std::fs::create_dir_all(&build_dir)?;
        if self.is_primary_checkout(worktree_path) {
            self.exclude_build_machinery_repo_locally()?;
        } else {
            std::fs::write(build_dir.join(".gitignore"), "mcp*.json\nattachments/\n")?;
        }
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": self.bridge_exe.to_string_lossy(),
                    "args": ["mcp", "--task", owner_id]
                }
            }
        });
        std::fs::write(
            build_dir.join(mcp_config_name(owner_id)),
            serde_json::to_string_pretty(&mcp)?,
        )?;
        Ok(())
    }
}

/// Owns project configuration and drives plans and runs through their
/// lifecycles.
///
/// Cloneable, and cheaply: a verb clones its project's orchestrator under the
/// app mutex and then runs the git with the mutex released.
#[derive(Clone)]
pub struct Orchestrator {
    repo_path: PathBuf,
    /// Run (and legacy task) worktrees: `build/<slug>` branches.
    worktrees: WorktreeManager,
    /// Where each issue's scratch plan docs are written, one directory per
    /// issue. Outside the repo: planning writes no files into the checkout it
    /// runs in.
    plan_docs_root: PathBuf,
    launch: AgentLaunch,
    templates: Templates,
}

impl Orchestrator {
    pub fn new(
        repo_path: impl Into<PathBuf>,
        worktrees_root: impl Into<PathBuf>,
        agent: Agent,
        templates: Templates,
        bridge_exe: PathBuf,
    ) -> Self {
        let repo_path = repo_path.into();
        let worktrees_root = worktrees_root.into();
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root.clone());
        let plan_docs_root = worktrees_root.join(".issue-docs");
        Orchestrator {
            repo_path: repo_path.clone(),
            worktrees,
            plan_docs_root,
            launch: AgentLaunch {
                repo_path,
                bridge_exe,
                agent,
                pty_size: PtySize {
                    rows: 40,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                },
            },
            templates,
        }
    }

    pub(crate) fn agent_launch(&self) -> AgentLaunch {
        self.launch.clone()
    }

    // ---- Plan seams (Plan/Run split) --------------------------------------

    /// File a plan and start nothing: a record in `Created` with its goal as the
    /// conversation's first message, no worktree and no session.
    ///
    /// This is what an issue is before anyone has said anything to it — the
    /// router files them this way, and so does the toolbar's New issue. The goal
    /// is posted UNREAD: no agent exists to have read it, and
    /// [`start_plan_drafting`](Self::start_plan_drafting) is what marks it seen,
    /// because dispatching the session is the agent acting on it.
    pub fn create_plan(
        &self,
        id: PlanId,
        goal: impl Into<String>,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> ActivePlan {
        let plan = Plan::new(id, goal);
        let now = crate::store::now_rfc3339();
        let mut agents = AgentRoster::with_first(&plan.id.0, model_choice.clone(), &now);
        agents
            .sole_thread_mut()
            .post_user(plan.goal.clone(), None, &now);
        ActivePlan {
            plan,
            workspace: None,
            base_branch: base_branch.to_string(),
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            revising_stage_id: None,
            model_choice,
            agents,
            last_summary: None,
            last_error: None,
        }
    }

    /// Start the planning session for a plan that has none, once its workspace
    /// is real: the plan leaves
    /// `Created`, reads everything the human has said to it so far, and the
    /// turn that spawns its session is rendered.
    ///
    /// The docs dir is throwaway — the canonical docs land in the store at each
    /// plan/revise `done` — but the session stays warm through the
    /// notes/revision loop (the scope doc's warm-session property).
    ///
    /// Pure bookkeeping: the disk work was
    /// [`prepare_plan_workspace`](Self::prepare_plan_workspace), and it ran
    /// with the app mutex released. Workspace first, state second — a failed
    /// prepare leaves the plan inert and re-startable rather than `Drafting`
    /// with nothing drafting.
    pub fn open_plan_drafting(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
    ) -> Result<AgentTurn, OrchestratorError> {
        active.plan.apply(PlanEvent::Dispatch)?;
        active.workspace = Some(workspace);
        // Everything the user said before this moment is what the session is
        // being started to answer, so the dispatch reads all of it.
        let _ = active
            .agents
            .sole_thread_mut()
            .read_unread(&crate::store::now_rfc3339());
        let prompt = self.render_plan(&self.templates.plan, active, "");
        Ok(AgentTurn::dispatched(prompt, "plan"))
    }

    /// Where this issue's planning agent works — the primary checkout, always,
    /// plus the scratch docs dir that belongs to this issue alone.
    fn plan_workspace(&self, plan_id: &str) -> PlanWorkspace {
        PlanWorkspace {
            checkout: self.repo_path.clone(),
            docs_dir: self.plan_docs_root.join(plan_id),
        }
    }

    /// Make that workspace real, and hold every disk touch a planning workspace
    /// needs: the scratch docs dir exists, it holds the docs as they stand, and
    /// the primary checkout carries this issue's MCP config so its `done`
    /// reports route back here.
    ///
    /// An empty docs dir — a restart, a workspace dropped at approve, a plan
    /// being drafted for the first time — is filled from the canonical store;
    /// a plan the store holds no docs for simply starts from the empty one. A
    /// dir the agent is already working in is left exactly as it is:
    /// re-materializing would overwrite the revision in flight.
    ///
    /// This is the only way a planning workspace is written, and it is a
    /// [`WorktreeMutation`](crate::lifecycle::WorktreeMutation)'s work — every
    /// door to an Issue's planning agent reaches it with the app mutex
    /// released.
    pub fn prepare_plan_workspace(
        &self,
        plan_id: &str,
        store: &Store,
    ) -> Result<PlanWorkspace, OrchestratorError> {
        let workspace = self.plan_workspace(plan_id);
        // The stage-doc directory is made up front so the agent only ever has
        // to write files into a directory that is already there.
        std::fs::create_dir_all(workspace.docs_dir.join(templates::STAGES_DIR))?;
        self.launch
            .scaffold_agent_worktree(&workspace.checkout, plan_id)?;
        if !dir_holds_a_file(&workspace.docs_dir) {
            match store.materialize_plan_docs(plan_id, &workspace.docs_dir) {
                Ok(()) | Err(crate::store::StoreError::NoStoredDocs { .. }) => {}
                Err(error) => return Err(OrchestratorError::Store(error)),
            }
        }
        Ok(workspace)
    }

    /// Consume a plan agent's `done` report. Doc persistence is TRANSACTIONAL:
    /// the worktree docs are ingested into the store BEFORE the lifecycle
    /// transition fires, so an ingest failure errors the `done`, leaves the
    /// plan in its working state, and surfaces on the card via `last_error` —
    /// a plan never advances with unpersisted docs.
    pub fn on_plan_done(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        report: DoneReport,
    ) -> Result<(), OrchestratorError> {
        match (report.phase, report.status) {
            // A blocked/failed report from any plan-side session parks the plan.
            (_, DoneStatus::Blocked) => {
                active.plan.apply(PlanEvent::Blocked)?;
            }
            (_, DoneStatus::Failed) => {
                active.plan.apply(PlanEvent::Failed)?;
            }
            (DonePhase::Plan, DoneStatus::Completed) => {
                // Legality FIRST: a stray plan report must be rejected with
                // zero mutation — the caller persists the plan even on Err, so
                // a manifest merged (or docs ingested) before the check would
                // smuggle agent output past a closed gate.
                plan_transition(&active.plan.state, PlanEvent::PlanReady)?;
                let plan_path = report
                    .outputs
                    .plan_path
                    .clone()
                    .unwrap_or_else(|| active.plan_path.clone());
                self.ingest_plan_docs_transactionally(active, store, &plan_path)?;
                if let Some(entries) = &report.outputs.stages {
                    if !entries.is_empty() {
                        merge_stage_docs(&mut active.stages, entries);
                    }
                }
                active.plan.apply(PlanEvent::PlanReady)?;
                // The reported path was fenced and ingested; adopt it.
                active.plan_path = plan_path;
            }
            // A per-stage plan-revision session completed: the doc changed
            // (any prior approval is stale) and the agent's per-comment
            // resolutions land on the stored comments.
            (DonePhase::Revise, DoneStatus::Completed) => {
                self.consume_plan_stage_revision(active, store, &report)?;
            }
            (
                DonePhase::Build
                | DonePhase::Validate
                | DonePhase::Triage
                | DonePhase::Recover
                | DonePhase::Route,
                DoneStatus::Completed,
            ) => {
                return Err(OrchestratorError::Gate(format!(
                    "a planning session reported phase={:?}; plans only accept plan/revise \
                     reports",
                    report.phase
                )));
            }
        }
        // Only a consumed report leaves a trace: the surfaced summary and the
        // clearing of any stale error land strictly after the arms above
        // succeeded.
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }

    /// The transactional half of every plan/revise `done`: copy the scratch
    /// docs into the store, or fail the report with the reason surfaced on the
    /// card. Setting `last_error` here is the one deliberate mutation on the
    /// error path — the plan stays in its working state, but the reviewer must
    /// see why the gate never opened.
    fn ingest_plan_docs_transactionally(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        plan_path: &str,
    ) -> Result<(), OrchestratorError> {
        let Some(workspace) = &active.workspace else {
            let reason = "plan docs were not persisted: the planning workspace is gone".to_string();
            active.last_error = Some(reason.clone());
            return Err(OrchestratorError::Gate(reason));
        };
        if let Err(ingest_error) =
            store.ingest_plan_docs(&active.plan.id.0, &workspace.docs_dir, plan_path)
        {
            active.last_error = Some(format!("plan docs were not persisted: {ingest_error}"));
            return Err(OrchestratorError::Store(ingest_error));
        }
        Ok(())
    }

    /// A per-stage plan-revision session completed (plan-side `done(revise)`).
    /// Probes every transition before committing any, then ingests the revised
    /// docs transactionally, then lets the state moves land.
    fn consume_plan_stage_revision(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        let stage_id = active.revising_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "revise report for a plan with no stage revision in flight".to_string(),
            )
        })?;
        let index = active
            .stage_doc_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        stage_doc_transition(&active.stages[index].state, StageDocEvent::Revised)?;
        plan_transition(&active.plan.state, PlanEvent::PlanReady)?;
        let plan_path = active.plan_path.clone();
        self.ingest_plan_docs_transactionally(active, store, &plan_path)?;

        active.stages[index].state =
            stage_doc_transition(&active.stages[index].state, StageDocEvent::Revised)?;
        active.plan.apply(PlanEvent::PlanReady)?;
        if let Some(resolutions) = &report.outputs.comment_resolutions {
            for resolution in resolutions {
                let answers_this_stage = active
                    .open_comments_for(&stage_id)
                    .iter()
                    .any(|comment| comment.id == resolution.comment_id);
                if !answers_this_stage
                    || !active
                        .agents
                        .sole_thread_mut()
                        .resolve_doc_comment(&resolution.comment_id, &resolution.response)
                {
                    eprintln!(
                        "stage revision for {stage_id}: unknown or non-open comment {:?}; skipping",
                        resolution.comment_id
                    );
                }
            }
        }
        active.revising_stage_id = None;
        Ok(())
    }

    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_plan_idle(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        active.plan.apply(PlanEvent::WentIdle)?;
        Ok(())
    }

    /// Approve the plan: the last human gate. The plan rests at `Approved`
    /// (the store docs are canonical) and the scratch docs dir is dropped.
    /// Dropping it is best-effort: it holds a copy of what the store already
    /// has, so a leftover directory is clutter, never a reason to refuse the
    /// approval the reviewer just gave.
    pub fn approve_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        // Pure legality first — nothing is dropped for an illegal approve.
        // Stage docs deliberately do NOT gate the coarse approve: per-stage
        // review is progressive (later docs keep getting approved/revised
        // while an earlier stage builds); the dispatch seams re-gate each doc
        // at the moment its build session would spawn.
        plan_transition(&active.plan.state, PlanEvent::Approve)?;
        self.discard_plan_docs_dir(active);
        active.plan.apply(PlanEvent::Approve)?;
        active.last_error = None;
        Ok(())
    }

    /// Drop a plan's scratch docs dir and forget the workspace. Best-effort by
    /// design — the canonical docs are in the store.
    fn discard_plan_docs_dir(&self, active: &mut ActivePlan) {
        if let Some(workspace) = active.workspace.take() {
            if workspace.docs_dir.exists() {
                if let Err(cleanup) = std::fs::remove_dir_all(&workspace.docs_dir) {
                    eprintln!(
                        "plan {}: removing the scratch docs dir {} failed: {cleanup}",
                        active.plan.id.0,
                        workspace.docs_dir.display()
                    );
                }
            }
        }
    }

    /// Submit a batch of plan notes: re-plan against them and hand the caller
    /// the turn to deliver. An issue hosts exactly one agent, so the notes
    /// reach the process the reviewer has been reading, never a replacement.
    /// The workspace is kept through the notes loop — this is handed the one
    /// [`prepare_plan_workspace`](Self::prepare_plan_workspace) just made.
    pub fn open_plan_notes(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        notes: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        active.plan.apply(PlanEvent::SendNotes)?;
        active.workspace = Some(workspace);
        active.last_error = None;
        let prompt = self.render_plan(&self.templates.revise, active, notes);
        Ok(AgentTurn::posted(prompt, notes, "revise"))
    }

    /// Approve one stage's doc: `Planned` → `Approved`. Pure bookkeeping, no
    /// session — the plan-side successor of the fused `approve_stage`. Legal on
    /// any non-terminal plan (an `Approved` plan keeps taking per-stage
    /// approvals: that is how a run's later stages get their gate opened while
    /// an earlier one is already building).
    pub fn approve_plan_stage(
        &self,
        active: &mut ActivePlan,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        if active.plan.state.is_terminal() {
            return Err(OrchestratorError::Gate(format!(
                "cannot approve a stage on a terminal plan (state {:?})",
                active.plan.state
            )));
        }
        let index = active
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        active.stages[index].state =
            stage_doc_transition(&active.stages[index].state, StageDocEvent::Approve)?;
        Ok(())
    }

    /// Send a stage's open comments to a fresh plan-revision session (the
    /// per-stage successor of `send_plan_notes`): the persisted open comments
    /// ARE the payload, rendered server-side. The plan re-plans against them in
    /// its disposable worktree — kept warm through the loop, or re-created with
    /// the canonical docs materialized when it was torn down/vanished — and
    /// `revising_stage_id` routes the resulting `done(revise)` through
    /// [`consume_plan_stage_revision`](Self::consume_plan_stage_revision).
    pub fn open_plan_stage_notes(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        stage_id: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        let index = gate_plan_stage_notes(active, stage_id)?;
        active.plan.apply(PlanEvent::SendNotes)?;
        active.workspace = Some(workspace);
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        let prompt = self.render_plan_stage(
            &self.templates.revise_stage,
            active,
            index,
            THREAD_NOTIFICATION,
        );
        Ok(AgentTurn::posted(prompt, THREAD_NOTIFICATION, "revise"))
    }

    /// A freeform human message to the plan's agent (the plan-side `message`).
    /// A live `Drafting` session is redirected; parked states (blocked / failed
    /// / idle / interrupted) resume drafting with the message as the steer. The
    /// review gate is refused — `PlanReview` has the structured send-notes
    /// verb, and a freeform channel that moved the plan back to drafting would
    /// bypass the batched-review contract (`thread.post` is how you reach a
    /// plan agent at its gate without moving anything).
    pub fn open_plan_message(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        message: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        let event = gate_plan_message(active, message)?;
        // An interrupted plan lost its workspace; this is the one that was
        // re-made for it, docs and all.
        active.workspace = Some(workspace);
        let prompt = self.render_plan(&self.templates.message, active, message);
        if let Some(event) = event {
            active.plan.apply(event)?;
        }
        active.last_error = None;
        Ok(AgentTurn::posted(prompt, message, "message"))
    }

    /// Re-dispatch an interrupted plan phase in a fresh session (the plan-side
    /// `resume`). The plan machine has a single working phase, so the only
    /// routing is whether a per-stage revision was in flight
    /// (`revising_stage_id` → `revise_stage` with the stage's open comments) or
    /// a full (re-)plan. The prompt is routed BEFORE the `Reply` transition
    /// commits, so a routing failure never strands the plan out of its
    /// interrupted state (the caller persists it even on Err).
    pub fn open_plan_resume(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
    ) -> Result<AgentTurn, OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::Reply)?;
        active.workspace = Some(workspace);
        let prompt = match active.revising_stage_id.clone() {
            Some(stage_id) => {
                let index = active
                    .stage_doc_index(&stage_id)
                    .map_err(OrchestratorError::Gate)?;
                self.render_plan_stage(
                    &self.templates.revise_stage,
                    active,
                    index,
                    THREAD_NOTIFICATION,
                )
            }
            None => self.render_plan(&self.templates.plan, active, ""),
        };
        active.plan.apply(PlanEvent::Reply)?;
        active.last_error = None;
        Ok(AgentTurn::dispatched(prompt, "revise"))
    }

    /// Abandon a plan from any non-terminal state: kill the plan agent, mark
    /// the plan `Abandoned`, and drop its scratch docs dir. The cleanup is
    /// best-effort — a leftover directory is logged, never a reason to fail the
    /// abandon; the store docs are canonical and survive either way.
    pub fn abandon_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        active.plan.apply(PlanEvent::Abandon)?;
        self.discard_plan_docs_dir(active);
        Ok(())
    }

    /// Render a stage-scoped template for a plan (the plan-side twin of
    /// [`render_run_stage`](Self::render_run_stage)): the stage doc's own fields
    /// plus the next stage's doc path. The run-side variables (start sha,
    /// validation findings, prior-stage notes) are all empty — they belong to a
    /// run's execution progress, not a plan's doc review.
    fn render_plan_stage(
        &self,
        template: &str,
        active: &ActivePlan,
        index: usize,
        comments: &str,
    ) -> String {
        let doc = &active.stages[index];
        let next_stage_path = active
            .stages
            .get(index + 1)
            .map(|next| next.path.as_str())
            .unwrap_or("");
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                docs_dir: &plan_docs_dir_display(active),
                comments,
                base_branch: &active.base_branch,
                stage_id: &doc.id,
                stage_title: &doc.title,
                stage_path: &doc.path,
                stage_summary: &doc.summary,
                next_stage_path,
                stage_start_sha: "",
                findings: "",
                prior_notes: "",
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, &active.stages, |_| "not started".to_string())
    }

    // ---- Run seams (Plan/Run split) ----------------------------------------

    /// Create a worktree with nothing attached to it — no `.build/` scaffold, no
    /// run record, no session. The human works in it by hand (a terminal or an
    /// agent tab); Build only owns the directory and the branch it cut. It sits
    /// on the same `build/<slug>` naming as run worktrees so teardown, adoption
    /// and the scan all treat it identically.
    pub fn create_bare_worktree(
        &self,
        slug: &str,
        base_branch: &str,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self.worktrees.create(slug, base_branch)?)
    }

    /// The same bare checkout, on a branch that already exists — here or on a
    /// remote. A name no ref anywhere backs is refused, never cut.
    pub fn create_worktree_on_existing_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self
            .worktrees
            .create_on_existing_branch(branch, base_branch)?)
    }

    /// The same bare checkout, on a branch the caller named in full and means
    /// to start: an existing branch is checked out rather than cut a second
    /// time over the work it holds, and a name nothing backs is cut from the
    /// base exactly as it was given.
    pub fn create_worktree_cutting_named_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self.worktrees.create_cutting_branch(branch, base_branch)?)
    }

    /// Where the checkout for `slug` will go if nothing is in its way. The
    /// decide phase of a create has no directory to hash an id out of yet, and
    /// this is the path it expects one at.
    pub fn planned_checkout_path(&self, slug: &str) -> PathBuf {
        self.worktrees.path_for(slug)
    }

    /// One checkout of this repository, described the way the board's scan
    /// describes it. A git walk of that one directory: off the app mutex.
    pub fn describe_checkout(
        &self,
        path: &Path,
        base_branch: &str,
    ) -> Result<ExternalWorktree, OrchestratorError> {
        Ok(crate::worktree::describe_checkout(
            &self.repo_path,
            base_branch,
            path,
        )?)
    }

    /// Every checkout of this repository no run owns, as they stand right now.
    /// The whole-repository walk a dispatch or an adoption resolves against,
    /// and seconds of git on a repository with many worktrees: off the app
    /// mutex, always.
    pub fn scan_checkouts(
        &self,
        base_branch: &str,
        excluded: &std::collections::HashSet<PathBuf>,
    ) -> Result<Vec<ExternalWorktree>, OrchestratorError> {
        Ok(crate::worktree::discover_external_worktrees(
            &self.repo_path,
            base_branch,
            excluded,
        )?)
    }

    /// Cut the checkout an Issue's implementation works in and make it ready
    /// to be worked in: `build/<slug>` off the base branch, `.build/`
    /// scaffolded (the MCP config carries the run id), the plan's canonical
    /// docs materialized out of the store and committed ("plan: <goal>" — the
    /// intent record the scope doc keeps through merge).
    ///
    /// That commit is the answer's `base_sha`, the baseline of the review diff,
    /// so the materialized docs never show up as review noise.
    ///
    /// Git and disk from end to end, seconds of it on a large repository: off
    /// the app mutex, always. A failure after the checkout exists removes it,
    /// so a preparation nobody can be handed leaves nothing behind.
    pub fn prepare_run_checkout(
        &self,
        issue: &ImplementableIssue,
        base_branch: &str,
        run_id: &str,
        store: &Store,
    ) -> Result<PreparedImplementation, OrchestratorError> {
        let worktree = self.worktrees.create(&issue.slug, base_branch)?.worktree;
        let prepared = self.scaffold_build_dir(&worktree, run_id).and_then(|()| {
            self.materialize_and_commit_plan_docs(
                &issue.plan_id,
                &worktree.path,
                &issue.goal,
                store,
            )
        });
        match prepared {
            Ok(base_sha) => Ok(PreparedImplementation { worktree, base_sha }),
            Err(error) => {
                self.discard_checkout(&worktree, /* keep_branch */ false);
                Err(error)
            }
        }
    }

    /// The same preparation on a checkout that already exists — the branch's
    /// own uncommitted work is not part of what the implementation does, and it
    /// must not vanish under the baseline either, so it lands as its own commit
    /// below the docs commit.
    ///
    /// Two commits and a store read: off the app mutex, always.
    pub fn prepare_adopted_checkout(
        &self,
        issue: &ImplementableIssue,
        checkout: &Path,
        store: &Store,
    ) -> Result<String, OrchestratorError> {
        self.commit_all_with_message(
            checkout,
            "Checkpoint: before Build implements an Issue here",
        )?;
        self.materialize_and_commit_plan_docs(&issue.plan_id, checkout, &issue.goal, store)
    }

    /// Open the run that stands for a prepared checkout: the record, the agent
    /// that will do the work, and the turn that starts it.
    ///
    /// Pure bookkeeping — the git ran in [`prepare_run_checkout`], and the
    /// refusals were made when the [`ImplementableIssue`] was judged.
    ///
    /// A multi-stage plan's first session is its first stage's build — the
    /// plan-level `Approved` gate covers starting stage one; later stages
    /// dispatch from the stage gate.
    ///
    /// [`prepare_run_checkout`]: Self::prepare_run_checkout
    pub fn open_prepared_run(
        &self,
        id: RunId,
        plan_link: &ActivePlan,
        prepared: PreparedImplementation,
        model_choice: ModelChoice,
    ) -> Result<(ActiveRun, AgentTurn), OrchestratorError> {
        let mut run = Run::new(
            id,
            Some(plan_link.plan.id.clone()),
            plan_link.plan.goal.clone(),
        );
        run.apply(RunEvent::Dispatch)?;

        let agents = AgentRoster::with_first(
            &run.id.0,
            model_choice.clone(),
            &crate::store::now_rfc3339(),
        );
        let mut active = ActiveRun {
            run,
            worktree: prepared.worktree,
            base_sha: Some(prepared.base_sha),
            plan_path: plan_link.plan_path.clone(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            triage: None,
            recovery: None,
            publication_attempt: None,
            model_choice,
            agents,
            last_summary: None,
            last_error: None,
        };

        let turn = self.open_implementation(&mut active, plan_link);
        Ok((active, turn))
    }

    /// Bind an Issue's implementation to a checkout that already exists,
    /// instead of cutting `build/<slug>` for it. The branch's run adopts the
    /// implementation: whatever the branch was carrying was checkpointed under
    /// its own message, the stage docs were committed on top, and THAT commit is
    /// the review baseline — so the diff the human reviews is exactly what the
    /// implementation adds to the branch.
    ///
    /// The work is handed to a FRESH agent (Decisions §Entity model: issue
    /// implementation stays a handoff), which is why the caller gets the new
    /// agent's id back: the turn is addressed to it, not to whatever agent was
    /// already talking on this branch.
    ///
    /// Pure bookkeeping: `base_sha` is what
    /// [`prepare_adopted_checkout`](Self::prepare_adopted_checkout) committed,
    /// and the refusals were made when the [`ImplementableIssue`] was judged.
    pub fn open_adopted_implementation(
        &self,
        active: &mut ActiveRun,
        plan_link: &ActivePlan,
        base_sha: String,
        model_choice: ModelChoice,
    ) -> Result<(AgentTurn, String), OrchestratorError> {
        let mut run = Run::new(
            active.run.id.clone(),
            Some(plan_link.plan.id.clone()),
            plan_link.plan.goal.clone(),
        );
        run.apply(RunEvent::Dispatch)?;
        active.run = run;
        active.base_sha = Some(base_sha);
        active.plan_path = plan_link.plan_path.clone();
        active.stages = Vec::new();
        active.current_stage_id = None;
        active.revising_stage_id = None;
        active.auto_advance = false;
        active.recovery = None;
        active.publication_attempt = None;
        active.model_choice = model_choice.clone();
        active.last_summary = None;
        active.last_error = None;

        let agent_id = active
            .agents
            .add(&active.run.id.0, model_choice, &crate::store::now_rfc3339())
            .id
            .clone();
        let turn = self.open_implementation(active, plan_link);
        Ok((turn, agent_id))
    }

    /// The first turn of an implementation, on a run whose worktree is already
    /// prepared and whose baseline is already pinned.
    ///
    /// Multi-stage plan → the first stage's build session (progress record
    /// created, stage diff pinned to the materialization commit); a single-doc
    /// plan → the whole-plan build prompt.
    fn open_implementation(&self, active: &mut ActiveRun, plan_link: &ActivePlan) -> AgentTurn {
        let prompt = if plan_link.is_multi_stage() {
            let first_stage = &plan_link.stages[0];
            active.current_stage_id = Some(first_stage.id.clone());
            let mut progress = StageProgress::dispatched(&first_stage.id);
            progress.start_sha = active.base_sha.clone();
            active.stages.push(progress);
            self.render_run_stage(
                &self.templates.build_stage,
                active,
                &plan_link.stages,
                0,
                "",
            )
        } else {
            self.render_run(&self.templates.build, active, "", &plan_link.stages)
        };
        AgentTurn::dispatched(prompt, "build")
    }

    /// Materialize a plan's canonical docs into a fresh run worktree and
    /// commit them, returning the commit sha that baselines the run's review
    /// diff.
    fn materialize_and_commit_plan_docs(
        &self,
        plan_id: &str,
        checkout: &Path,
        goal: &str,
        store: &Store,
    ) -> Result<String, OrchestratorError> {
        store.materialize_plan_docs(plan_id, checkout)?;
        self.commit_all_with_message(checkout, &format!("plan: {goal}"))?;
        Ok(self
            .git(checkout, &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
    }

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
    fn on_run_triage_done(
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
        seed: Option<&crate::thread::CompletionReport>,
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
                completion_report: &render_completion_report(seed),
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
    fn on_run_stage_session_done(
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
        let built_sha = self
            .git(&active.worktree.path, &["rev-parse", "HEAD"])?
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
    fn on_run_validation_done(
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
        let head = self
            .git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        let dirty = self.git(
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

    /// The run's diff for review: baselined on the materialization commit
    /// (`base_sha`) when one was recorded — keeping the committed plan docs
    /// out of review noise while still surfacing any build-agent edits to
    /// them — falling back to the merge-base with the base branch for
    /// adopted/migrated runs.
    pub fn run_diff(&self, active: &ActiveRun) -> Result<WorktreeDiff, OrchestratorError> {
        match &active.base_sha {
            Some(sha) => Ok(diff_against_base(&active.worktree.path, sha)?),
            None => Ok(diff_against_merge_base(
                &active.worktree.path,
                &active.worktree.base_branch,
            )?),
        }
    }

    /// Dispatch one stage's build in a fresh cold session from the between-
    /// stages gate — the run-side successor of the fused `dispatch_stage`. The
    /// sequential gate lives here and is deliberately cross-entity without any
    /// map lookup: the caller passes the owning plan's stage docs, so the run
    /// checks (1) the plan marks THIS stage `Approved`, and (2) every earlier
    /// stage passed validation ON THIS RUN (its `StageProgress` is
    /// `Validated{passed:true}`). Only then does it capture `start_sha` and
    /// spawn.
    pub fn dispatch_run_stage(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        stage_id: &str,
        model_override: Option<ModelChoice>,
    ) -> Result<AgentTurn, OrchestratorError> {
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        // Run coarse-state legality first (Dispatch is legal from StageGate; the
        // very first stage comes through `open_prepared_run` instead) — nothing is
        // spawned for an illegal dispatch.
        run_transition(&active.run.state, RunEvent::Dispatch)
            .map_err(|e| OrchestratorError::Gate(format!("cannot dispatch a stage: {e}")))?;
        // Plan-side gate: the human approved this stage's doc.
        if plan_stage_docs[doc_index].state != StageDocState::Approved {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} is not approved (plan doc state {:?})",
                plan_stage_docs[doc_index].state
            )));
        }
        // Sequential gate: every earlier stage must have passed validation on
        // this run (the run consults its own progress, keyed by the plan's ids).
        if let Some(unvalidated) = plan_stage_docs[..doc_index].iter().find(|doc| {
            active
                .stage_progress(&doc.id)
                .map(|progress| progress.state)
                != Some(StageProgressState::Validated { passed: true })
        }) {
            return Err(OrchestratorError::Gate(format!(
                "stage {} has not passed validation yet",
                unvalidated.id
            )));
        }

        // Probe the candidate boundary before mutating the run machine. A
        // vanished/corrupt checkout must leave StageGate intact for recovery.
        let start_sha = self
            .git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        active.run.apply(RunEvent::Dispatch)?;
        // A fresh next stage has no progress record yet; create one (`Building`,
        // pinned to the current HEAD). A record already present keeps its
        // `start_sha` so the stage diff always covers all of its work.
        match active.stage_progress_index(stage_id) {
            Some(existing) => {
                if active.stages[existing].start_sha.is_none() {
                    active.stages[existing].start_sha = Some(start_sha);
                }
            }
            None => {
                let mut progress = StageProgress::dispatched(stage_id);
                progress.start_sha = Some(start_sha);
                active.stages.push(progress);
            }
        }
        active.current_stage_id = Some(stage_id.to_string());
        if let Some(choice) = model_override {
            active.model_choice = choice;
        }
        active.last_error = None;
        let prompt = self.render_run_stage(
            &self.templates.build_stage,
            active,
            plan_stage_docs,
            doc_index,
            "",
        );
        Ok(AgentTurn::dispatched(prompt, "build"))
    }

    /// Send a validation-failed stage back to a fresh fix session (the run-side
    /// `fix_stage`). The stored validation findings drive the prompt; `note` is
    /// the reviewer's optional steer. The stage's `start_sha` is kept across the
    /// fix re-dispatch so its diff still covers all of the stage's work.
    pub fn fix_run_stage(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        stage_id: &str,
        note: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        run_transition(&active.run.state, RunEvent::Dispatch)
            .map_err(|e| OrchestratorError::Gate(format!("cannot fix a stage: {e}")))?;
        let progress_index = active.stage_progress_index(stage_id).ok_or_else(|| {
            OrchestratorError::Gate(format!("stage {stage_id} has no progress to fix"))
        })?;
        if active.stages[progress_index].state != (StageProgressState::Validated { passed: false })
        {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} has no failed validation to fix (state {:?})",
                active.stages[progress_index].state
            )));
        }

        active.run.apply(RunEvent::Dispatch)?;
        // Validated{passed:false} → Building, keeping start_sha and the stored
        // findings the fix prompt consumes.
        active.stages[progress_index].apply(StageProgressEvent::Dispatch)?;
        active.current_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        let prompt = self.render_run_stage(
            &self.templates.fix_stage,
            active,
            plan_stage_docs,
            doc_index,
            note,
        );
        Ok(AgentTurn::dispatched(prompt, "build"))
    }

    /// Submit a batch of diff comments (the run-side `request_changes`): put the
    /// run back to work and hand the caller the turn to deliver. Valid both from
    /// `Review` (agent parked) and `Building` (agent still working). A stage
    /// awaiting its validation verdict is refused — only a `validate` report may
    /// move it, so redirecting it here would hang the run.
    ///
    /// The worktree's agent is never ended and never replaced: the reviewer is
    /// mid conversation with a process, and killing it to say something to it
    /// throws away the context that made the review worth having.
    /// `conversation_agent` names whose conversation the comments were posted
    /// to, for the catch-up packet a cold spawn opens on. `None` is the roster
    /// standing on the run — the first agent's, which the caller may have
    /// swapped for the Issue's.
    pub fn run_request_changes(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        comments: &str,
        conversation_agent: Option<&str>,
    ) -> Result<AgentTurn, OrchestratorError> {
        if let Some(stage_id) = active.current_stage_id.clone() {
            if let Some(progress) = active.stage_progress(&stage_id) {
                if matches!(
                    progress.state,
                    StageProgressState::Built | StageProgressState::Validating
                ) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the verdict \
                         before requesting changes"
                    )));
                }
            }
        }
        active.run.apply(RunEvent::RequestChanges)?;
        active.last_error = None;
        let rendered = self.render_run(
            &self.templates.review_changes,
            active,
            comments,
            plan_stage_docs,
        );
        // The agent whose conversation the reviewer was reading has to be on
        // this run's roster: the turn is addressed to it, and a name that is
        // not here is a caller error rather than a turn for somebody else.
        active
            .agents
            .resolve(conversation_agent)
            .map_err(OrchestratorError::Gate)?;
        Ok(AgentTurn::posted(rendered, comments, "revise"))
    }

    /// A freeform human message to the run's agent (the run-side `message`). A
    /// working run keeps working; parked states (blocked / failed / idle /
    /// interrupted) resume building.
    ///
    /// Review gates (`Review`, `StageGate`) are still refused, but the reason
    /// narrowed when the agent became persistent. It is no longer "there is no
    /// session to talk to" — there is, and `thread.post` reaches it at a gate
    /// without moving anything. It is that `run.message` MOVES the run back to
    /// `Building`, and what reopens a gate is the gate's own structured verb
    /// (request changes, dispatch a stage). A freeform side channel that
    /// restarts the build would bypass the batched-review contract.
    ///
    /// A stage awaiting its validation verdict is refused for the same reason
    /// as in `run_request_changes`: only a `validate` report may move it.
    pub fn message_run(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        message: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        if message.trim().is_empty() {
            return Err(OrchestratorError::Gate("message must not be empty".into()));
        }
        if let Some(stage_id) = active.current_stage_id.clone() {
            if let Some(progress) = active.stage_progress(&stage_id) {
                if matches!(
                    progress.state,
                    StageProgressState::Built | StageProgressState::Validating
                ) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the \
                         verdict before messaging the agent"
                    )));
                }
            }
        }
        use crate::run::RunState as S;
        let event = match active.run.state {
            S::Building => None,
            S::Blocked | S::Failed | S::IdleUnreported | S::Interrupted => Some(RunEvent::Reply),
            S::Review | S::StageGate => {
                return Err(OrchestratorError::Gate(
                    "the run is at a review gate — use request changes / dispatch a stage there"
                        .into(),
                ))
            }
            S::Created | S::Merged | S::Abandoned | S::Archived => {
                return Err(OrchestratorError::Gate(
                    "the run's conversation is closed — there is nothing to message".into(),
                ))
            }
        };
        if let Some(event) = event {
            // Pure legality first — the caller persists the run even on Err.
            run_transition(&active.run.state, event)?;
        }
        let prompt = self.render_run(&self.templates.message, active, message, plan_stage_docs);
        if let Some(event) = event {
            active.run.apply(event)?;
        }
        active.last_error = None;
        Ok(AgentTurn::posted(prompt, message, "message"))
    }

    /// Re-dispatch an interrupted build phase in a fresh session (the run-side
    /// `resume`). `plan_stage_docs` (the caller's join by `plan_id`; empty for a
    /// single-doc/adopted run) routes a multi-stage run by its current stage's
    /// persisted progress. The prompt is routed BEFORE the `Reply` transition
    /// commits, so a routing failure never strands the run out of its
    /// interrupted state.
    pub fn resume_run(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<AgentTurn, OrchestratorError> {
        run_transition(&active.run.state, RunEvent::Reply)?;
        let prompt = if plan_stage_docs.is_empty() {
            self.render_run(&self.templates.build, active, "", plan_stage_docs)
        } else {
            self.resume_run_stage_prompt(active, plan_stage_docs)?
        };
        active.run.apply(RunEvent::Reply)?;
        active.last_error = None;
        Ok(AgentTurn::dispatched(prompt, "resume"))
    }

    /// An interrupted multi-stage build phase, routed by the current stage's
    /// persisted progress: `Building` respawns the build session (or `fix_stage`
    /// when a failed validation report shows that is what died); `Built` /
    /// `Validating` respawn the validation pass (a `Built` stage is forced to
    /// `Validating` first). A `Validated` current stage means the interrupted
    /// session was a post-review change request, whose comments were not
    /// persisted — it cannot be resumed blindly.
    fn resume_run_stage_prompt(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<String, OrchestratorError> {
        let stage_id = active.current_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "multi-stage run is building but has no current stage".to_string(),
            )
        })?;
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let progress_index = active.stage_progress_index(&stage_id).ok_or_else(|| {
            OrchestratorError::Gate(format!("no progress record for stage {stage_id}"))
        })?;
        match active.stages[progress_index].state {
            StageProgressState::Building => {
                let died_in_fix_session = active.stages[progress_index]
                    .validation
                    .as_ref()
                    .is_some_and(|report| !report.passed);
                let template = if died_in_fix_session {
                    &self.templates.fix_stage
                } else {
                    &self.templates.build_stage
                };
                Ok(self.render_run_stage(template, active, plan_stage_docs, doc_index, ""))
            }
            StageProgressState::Built => {
                active.stages[progress_index].apply(StageProgressEvent::StartValidation)?;
                Ok(self.render_run_stage(&self.templates.validate, active, plan_stage_docs, doc_index, ""))
            }
            StageProgressState::Validating => Ok(self.render_run_stage(
                &self.templates.validate,
                active,
                plan_stage_docs,
                doc_index,
                "",
            )),
            StageProgressState::Validated { passed: true } => {
                Err(OrchestratorError::Gate(format!(
                    "stage {stage_id} already passed validation — the interrupted session was a \
                     post-review change request; re-send the diff comments with Request Changes, \
                     or approve the merge"
                )))
            }
            StageProgressState::Validated { passed: false } => {
                Err(OrchestratorError::Gate(format!(
                    "stage {stage_id} failed validation — dispatch a fix session instead of resuming"
                )))
            }
        }
    }

    /// Write Build's ownership into a checkout it is about to adopt: the
    /// checkpoint commit that keeps pre-Build work its own legible commit, then
    /// the `.build/mcp.json` scaffold (left uncommitted). The disk half of an
    /// adoption, and the half that must run with the app mutex released.
    ///
    /// Ordered as the fused dispatch path is, and separated from
    /// [`adopt_run`](Self::adopt_run) so the verdict — which cannot fail once
    /// the checkout is an [`AdoptableCheckout`] — is written down under the
    /// same lock acquisition as everything else it settles.
    pub fn prepare_adoption(
        &self,
        checkout: &AdoptableCheckout,
        base_branch: &str,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.commit_all_with_message(&checkout.path, "Checkpoint: adopted by Build")?;
        self.scaffold_build_dir(&checkout.worktree(base_branch), owner_id)
    }

    /// Mint a plan-less run around a checkout [`prepare_adoption`] has already
    /// written to (the run-side `adopt`; `plan_id` is `None`). No agent session
    /// is spawned — the run lands in `Review` (there is work to review). Pure
    /// bookkeeping: every refusal was spent judging the checkout, and no disk
    /// is touched here.
    ///
    /// [`prepare_adoption`]: Self::prepare_adoption
    pub fn adopt_run(
        &self,
        id: RunId,
        checkout: &AdoptableCheckout,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActiveRun, OrchestratorError> {
        let branch = checkout.branch.clone();
        let worktree = checkout.worktree(base_branch);
        let goal = derive_adoption_goal(&branch, &checkout.head_subject);
        let mut run = Run::new(id, None, goal);
        run.apply(RunEvent::Dispatch)?;
        run.apply(RunEvent::BuildReady)?;

        // Adoption speaks to nobody: it is git and records. The branch starts
        // with no agents, its chat tab shows the new-agent view, and the first
        // thing said to it creates the agent that hears it — on `model_choice`,
        // which is what the adopting caller named.
        Ok(ActiveRun {
            run,
            worktree,
            // Adopted runs baseline their review diff on the merge-base — there
            // is no materialization commit to pin.
            base_sha: None,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: true,
            triage: None,
            recovery: None,
            publication_attempt: None,
            model_choice,
            agents: AgentRoster::empty(),
            last_summary: None,
            last_error: None,
        })
    }

    /// Approve the run's diff and merge (the run-side `approve_merge`). Merge
    /// honesty (contract): the git work runs FIRST — only if commit + merge
    /// succeed does the run become `Merged`; a git failure keeps it in `Review`
    /// with a `merge_failed:` reason. Worktree cleanup is deliberately left to
    /// the caller (after it persists the `Merged` verdict), collapsing the
    /// crash window to a self-healing one.
    pub fn run_approve_merge(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        // Reject up front if the run isn't at a review gate (pure legality).
        run_transition(&active.run.state, RunEvent::ApproveMerge)?;
        self.commit_all(&active.worktree.path, &active.run.goal)
            .map_err(as_merge_failure)?;
        self.merge_into_base(&active.worktree.branch(), &active.worktree.base_branch)
            .map_err(as_merge_failure)?;
        active.run.apply(RunEvent::ApproveMerge)?;
        active.last_error = None;
        Ok(())
    }

    /// Commit any outstanding work on the run branch (the implicit commit step
    /// every finish action shares). Keeps the worktree; no lifecycle change.
    pub fn run_commit(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)
    }

    /// Commit, then push the run branch to its configured remote. Keeps the
    /// worktree, so the agent can keep working / the user can open a PR.
    pub fn run_push(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)?;
        let repo = git2::Repository::discover(&active.worktree.path)
            .map_err(|error| OrchestratorError::Git(error.to_string()))?;
        let remote = configured_remote_for_branch(&repo, &active.worktree.branch())
            .ok_or_else(|| OrchestratorError::Git("no configured push remote".to_string()))?;
        self.git(
            &active.worktree.path,
            // `--` stops option parsing so option-shaped names remain opaque.
            &["push", "-u", &remote, "--", &active.worktree.branch()],
        )?;
        Ok(())
    }

    /// Approve & merge (as [`run_approve_merge`](Self::run_approve_merge)) and
    /// then push the updated base branch to `origin`.
    pub fn run_merge_and_push(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        let base = active.worktree.base_branch.clone();
        self.run_approve_merge(active)?;
        let repo = git2::Repository::open(&self.repo_path)
            .map_err(|error| OrchestratorError::Git(error.to_string()))?;
        let remote = configured_remote_for_branch(&repo, &base)
            .ok_or_else(|| OrchestratorError::Git("no configured push remote".to_string()))?;
        self.git(&self.repo_path, &["push", &remote, &base])?;
        Ok(())
    }

    /// Abandon without touching the checkout — the lifecycle verdict alone.
    /// A run adopted around the PRIMARY checkout ends this way: that directory
    /// is the repository, and [`WorktreeManager::remove`] opens with
    /// `remove_dir_all`.
    pub fn abandon_run_keeping_checkout(
        &self,
        active: &mut ActiveRun,
    ) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::Abandon)?;
        Ok(())
    }

    /// Mid-run stage-doc revision (spec seam #3): re-plan one stage's doc from
    /// the plan's open comments, but run the revision session in the RUN's
    /// worktree (that is where the docs are materialized and where the diff /
    /// PTY live). Only legal at the between-stages gate, where the upcoming
    /// stage's doc is under review. The run's coarse state is untouched — the
    /// revision is a plan-doc operation that merely borrows the run's worktree;
    /// `revising_stage_id` marks it so the resulting `done(revise)` is routed to
    /// [`consume_run_stage_revision`](Self::consume_run_stage_revision) (a store
    /// write-back) rather than through [`on_run_done`](Self::on_run_done).
    pub fn send_run_stage_notes(
        &self,
        active: &mut ActiveRun,
        plan: &ActivePlan,
        stage_id: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        if active.run.state != RunState::StageGate {
            return Err(OrchestratorError::Gate(format!(
                "stage-doc revisions run from the stage gate (run is {:?})",
                active.run.state
            )));
        }
        let doc_index = plan
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        let open = plan.open_comments_for(stage_id);
        if open.is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "no open comments on stage {stage_id}"
            )));
        }
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        let prompt = self.render_run_stage(
            &self.templates.revise_stage,
            active,
            &plan.stages,
            doc_index,
            THREAD_NOTIFICATION,
        );
        Ok(AgentTurn::posted(prompt, THREAD_NOTIFICATION, "revise"))
    }

    /// Consume a mid-run stage-doc revision's `done(revise)`: ingest the revised
    /// docs from the run's worktree back into the canonical store (fail-fast —
    /// the revision is never accepted with unpersisted docs), reset the plan's
    /// stage-doc state (a revised doc's approval is stale), and land the agent's
    /// per-comment resolutions on the plan's comments. Cross-entity by design:
    /// the caller hands both the run (whose worktree holds the docs) and the
    /// owning plan (whose store id, doc state, and comments are updated). The
    /// run's coarse state is untouched.
    pub fn consume_run_stage_revision(
        &self,
        active: &mut ActiveRun,
        plan: &mut ActivePlan,
        store: &Store,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        let stage_id = active.revising_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "revise report for a run with no stage revision in flight".to_string(),
            )
        })?;
        let index = plan
            .stage_doc_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        // Probe the doc transition before any mutation.
        stage_doc_transition(&plan.stages[index].state, StageDocEvent::Revised)?;
        if let Err(ingest_error) =
            store.ingest_plan_docs(&plan.plan.id.0, &active.worktree.path, &plan.plan_path)
        {
            active.last_error = Some(format!("stage revision not persisted: {ingest_error}"));
            return Err(OrchestratorError::Store(ingest_error));
        }
        plan.stages[index].state =
            stage_doc_transition(&plan.stages[index].state, StageDocEvent::Revised)?;
        if let Some(resolutions) = &report.outputs.comment_resolutions {
            for resolution in resolutions {
                let answers_this_stage = plan
                    .open_comments_for(&stage_id)
                    .iter()
                    .any(|comment| comment.id == resolution.comment_id);
                if !answers_this_stage
                    || !plan
                        .agents
                        .sole_thread_mut()
                        .resolve_doc_comment(&resolution.comment_id, &resolution.response)
                {
                    eprintln!(
                        "run stage revision for {stage_id}: unknown or non-open comment {:?}; \
                         skipping",
                        resolution.comment_id
                    );
                }
            }
        }
        active.revising_stage_id = None;
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }

    /// Recreate a missing native implementation checkout from its exact
    /// persisted branch, using the verified local ref first and origin second.
    pub fn restore_run_worktree(
        &self,
        worktree: &Worktree,
        when_unregistered: crate::worktree::UnregisteredRestore,
    ) -> Result<Worktree, OrchestratorError> {
        Ok(self.worktrees.restore(worktree, when_unregistered)?)
    }

    /// Best-effort teardown of a leftover checkout: the directory always, and
    /// the branch under it unless the caller is handing that back. A failed
    /// cleanup is logged, never fatal — a stray worktree is only clutter, and
    /// what removes a card is the record, not the directory.
    ///
    /// `keep_branch` is the caller's own fact and never derivable here: a run's
    /// work outlives an abandon so it can be re-attempted, and a dispatch that
    /// checked out a branch somebody else made must hand that branch back
    /// whole.
    pub fn discard_checkout(&self, worktree: &Worktree, keep_branch: bool) {
        let removed = if keep_branch {
            self.worktrees.remove_keeping_branch(worktree)
        } else {
            self.worktrees.remove(worktree)
        };
        if let Err(e) = removed {
            eprintln!(
                "discard_checkout {} (keep_branch {keep_branch}): {e}",
                worktree.name
            );
        }
    }

    // --- internals -------------------------------------------------------------

    fn render_plan(&self, template: &str, active: &ActivePlan, comments: &str) -> String {
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                docs_dir: &plan_docs_dir_display(active),
                comments,
                base_branch: &active.base_branch,
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, &active.stages, |_| "not started".to_string())
    }

    fn render_run(
        &self,
        template: &str,
        active: &ActiveRun,
        comments: &str,
        plan_stage_docs: &[StageDoc],
    ) -> String {
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.run.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, plan_stage_docs, |stage_id| {
            active
                .stage_progress(stage_id)
                .map(|progress| format!("{:?}", progress.state))
                .unwrap_or_else(|| "not started".to_string())
        })
    }

    /// Render a stage-scoped template for a run with the full stage variable
    /// set: the stage doc's own fields (from the plan's manifest), the next
    /// stage's doc path (empty on the final stage), the previous stage's
    /// validation notes, and this stage's own findings — the split twin of
    /// [`render_stage`](Self::render_stage), joining plan docs to run progress
    /// by stage id.
    fn render_run_stage(
        &self,
        template: &str,
        active: &ActiveRun,
        plan_stage_docs: &[StageDoc],
        doc_index: usize,
        comments: &str,
    ) -> String {
        let doc = &plan_stage_docs[doc_index];
        let next_stage_path = plan_stage_docs
            .get(doc_index + 1)
            .map(|next| next.path.as_str())
            .unwrap_or("");
        let prior_notes = doc_index
            .checked_sub(1)
            .and_then(|previous| active.stage_progress(&plan_stage_docs[previous].id))
            .and_then(|progress| progress.validation.as_ref())
            .map(|v| v.notes_for_next_stage.as_str())
            .unwrap_or("");
        let progress = active.stage_progress(&doc.id);
        let stage_start_sha = progress.and_then(|p| p.start_sha.as_deref()).unwrap_or("");
        let findings = progress
            .and_then(|p| p.validation.as_ref())
            .map(|v| v.findings.as_str())
            .unwrap_or("");
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.run.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                stage_id: &doc.id,
                stage_title: &doc.title,
                stage_path: &doc.path,
                stage_summary: &doc.summary,
                next_stage_path,
                stage_start_sha,
                findings,
                prior_notes,
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, plan_stage_docs, |stage_id| {
            active
                .stage_progress(stage_id)
                .map(|progress| match progress.state {
                    StageProgressState::Building => "building",
                    StageProgressState::Built => "built",
                    StageProgressState::Validating => "validating",
                    StageProgressState::Validated { passed: true } => "complete",
                    StageProgressState::Validated { passed: false } => "validation failed",
                })
                .unwrap_or("not started")
                .to_string()
        })
    }

    /// Write the per-entity MCP config under `.build/` so it never trips
    /// plan-scope enforcement, pointing the harness at the owning entity's
    /// `done` server. `owner_id` is a plan, run, or (legacy) task id — the MCP
    /// CLI stays `mcp --task <id>` (opaque); the daemon routes each report by
    /// owner lookup.
    fn scaffold_build_dir(
        &self,
        worktree: &Worktree,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.launch
            .scaffold_agent_worktree(&worktree.path, owner_id)
    }

    fn commit_all(&self, worktree_path: &Path, goal: &str) -> Result<(), OrchestratorError> {
        self.commit_all_with_message(worktree_path, &format!("Build: {goal}"))
    }

    /// Stage everything and commit with `message` verbatim; a clean tree is a
    /// no-op (nothing staged, nothing committed).
    fn commit_all_with_message(
        &self,
        worktree_path: &Path,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        // The scaffolded MCP config is machine-local plumbing (absolute binary
        // path, per-task identity): committing it would merge it into the base
        // branch and add/add-conflict against every other branch's copy. It is
        // kept out of every commit by `.build/.gitignore` (written at scaffold
        // time), which `git add -A` honors silently — and which also guards the
        // agent's own commits. (A `:(exclude)` pathspec here would instead ERROR,
        // since it names an ignored path explicitly.)
        self.git(worktree_path, &["add", "-A", "--", "."])?;
        // Only commit if something is staged (the MCP config alone must not
        // produce a commit).
        let staged = self.git(worktree_path, &["diff", "--cached", "--name-only"])?;
        if !staged.trim().is_empty() {
            self.git(worktree_path, &["commit", "-m", message])?;
        }
        Ok(())
    }

    /// Merge the task branch into `base_branch` via the primary checkout. The
    /// primary repo is the user's live checkout, so first verify it actually has
    /// the base branch checked out — merging into whatever happens to be at HEAD
    /// would land the task on the wrong branch (and a later push of the base
    /// branch would silently publish nothing).
    fn merge_into_base(&self, branch: &str, base_branch: &str) -> Result<(), OrchestratorError> {
        let head = self
            .git(&self.repo_path, &["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_string();
        if head != base_branch {
            return Err(OrchestratorError::Git(format!(
                "primary checkout is on {head:?}, not the base branch {base_branch:?} — \
                 check out {base_branch:?} (or commit/stash your work) and approve again"
            )));
        }
        // `--` stops option parsing so an option-shaped branch name can never be
        // read by git as a flag (defense in depth alongside the adopt-time guard).
        if let Err(merge_error) = self.git(&self.repo_path, &["merge", "--no-edit", "--", branch]) {
            // A conflict leaves the primary checkout wedged mid-merge; abort it so
            // the checkout returns to a clean base and later merges aren't poisoned.
            // Best-effort — the merge failure is the error we surface either way.
            if let Err(abort_error) = self.git(&self.repo_path, &["merge", "--abort"]) {
                eprintln!(
                    "merge_into_base {branch}: merge failed and abort also failed: {abort_error}"
                );
            }
            return Err(merge_error);
        }
        Ok(())
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, OrchestratorError> {
        run_git(dir, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::DoneOutputs;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@build.ing"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    /// A warm "harness" that stays alive and drains stdin (it discards the
    /// prompt), like a real interactive CLI. Draining matters: a child that never
    /// reads lets the PTY's canonical-mode input queue fill, so writing a
    /// full-size rendered prompt would block and then fail with EIO. The startup
    /// byte matters too: like a real TUI painting its screen, it satisfies the
    /// spawn's readiness wait so dispatches don't idle out the grace. The test
    /// plays the agent: it writes files and forwards `done` reports.
    fn warm_harness() -> HarnessSpec {
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf '\\033[?2004h'; cat >/dev/null")
    }

    fn orchestrator(dir: &tempfile::TempDir, repo: &Path) -> Orchestrator {
        Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::Warm(warm_harness()),
            Templates::default(),
            std::fs::canonicalize(repo.join("README.md")).unwrap(),
        )
    }

    /// `.git/info/exclude` is the human's own file, and every planning
    /// workspace of the same project appends Build's two rules to it with the
    /// app mutex released — so two Issues planned at once are two writers.
    /// A reader must see the file whole at every instant, and the human's own
    /// rules must be there, once, when the writers are done.
    #[test]
    fn concurrent_planning_workspaces_never_shorten_the_humans_exclude_file() {
        let (dir, repo) = init_repo();
        let orch = std::sync::Arc::new(orchestrator(&dir, &repo));
        let info = repo.join(".git").join("info");
        std::fs::create_dir_all(&info).unwrap();
        let exclude = info.join("exclude");
        let human_rules: String = (0..4096)
            .map(|i| format!("scratch/notes-{i:04}.md\n"))
            .collect();
        std::fs::write(&exclude, &human_rules).unwrap();
        let expected = format!(
            "{human_rules}# Build's machine-local agent plumbing\n.build/mcp*.json\n.build/attachments/\n"
        );

        let writers: Vec<_> = (0..8)
            .map(|i| {
                let orch = std::sync::Arc::clone(&orch);
                let repo = repo.clone();
                std::thread::spawn(move || {
                    orch.launch
                        .write_build_dir(&repo, &format!("plan-{i}"))
                        .unwrap();
                })
            })
            .collect();
        let torn = {
            let exclude = exclude.clone();
            let human_rules = human_rules.clone();
            let expected = expected.clone();
            std::thread::spawn(move || {
                let mut torn = Vec::new();
                for _ in 0..2000 {
                    let seen = std::fs::read_to_string(&exclude).unwrap();
                    if seen != human_rules && seen != expected {
                        torn.push(seen.len());
                    }
                }
                torn
            })
        };
        for writer in writers {
            writer.join().unwrap();
        }
        let torn = torn.join().unwrap();

        assert!(
            torn.is_empty(),
            "a reader saw the exclude file part-written, at these lengths: {torn:?}"
        );
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), expected);
        let leftovers: Vec<_> = std::fs::read_dir(&info)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "exclude")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left beside exclude: {leftovers:?}"
        );
    }

    /// A checkout whose directory a human already removed still says whose
    /// branch it is: the answer lives beside the registration in the main
    /// repository, not behind the pointer in the missing directory. Reading it
    /// through the pointer left the registration and the branch behind.
    #[test]
    fn discarding_a_checkout_whose_directory_is_gone_still_takes_its_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let worktree = orch
            .create_bare_worktree("vanished", "main")
            .unwrap()
            .worktree;
        std::fs::remove_dir_all(&worktree.path).unwrap();

        orch.discard_checkout(&worktree, false);

        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_worktree("vanished")
                .err()
                .map(|error| error.code() == git2::ErrorCode::NotFound)
                .unwrap_or(false),
            "the stale registration is pruned"
        );
        assert!(
            r.find_branch("build/vanished", git2::BranchType::Local)
                .is_err(),
            "the branch Build cut goes with it"
        );
    }

    #[test]
    fn agent_launch_prepares_scaffolding_spec_and_pty_size_as_one_value() {
        let (dir, repo) = init_repo();
        let worktree = dir.path().join("prepared-worktree");
        std::fs::create_dir(&worktree).unwrap();
        let agent = Agent::WarmBuilder(std::sync::Arc::new(|_, _, options| {
            assert!(
                options
                    .cwd
                    .join(mcp_config_path(&options.owner_id))
                    .exists(),
                "the scaffold must exist before the fallible harness builder runs"
            );
            Ok(HarnessSpec::new("prepared-harness"))
        }));
        let orchestrator = Orchestrator::new(
            repo.clone(),
            dir.path().join("worktrees"),
            agent,
            Templates::default(),
            std::fs::canonicalize(repo.join("README.md")).unwrap(),
        );

        let prepared = orchestrator
            .agent_launch()
            .prepare(
                "agent-prepared",
                &worktree,
                &ModelChoice::default(),
                false,
                None,
                "token",
            )
            .unwrap();

        assert_eq!(prepared.spec.binary, "prepared-harness");
        assert_eq!(prepared.pty_size.rows, 40);
        assert_eq!(prepared.pty_size.cols, 120);
    }

    #[test]
    fn prepared_agent_launch_preserves_setup_errors() {
        let (dir, repo) = init_repo();
        let worktree = dir.path().join("failed-worktree");
        std::fs::create_dir(&worktree).unwrap();
        let agent = Agent::WarmBuilder(std::sync::Arc::new(|_, _, _| {
            Err(HarnessError::Setup("injected Pi setup failure".to_string()))
        }));
        let orchestrator = Orchestrator::new(
            repo.clone(),
            dir.path().join("worktrees"),
            agent,
            Templates::default(),
            std::fs::canonicalize(repo.join("README.md")).unwrap(),
        );
        let error = orchestrator
            .agent_launch()
            .prepare(
                "agent-fail",
                &worktree,
                &ModelChoice::default(),
                false,
                None,
                "token",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestratorError::Harness(HarnessError::Setup(message))
                if message == "injected Pi setup failure"
        ));
    }

    fn done(phase: DonePhase, status: DoneStatus, plan_path: Option<&str>) -> DoneReport {
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

    // ---- Worktree adoption ----

    use crate::worktree::discover_external_worktrees;

    /// Create a user worktree at `dir/<name>` on a new `branch` (cut from the
    /// primary HEAD) and return its discovered summary — the same shape the
    /// app layer resolves a `worktree_id` to.
    fn user_worktree(
        dir: &tempfile::TempDir,
        repo: &Path,
        name: &str,
        branch: &str,
    ) -> ExternalWorktree {
        let path = dir.path().join(name);
        assert!(Command::new("git")
            .args(["worktree", "add", "-b", branch, path.to_str().unwrap()])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        discover_external_worktrees(repo, "main", &std::collections::HashSet::new())
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .expect("the new worktree is discoverable")
    }

    fn worktree_head(worktree: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // ---- Multi-stage: lifecycle ----

    use crate::plan::StageManifestEntry;
    use crate::run::ValidationReport;

    fn manifest_entry(id: &str, title: &str, position: usize) -> StageManifestEntry {
        StageManifestEntry {
            id: id.into(),
            title: title.into(),
            path: format!(".build/plan/{position:02}-{id}.md"),
            summary: format!("{title}."),
        }
    }

    fn done_plan_stages(entries: Vec<StageManifestEntry>) -> DoneReport {
        DoneReport {
            phase: DonePhase::Plan,
            status: DoneStatus::Completed,
            summary: "planned".into(),
            outputs: DoneOutputs {
                plan_path: Some(templates::STAGES_MANIFEST_PATH.to_string()),
                stages: Some(entries),
                ..DoneOutputs::default()
            },
        }
    }

    fn done_validate(passed: bool, findings: &str, notes: &str) -> DoneReport {
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

    fn last_commit_subject(worktree: &Path) -> String {
        let out = Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // ---- Multi-stage: per-stage revision, fix sessions, and resume routing ----

    // ---- Multi-stage: ActiveTask bookkeeping ----

    // ---- Plan/Run split seams ----

    use crate::plan::{PlanState, StageDocState};
    use crate::run::{RunId, RunState, StageProgressState};
    use crate::store::{PersistedPlan, PersistedRun, Store};

    fn split_store(dir: &tempfile::TempDir) -> Store {
        Store::new(dir.path().join("store")).expect("store opens")
    }

    /// Leave a reviewer comment on one stage. A comment is a post on the
    /// Issue's conversation and nowhere else, so this is how a test makes one.
    /// Returns the comment's id.
    fn comment_on(plan: &mut ActivePlan, stage_id: &str) -> String {
        let path = plan
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.path.clone())
            .unwrap_or_default();
        let issue_id = plan.plan.id.0.clone();
        plan.agents.primary_mut().unwrap().thread.post_doc_comment(
            &issue_id,
            stage_id,
            &path,
            None,
            format!("comment on {stage_id}"),
            "2026-08-13T09:00:00Z",
        )
    }

    /// One comment as the conversation now holds it.
    fn comment_by_id(plan: &ActivePlan, comment_id: &str) -> crate::thread::DocComment {
        plan.agents
            .sole_thread()
            .doc_comments()
            .into_iter()
            .find(|comment| comment.id == comment_id)
            .unwrap_or_else(|| panic!("no comment {comment_id}"))
    }

    /// The plan's live scratch docs dir (panics once the workspace is gone).
    fn plan_docs_dir(plan: &ActivePlan) -> PathBuf {
        plan.workspace
            .as_ref()
            .expect("plan has a live planning workspace")
            .docs_dir
            .clone()
    }

    fn drafting_plan(orch: &Orchestrator, store: &Store, id: &str, goal: &str) -> ActivePlan {
        drafting_plan_and_turn(orch, store, id, goal).0
    }

    /// Assert both halves of the cold/warm rule on a DISPATCHED turn (one whose
    /// whole content is the rendered prompt: a dispatch, a resume, a validation
    /// hand-off, a stage fix), and hand back the warm half to assert content on.
    ///
    /// Both halves matter equally: the caller cannot know which one will travel
    /// — that depends on whether it had to spawn a harness — so a turn that
    /// renders only the half a test happens to look at reaches the other kind of
    /// agent with nothing.
    fn dispatch_turn_halves(turn: &AgentTurn, phase: &str) -> String {
        assert_eq!(turn.phase, phase, "turn phase: {turn:?}");
        assert!(
            !turn.warm.is_empty(),
            "a turn with nothing to say: {turn:?}"
        );
        assert!(
            !turn.warm.contains("Build conversation protocol"),
            "an agent already in the conversation is not re-taught the protocol: {}",
            turn.warm
        );
        assert!(
            turn.cold.starts_with(&turn.warm),
            "cold is the warm instruction plus the conversation it missed — cold {:?}, warm {:?}",
            turn.cold,
            turn.warm
        );
        assert!(
            turn.cold.contains("Build conversation protocol"),
            "a spawned agent gets the protocol: {}",
            turn.cold
        );
        turn.warm.clone()
    }

    /// Assert both halves of the cold/warm rule on a POSTED turn (a change
    /// request, a message, a batch of notes): the payload is already durable on
    /// the thread, so a warm agent hears only `nudge` while a cold one gets the
    /// same instruction wrapped in the run/plan context it cannot reconstruct.
    fn posted_turn_halves(turn: &AgentTurn, phase: &str, nudge: &str) -> String {
        assert_eq!(turn.phase, phase, "turn phase: {turn:?}");
        assert_eq!(
            turn.warm, nudge,
            "an agent already in the conversation hears the instruction alone"
        );
        assert!(
            turn.cold.contains(nudge),
            "the instruction travels cold too: {}",
            turn.cold
        );
        assert!(
            turn.cold.contains("Build conversation protocol"),
            "a spawned agent gets the protocol: {}",
            turn.cold
        );
        turn.cold.clone()
    }

    /// A door to an Issue's planning agent, driven the way the app drives it:
    /// gate it, prepare the workspace (the app does that with its mutex
    /// released), open the session. One helper per door, so a test says which
    /// door it is knocking on and nothing else has to know the order.
    fn start_plan_drafting(
        orch: &Orchestrator,
        active: &mut ActivePlan,
        store: &Store,
    ) -> Result<AgentTurn, OrchestratorError> {
        let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
        orch.open_plan_drafting(active, workspace)
    }

    fn send_plan_notes(
        orch: &Orchestrator,
        active: &mut ActivePlan,
        store: &Store,
        notes: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::SendNotes)?;
        let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
        orch.open_plan_notes(active, workspace, notes)
    }

    fn send_plan_stage_notes(
        orch: &Orchestrator,
        active: &mut ActivePlan,
        store: &Store,
        stage_id: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        gate_plan_stage_notes(active, stage_id)?;
        let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
        orch.open_plan_stage_notes(active, workspace, stage_id)
    }

    fn message_plan(
        orch: &Orchestrator,
        active: &mut ActivePlan,
        store: &Store,
        message: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        gate_plan_message(active, message)?;
        let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
        orch.open_plan_message(active, workspace, message)
    }

    fn resume_plan(
        orch: &Orchestrator,
        active: &mut ActivePlan,
        store: &Store,
    ) -> Result<AgentTurn, OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::Reply)?;
        let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
        orch.open_plan_resume(active, workspace)
    }

    /// A dispatched plan plus the turn the dispatch wants said to its agent —
    /// the orchestrator's whole output now that it owns no process.
    fn drafting_plan_and_turn(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> (ActivePlan, AgentTurn) {
        let mut active = orch.create_plan(PlanId::new(id), goal, "main", Default::default());
        let turn = start_plan_drafting(orch, &mut active, store).unwrap();
        (active, turn)
    }

    /// Play the plan agent: write a single plan doc and report done, landing
    /// the plan at PlanReview with its docs ingested into the store.
    fn plan_in_review(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
        plan_in_review_with_goal(orch, store, id, "Add a greeting")
    }

    fn plan_in_review_with_goal(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActivePlan {
        let mut plan = drafting_plan(orch, store, id, goal);
        let worktree_path = plan_docs_dir(&plan);
        std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v1\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        plan
    }

    fn approved_plan(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
        approved_plan_with_goal(orch, store, id, "Add a greeting")
    }

    fn approved_plan_with_goal(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActivePlan {
        let mut plan = plan_in_review_with_goal(orch, store, id, goal);
        orch.approve_plan(&mut plan).unwrap();
        plan
    }

    /// A plan whose agent produced `stage_count` stage docs plus the manifest,
    /// driven to PlanReview (the docs are ingested into the store).
    fn multi_stage_plan_in_review(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        stage_count: usize,
    ) -> ActivePlan {
        let titles = ["First", "Second", "Third"];
        let mut plan = drafting_plan(orch, store, id, "Add greetings");
        let plan_dir = plan_docs_dir(&plan).join(".build/plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let mut entries = Vec::new();
        for (position, title) in titles.iter().take(stage_count).enumerate() {
            let stage_id = title.to_lowercase();
            std::fs::write(
                plan_dir.join(format!("{:02}-{stage_id}.md", position + 1)),
                format!("# Stage: {title}\n"),
            )
            .unwrap();
            entries.push(manifest_entry(&stage_id, title, position + 1));
        }
        std::fs::write(plan_dir.join("stages.json"), "[]").unwrap();
        orch.on_plan_done(&mut plan, store, done_plan_stages(entries))
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        plan
    }

    fn approved_multi_stage_plan(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        stage_count: usize,
    ) -> ActivePlan {
        let mut plan = multi_stage_plan_in_review(orch, store, id, stage_count);
        let stage_ids: Vec<String> = plan.stages.iter().map(|s| s.id.clone()).collect();
        for stage_id in stage_ids {
            orch.approve_plan_stage(&mut plan, &stage_id).unwrap();
        }
        orch.approve_plan(&mut plan).unwrap();
        plan
    }

    fn dispatch_planned_run(
        orch: &Orchestrator,
        store: &Store,
        plan: &ActivePlan,
        id: &str,
    ) -> ActiveRun {
        dispatch_planned_run_and_turn(orch, store, plan, id).0
    }

    /// A dispatched run plus the turn the dispatch wants said to its agent.
    fn dispatch_planned_run_and_turn(
        orch: &Orchestrator,
        store: &Store,
        plan: &ActivePlan,
        id: &str,
    ) -> (ActiveRun, AgentTurn) {
        let issue = ImplementableIssue::judge(RunSource {
            plan,
            has_active_run: false,
        })
        .unwrap();
        let prepared = orch
            .prepare_run_checkout(&issue, "main", id, store)
            .unwrap();
        orch.open_prepared_run(RunId::new(id), plan, prepared, Default::default())
            .unwrap()
    }

    /// A run implementing a single-doc plan: one build session, no stage
    /// pipeline. This is what the retired goal-only dispatch used to stand in
    /// for, so it is the fixture for every run-side test that only needs "a run
    /// the agent is building in". The plan id derives from the run id, so
    /// repeated calls inside one test never collide.
    fn dispatch_single_stage_run(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActiveRun {
        let plan = approved_plan_with_goal(orch, store, &format!("plan-of-{id}"), goal);
        dispatch_planned_run(orch, store, &plan, id)
    }

    /// Every checkout git knows about for this repo, primary first.
    fn registered_checkouts(repo: &Path) -> Vec<String> {
        let out = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("worktree ").map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn a_drafting_plan_runs_on_the_primary_checkout_and_cuts_no_worktree() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let workspace = plan.workspace.as_ref().expect("a planning workspace");
        assert_eq!(
            workspace.checkout, repo,
            "an issue's planning agent works in the project's primary checkout"
        );
        assert_eq!(
            registered_checkouts(&repo).len(),
            1,
            "planning cuts no worktree: {:?}",
            registered_checkouts(&repo)
        );
        assert!(
            !workspace.docs_dir.starts_with(&repo),
            "the scratch docs dir lives outside the repo: {}",
            workspace.docs_dir.display()
        );
        assert!(
            workspace.docs_dir.is_dir(),
            "the agent has a docs dir to write into: {}",
            workspace.docs_dir.display()
        );

        // The scaffolded MCP config routes `done` reports back to THIS plan,
        // and it lands in the checkout the agent actually runs in.
        let mcp = std::fs::read_to_string(repo.join(mcp_config_path("plan-1"))).unwrap();
        assert!(mcp.contains("plan-1"), "{mcp}");
    }

    /// Planning runs in the human's own checkout, so it must leave no trace
    /// there: nothing to commit, and — critically — no untracked
    /// `.build/.gitignore`, which would refuse to be overwritten by the merge
    /// of any branch that carries one. The rules live in the repo-local
    /// exclude file instead.
    #[tokio::test]
    async fn planning_leaves_the_primary_checkout_clean() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let _first = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

        assert!(
            !repo.join(".build/.gitignore").exists(),
            "an ignore file here would block every later merge"
        );
        let exclude_path = repo.join(".git/info/exclude");
        let exclude = std::fs::read_to_string(&exclude_path).unwrap();
        assert!(exclude.contains(".build/mcp*.json"), "{exclude}");
        assert!(exclude.contains(".build/attachments/"), "{exclude}");
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "",
            "planning dirties nothing in the primary checkout"
        );

        // A second issue writes its own config and repeats no rule.
        let _second = drafting_plan(&orch, &store, "plan-2", "Add a farewell");
        let exclude = std::fs::read_to_string(&exclude_path).unwrap();
        assert_eq!(
            exclude
                .lines()
                .filter(|line| line.trim() == ".build/mcp*.json")
                .count(),
            1,
            "the exclude rules are written once: {exclude}"
        );
        assert!(repo.join(mcp_config_path("plan-2")).exists());
    }

    #[tokio::test]
    async fn plan_done_ingests_docs_into_the_store_before_plan_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let plan = plan_in_review(&orch, &store, "plan-1");
        assert_eq!(plan.plan_path, ".build/plan.md");
        assert_eq!(plan.last_summary.as_deref(), Some("summary"));
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n"),
            "the store copy is canonical the moment the gate opens"
        );
    }

    #[tokio::test]
    async fn plan_done_ingest_failure_keeps_the_plan_drafting_with_the_error_surfaced() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

        // The agent reports done but wrote NO docs: the ingest is transactional,
        // so the done errors and the plan never advances with unpersisted docs.
        let err = orch
            .on_plan_done(
                &mut plan,
                &store,
                done(
                    DonePhase::Plan,
                    DoneStatus::Completed,
                    Some(".build/plan.md"),
                ),
            )
            .expect_err("ingest failure fails the done");
        assert!(matches!(err, OrchestratorError::Store(_)), "{err}");
        assert_eq!(plan.plan.state, PlanState::Drafting, "no state advance");
        assert!(
            plan.last_error
                .as_deref()
                .is_some_and(|e| e.contains("not persisted")),
            "{:?}",
            plan.last_error
        );
        assert_eq!(store.read_plan_doc("plan-1", ".build/plan.md"), None);
    }

    #[tokio::test]
    async fn plan_blocked_and_failed_reports_park_the_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut blocked = drafting_plan(&orch, &store, "plan-b", "goal b");
        orch.on_plan_done(
            &mut blocked,
            &store,
            done(DonePhase::Plan, DoneStatus::Blocked, None),
        )
        .unwrap();
        assert_eq!(blocked.plan.state, PlanState::Blocked);
        assert_eq!(blocked.last_summary.as_deref(), Some("summary"));

        let mut failed = drafting_plan(&orch, &store, "plan-f", "goal f");
        orch.on_plan_done(
            &mut failed,
            &store,
            done(DonePhase::Plan, DoneStatus::Failed, None),
        )
        .unwrap();
        assert_eq!(failed.plan.state, PlanState::Failed);
    }

    #[tokio::test]
    async fn plan_idle_then_late_done_is_still_honored() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

        orch.on_plan_idle(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::IdleUnreported);

        // Quiescence never decided anything: the late report still lands.
        std::fs::write(plan_docs_dir(&plan).join(".build/plan.md"), "# Late plan\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Late plan\n")
        );
    }

    #[tokio::test]
    async fn build_or_validate_reports_on_a_plan_are_rejected_without_mutation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

        for phase in [DonePhase::Build, DonePhase::Validate] {
            let err = orch
                .on_plan_done(&mut plan, &store, done(phase, DoneStatus::Completed, None))
                .expect_err("plans only accept plan/revise reports");
            assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
        }
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(plan.last_summary, None, "a rejected report leaves no trace");
    }

    #[tokio::test]
    async fn plan_done_merges_the_manifest_into_stage_docs() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        assert_eq!(plan.plan_path, templates::STAGES_MANIFEST_PATH);
        let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second"]);
        assert!(plan
            .stages
            .iter()
            .all(|s| s.state == StageDocState::Planned));
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First\n")
        );

        // A re-plan retitles "second" (doc state kept), drops "first", and
        // appends "third" — the plan side only carries doc review, so a
        // dropped id simply disappears (run progress is never deleted).
        plan.stages[1].state = StageDocState::Approved;
        send_plan_notes(&orch, &mut plan, &store, "restructure").unwrap();
        std::fs::write(
            plan_docs_dir(&plan).join(".build/plan/03-third.md"),
            "# Stage: Third\n",
        )
        .unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done_plan_stages(vec![
                manifest_entry("second", "Second v2", 2),
                manifest_entry("third", "Third", 3),
            ]),
        )
        .unwrap();
        let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["second", "third"]);
        assert_eq!(plan.stages[0].title, "Second v2");
        assert_eq!(
            plan.stages[0].state,
            StageDocState::Approved,
            "an existing id keeps its review sub-state across a re-plan"
        );
        assert_eq!(plan.stages[1].state, StageDocState::Planned);
    }

    #[tokio::test]
    async fn send_plan_notes_revises_in_the_warm_worktree() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let worktree_path = plan_docs_dir(&plan);

        let turn = send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(
            plan_docs_dir(&plan),
            worktree_path,
            "the worktree stays warm through the notes loop"
        );
        // The notes are a turn for the agent already drafting in that worktree,
        // not a prompt for a replacement.
        let cold = posted_turn_halves(&turn, "revise", "tighten step 2");
        assert!(
            cold.contains(".build/plan.md"),
            "a cold agent is pointed at the doc it must revise: {cold}"
        );

        // The revised doc lands in the store on the next done.
        std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v2\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v2\n")
        );
    }

    #[tokio::test]
    async fn send_plan_notes_refills_a_vanished_docs_dir_from_the_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // The scratch docs dir was deleted out from under the plan. The docs
        // are canonical in the store, so a revision just refills it.
        let docs_dir = plan_docs_dir(&plan);
        std::fs::remove_dir_all(&docs_dir).unwrap();
        send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(plan_docs_dir(&plan), docs_dir, "the same docs dir");
        assert_eq!(
            std::fs::read_to_string(docs_dir.join(".build/plan.md")).unwrap(),
            "# Plan v1\n",
            "docs re-materialized from the store before the turn is delivered"
        );
    }

    #[tokio::test]
    async fn send_plan_notes_remakes_a_dropped_workspace_from_the_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // Simulate the interrupted arm where the workspace is gone entirely
        // (a plan reattached after a restart holds none).
        let docs_dir = plan_docs_dir(&plan);
        std::fs::remove_dir_all(&docs_dir).unwrap();
        plan.workspace = None;
        send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let workspace = plan.workspace.as_ref().expect("a workspace was remade");
        assert_eq!(workspace.checkout, repo, "still the primary checkout");
        assert_eq!(
            std::fs::read_to_string(workspace.docs_dir.join(".build/plan.md")).unwrap(),
            "# Plan v1\n"
        );
        // The checkout is fully scaffolded (done reports must route).
        assert!(repo.join(mcp_config_path("plan-1")).exists());
    }

    /// A revision in flight is never overwritten: the docs dir the agent is
    /// working in is left exactly as the agent left it.
    #[tokio::test]
    async fn send_plan_notes_leaves_a_live_docs_dir_alone() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        let docs_dir = plan_docs_dir(&plan);
        std::fs::write(docs_dir.join(".build/plan.md"), "# Plan being revised\n").unwrap();
        send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();

        assert_eq!(
            std::fs::read_to_string(docs_dir.join(".build/plan.md")).unwrap(),
            "# Plan being revised\n",
            "the store copy must not clobber the draft in flight"
        );
    }

    #[tokio::test]
    async fn approve_plan_drops_the_scratch_docs_dir() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let docs_dir = plan_docs_dir(&plan);

        orch.approve_plan(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::Approved);
        assert_eq!(plan.workspace, None, "the workspace is gone");
        assert!(!docs_dir.exists(), "the scratch docs are gone with it");
        assert_eq!(
            registered_checkouts(&repo).len(),
            1,
            "planning never had a worktree to tear down"
        );
        // The canonical docs survive.
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n")
        );
    }

    /// The reviewer-facing bug this guards: a planning checkout cleaned up
    /// outside Build made "Mark issue ready" fail, because removing something
    /// already absent was read as a failure. Approve only wants the scratch
    /// docs gone — and they are.
    #[tokio::test]
    async fn approve_plan_succeeds_when_the_docs_dir_already_vanished() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // Cleaned up behind the plan's back.
        std::fs::remove_dir_all(plan_docs_dir(&plan)).unwrap();

        orch.approve_plan(&mut plan)
            .expect("an already-gone docs dir is the goal, not a failure");
        assert_eq!(plan.plan.state, PlanState::Approved);
        assert!(plan.workspace.is_none(), "the plan lets the carcass go");
    }

    #[tokio::test]
    async fn an_implementable_issue_rejects_one_whose_first_stage_doc_is_unapproved() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        // A migrated plan can rest at Approved while a stage doc is Planned
        // (legacy records never re-gate); dispatch must still hold the line.
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Planned;

        let Err(error) = ImplementableIssue::judge(RunSource {
            plan: &plan,
            has_active_run: false,
        }) else {
            panic!("stage 0 must be approved before its build session spawns");
        };
        assert!(
            error.to_string().contains("first"),
            "the gate names the unapproved stage: {error}"
        );
    }

    #[tokio::test]
    async fn plan_stage_revision_done_ingests_resolves_comments_and_resets_approval() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Approved;
        let first_comment = comment_on(&mut plan, "first");
        let second_comment = comment_on(&mut plan, "first");

        // A stage-revision session is in flight for "first" (the dispatching
        // verb lands with the periphery; the state is arranged directly to
        // isolate the done handling).
        plan.plan.apply(crate::plan::PlanEvent::SendNotes).unwrap();
        plan.revising_stage_id = Some("first".into());
        std::fs::write(
            plan_docs_dir(&plan).join(".build/plan/01-first.md"),
            "# Stage: First (revised)\n",
        )
        .unwrap();

        orch.on_plan_done(
            &mut plan,
            &store,
            DoneReport {
                phase: DonePhase::Revise,
                status: DoneStatus::Completed,
                summary: "revised".into(),
                outputs: DoneOutputs {
                    comment_resolutions: Some(vec![
                        crate::mcp::CommentResolution {
                            comment_id: first_comment.clone(),
                            response: "switched to a timestamp".into(),
                        },
                        crate::mcp::CommentResolution {
                            comment_id: "message-999".into(),
                            response: "unknown id is skipped".into(),
                        },
                    ]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();

        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            plan.stages[0].state,
            StageDocState::Planned,
            "a revised doc resets the stale approval"
        );
        assert_eq!(plan.revising_stage_id, None);
        let answered = comment_by_id(&plan, &first_comment);
        assert_eq!(answered.state, crate::thread::DocCommentState::Addressed);
        assert_eq!(
            answered.agent_reply.as_deref(),
            Some("switched to a timestamp")
        );
        assert_eq!(
            comment_by_id(&plan, &second_comment).state,
            crate::thread::DocCommentState::Open
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First (revised)\n"),
            "the revision is ingested into the canonical store copy"
        );
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

    #[test]
    fn reattach_plan_mirrors_the_store_record() {
        let record = PersistedPlan {
            id: "plan-1".into(),
            goal: "add a greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state: crate::plan::PlanState::Interrupted,
            archived_at: Some("2026-07-01T10:06:00Z".into()),
            implementation_intent: crate::plan::ImplementationIntent::All,
            implementation_activity: crate::plan::ImplementationActivity::WaitingApproval(
                "second".into(),
            ),
            plan_path: ".build/plan.md".into(),
            stages: vec![],
            provider: crate::models::AgentProvider::Claude,
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
            agents: crate::agent::stored_agents("plan-1"),
            legacy_thread: crate::thread::Thread::default(),
            last_summary: Some("planned it".into()),
            last_error: Some("boom".into()),
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
            state_changed_at: None,
        };
        let active = ActivePlan::reattach(&record);
        assert_eq!(active.plan.id.0, "plan-1");
        assert_eq!(active.plan.state, PlanState::Interrupted);
        assert_eq!(active.plan.archived_at, record.archived_at);
        assert_eq!(
            active.workspace, None,
            "a reattached plan holds no workspace: the next dispatch re-derives it"
        );
        assert_eq!(active.base_branch, "main");
        assert_eq!(
            active.model_choice.model.as_deref(),
            Some("claude-opus-4-8")
        );
        assert_eq!(active.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn reattach_run_mirrors_the_store_record() {
        let record = PersistedRun {
            id: "run-1".into(),
            plan_id: Some("plan-1".into()),
            goal: "add a greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state: crate::run::RunState::Interrupted,
            branch: "build/add-a-greeting".into(),
            worktree_name: "add-a-greeting".into(),
            worktree_path: "/tmp/wt/add-a-greeting".into(),
            base_sha: Some("deadbeef".into()),
            stages: vec![crate::run::StageProgress::dispatched("first")],
            current_stage_id: Some("first".into()),
            revising_stage_id: None,
            auto_advance: true,
            adopted: true,
            triage: None,
            recovery: None,
            publication_attempt: None,
            provider: crate::models::AgentProvider::Claude,
            model: None,
            effort: Some("high".into()),
            agents: crate::agent::stored_agents("run-1"),
            legacy_thread: crate::thread::Thread::default(),
            last_summary: Some("built it".into()),
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
            state_changed_at: None,
        };
        let active = ActiveRun::reattach(&record, ".build/plan.md".into());
        assert_eq!(active.run.id.0, "run-1");
        assert_eq!(
            active.run.plan_id.as_ref().map(|p| p.0.as_str()),
            Some("plan-1")
        );
        assert_eq!(active.run.state, RunState::Interrupted);
        assert_eq!(active.worktree.branch(), "build/add-a-greeting");
        assert_eq!(active.base_sha.as_deref(), Some("deadbeef"));
        assert_eq!(active.plan_path, ".build/plan.md");
        assert_eq!(active.stages.len(), 1);
        assert_eq!(active.current_stage_id.as_deref(), Some("first"));
        assert!(active.auto_advance);
        assert!(active.adopted);
        assert_eq!(active.model_choice.effort.as_deref(), Some("high"));
    }

    #[tokio::test]
    async fn dispatch_single_stage_run_goes_straight_to_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(
            run.run.plan_id.as_ref().map(|id| id.0.as_str()),
            Some("plan-of-run-1"),
            "every run implements a plan"
        );
        assert!(
            run.base_sha.is_some(),
            "the materialized plan doc baselines the review diff"
        );
        assert!(run.worktree.branch().starts_with("build/"));
        assert!(run.worktree.path.join(mcp_config_path("run-1")).exists());

        std::fs::write(run.worktree.path.join("fix.txt"), "fixed\n").unwrap();
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Review);
        let diff = orch.run_diff(&run).unwrap();
        assert!(diff.files().iter().any(|f| f.path == "fix.txt"));
    }

    #[tokio::test]
    async fn dispatch_planned_run_materializes_commits_and_baselines_the_diff() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_plan(&orch, &store, "plan-1");

        let (mut run, turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(
            run.run.plan_id.as_ref().map(|p| p.0.as_str()),
            Some("plan-1")
        );
        assert_eq!(
            run.run.goal, "Add a greeting",
            "the run inherits the plan's goal"
        );

        // Dispatch order: materialize → commit ("plan: <goal>") → base_sha.
        assert_eq!(
            std::fs::read_to_string(run.worktree.path.join(".build/plan.md")).unwrap(),
            "# Plan v1\n"
        );
        assert_eq!(
            last_commit_subject(&run.worktree.path),
            "plan: Add a greeting"
        );
        let head = worktree_head(&run.worktree.path);
        assert_eq!(run.base_sha.as_deref(), Some(head.as_str()));

        // The materialized docs are the diff baseline — zero review noise…
        assert!(orch.run_diff(&run).unwrap().files().is_empty());
        // …while build-agent work (and any doc edits) still surface.
        std::fs::write(run.worktree.path.join("greeting.txt"), "hello\n").unwrap();
        let diff = orch.run_diff(&run).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["greeting.txt"]);

        // The build prompt points at the plan's doc, warm or cold.
        let prompt = dispatch_turn_halves(&turn, "build");
        assert!(prompt.contains(".build/plan.md"), "{prompt}");
        assert!(prompt.contains("Add a greeting"), "{prompt}");

        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Review);
    }

    // ---- Review prioritization (triage) ----

    use crate::run::{TriageHunk, TriageLevel, TriageReport};

    fn done_build_with_report(report: crate::thread::CompletionReport) -> DoneReport {
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "built it".into(),
            outputs: DoneOutputs {
                completion_report: Some(report),
                ..DoneOutputs::default()
            },
        }
    }

    fn done_triage(based_on: &str, hunks: Vec<TriageHunk>) -> DoneReport {
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

    fn classified(hunk_id: &str, level: TriageLevel) -> TriageHunk {
        TriageHunk {
            hunk_id: hunk_id.into(),
            level,
            rationale: Some("because".into()),
            group: None,
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
        let build_report = done_build_with_report(crate::thread::CompletionReport {
            critical_files: vec!["crypto.rs — key derivation".into()],
            risk_notes: vec!["untested on rotation".into()],
            ..Default::default()
        });
        let consumed = orch
            .on_run_done(&mut run, &[], build_report.clone())
            .unwrap();
        assert!(consumed.next.is_none(), "nothing else is being said");
        assert!(
            triage_is_due(&build_report, consumed.next.is_some()),
            "the diff wants ordering for review"
        );

        let patch = orch.run_diff(&run).unwrap().patch().to_string();
        let seed = crate::thread::CompletionReport {
            critical_files: vec!["crypto.rs — key derivation".into()],
            risk_notes: vec!["untested on rotation".into()],
            ..Default::default()
        };
        let turn = orch
            .triage_turn(&run, &patch, "revision-sha-1", Some(&seed))
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

    /// Triage gates nothing, so the lifecycle's opinion of a report does not
    /// decide whether the diff gets ordered. An agent that reports done at a
    /// review gate moves no state and still leaves a diff to read.
    #[test]
    fn what_needs_ordering_is_decided_by_the_diff_not_by_the_state_machine() {
        for phase in [DonePhase::Build, DonePhase::Revise] {
            assert!(triage_is_due(
                &done(phase, DoneStatus::Completed, None),
                false
            ));
            assert!(
                !triage_is_due(&done(phase, DoneStatus::Blocked, None), false),
                "a blocked turn produced no finished diff"
            );
            assert!(
                !triage_is_due(&done(phase, DoneStatus::Completed, None), true),
                "the agent hears one thing at a time"
            );
        }
        for phase in [DonePhase::Plan, DonePhase::Triage, DonePhase::Route] {
            assert!(!triage_is_due(
                &done(phase, DoneStatus::Completed, None),
                false
            ));
        }
    }

    /// Nothing changed, nothing to order: the turn is not spent.
    #[tokio::test]
    async fn an_empty_diff_gets_no_triage_turn() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
        assert!(orch.triage_turn(&run, "", "revision-sha-1", None).is_none());
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
    async fn an_implementable_issue_enforces_the_single_active_writer_rule() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_plan(&orch, &store, "plan-1");

        let err = match ImplementableIssue::judge(RunSource {
            plan: &plan,
            has_active_run: true,
        }) {
            Ok(_) => panic!("a second concurrent run of the same plan must be rejected"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("active run"), "{err}");
    }

    #[tokio::test]
    async fn an_implementable_issue_requires_an_approved_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = plan_in_review(&orch, &store, "plan-1");

        let err = match ImplementableIssue::judge(RunSource {
            plan: &plan,
            has_active_run: false,
        }) {
            Ok(_) => panic!("only an approved plan can be implemented"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("approved"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_multi_stage_run_starts_the_first_stage() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);

        let (run, turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.current_stage_id.as_deref(), Some("first"));
        assert_eq!(run.stages.len(), 1);
        assert_eq!(run.stages[0].state, StageProgressState::Building);
        assert_eq!(
            run.stages[0].start_sha, run.base_sha,
            "the first stage's diff starts at the materialization commit"
        );
        assert!(run.worktree.path.join(".build/plan/01-first.md").exists());
        let prompt = dispatch_turn_halves(&turn, "build");
        assert!(prompt.contains("Execute ONE stage"), "{prompt}");
        assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");
        assert!(
            prompt.contains("Ordered Issue stage-plan catalog"),
            "{prompt}"
        );
        let first = prompt
            .find("first — First")
            .expect("first stage in catalog");
        let second = prompt
            .find("second — Second")
            .expect("second stage in catalog");
        assert!(first < second, "catalog preserves manifest order: {prompt}");
        assert!(
            turn.cold
                .contains("Before acting, call `read_unread_messages`"),
            "cold Issue agents always pull the authoritative mailbox: {}",
            turn.cold
        );
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
    async fn run_validation_pass_mid_plan_parks_at_the_stage_gate() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        run.auto_advance = true;
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(true, "- ok", "note for second"),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(
            run.stages[0].state,
            StageProgressState::Validated { passed: true }
        );
        assert_eq!(
            run.stages[0]
                .validation
                .as_ref()
                .map(|v| v.notes_for_next_stage.as_str()),
            Some("note for second")
        );
        assert!(run.auto_advance, "run-all stays armed after a pass");
        assert_eq!(run.stages[0].completion_sha, run.stages[0].built_sha);
    }

    #[tokio::test]
    async fn validation_rejects_a_dirty_or_moved_candidate_boundary() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 1);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("only.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        std::fs::write(run.worktree.path.join("validation-mutated.txt"), "bad\n").unwrap();
        let error = orch
            .on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("validation must be observational"),
            "{error}"
        );
        assert_eq!(run.stages[0].state, StageProgressState::Validating);
        assert_eq!(run.stages[0].completion_sha, None);
    }

    #[tokio::test]
    async fn run_validation_pass_on_the_last_stage_opens_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 1);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("only.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(run.run.state, RunState::Review, "last stage → merge review");
    }

    #[tokio::test]
    async fn run_validation_failure_parks_at_the_stage_gate_and_disarms_run_all() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        run.auto_advance = true;
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(false, "- migration missing", ""),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(
            run.stages[0].state,
            StageProgressState::Validated { passed: false }
        );
        assert_eq!(
            run.stages[0]
                .validation
                .as_ref()
                .map(|v| v.findings.as_str()),
            Some("- migration missing")
        );
        assert!(!run.auto_advance, "a failed validation disarms run-all");
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

    // ---- Plan/Run split: stage flows, interaction verbs, git finishers ----

    /// Drive a two-stage planned run through its first stage (build + a passing
    /// validation), leaving it parked at the between-stages gate with stage one
    /// `Validated{passed:true}`.
    fn run_past_first_stage(
        orch: &Orchestrator,
        store: &Store,
        plan: &ActivePlan,
        id: &str,
    ) -> ActiveRun {
        let mut run = dispatch_planned_run(orch, store, plan, id);
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", "notes"))
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        run
    }

    #[tokio::test]
    async fn approve_plan_stage_approves_a_doc_and_rejects_on_terminal() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

        // An approved plan keeps taking per-stage approvals (that is how a run's
        // later stages get gated while an earlier one builds).
        orch.approve_plan(&mut plan).unwrap();
        orch.approve_plan_stage(&mut plan, "second").unwrap();
        assert_eq!(plan.stages[1].state, StageDocState::Approved);
        assert_eq!(plan.stages[0].state, StageDocState::Planned);

        // Double-approve is rejected by the pure doc-state machine.
        let err = orch
            .approve_plan_stage(&mut plan, "second")
            .expect_err("double approve is illegal");
        assert!(matches!(err, OrchestratorError::StageDoc(_)), "{err}");

        // Unknown stage id → a gate error, not a panic.
        assert!(matches!(
            orch.approve_plan_stage(&mut plan, "ghost"),
            Err(OrchestratorError::Gate(_))
        ));

        // A terminal plan takes no approvals.
        orch.abandon_plan(&mut plan).unwrap();
        let err = orch
            .approve_plan_stage(&mut plan, "first")
            .expect_err("no approvals on a terminal plan");
        assert!(err.to_string().contains("terminal"), "{err}");
    }

    #[tokio::test]
    async fn send_plan_stage_notes_revises_a_stage_and_round_trips_through_done() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Approved;
        let first_comment = comment_on(&mut plan, "first");
        comment_on(&mut plan, "second");

        let turn = send_plan_stage_notes(&orch, &mut plan, &store, "first").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(plan.revising_stage_id.as_deref(), Some("first"));
        let prompt = posted_turn_halves(&turn, "revise", THREAD_NOTIFICATION);
        assert!(prompt.contains("read_unread_messages"), "{prompt}");
        assert_eq!(
            turn.warm, THREAD_NOTIFICATION,
            "the comments travel through MCP; the instruction only points at them"
        );
        assert!(
            !prompt.contains("Comment:"),
            "no server-rendered comment block: {prompt}"
        );
        assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");

        // The agent revises the doc and reports done → back to PlanReview, the
        // stage approval reset, the comment resolved, the store copy updated.
        std::fs::write(
            plan_docs_dir(&plan).join(".build/plan/01-first.md"),
            "# Stage: First (revised)\n",
        )
        .unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            DoneReport {
                phase: DonePhase::Revise,
                status: DoneStatus::Completed,
                summary: "revised".into(),
                outputs: DoneOutputs {
                    comment_resolutions: Some(vec![crate::mcp::CommentResolution {
                        comment_id: first_comment.clone(),
                        response: "done".into(),
                    }]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(plan.stages[0].state, StageDocState::Planned);
        assert_eq!(plan.revising_stage_id, None);
        assert_eq!(
            comment_by_id(&plan, &first_comment).state,
            crate::thread::DocCommentState::Addressed
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First (revised)\n")
        );
    }

    #[tokio::test]
    async fn send_plan_stage_notes_gates_on_plan_state_and_open_comments() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

        // No open comments on the stage.
        let err =
            send_plan_stage_notes(&orch, &mut plan, &store, "first").expect_err("no open comments");
        assert!(err.to_string().contains("no open comments"), "{err}");
        assert_eq!(plan.plan.state, PlanState::PlanReview);

        // Not at the review gate (approved) → the transition is rejected.
        orch.approve_plan(&mut plan).unwrap();
        comment_on(&mut plan, "first");
        let err = send_plan_stage_notes(&orch, &mut plan, &store, "first")
            .expect_err("an approved plan is past the review gate");
        assert!(err.to_string().contains("cannot send stage notes"), "{err}");
    }

    #[tokio::test]
    async fn message_plan_redirects_drafting_resumes_parked_and_refuses_gates() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // Empty message is refused before any state is touched.
        let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");
        assert!(message_plan(&orch, &mut plan, &store, "   ")
            .unwrap_err()
            .to_string()
            .contains("empty"));

        // Drafting → a live redirect (no state change): the message is a turn
        // for the agent already drafting, never a replacement session.
        let turn = message_plan(&orch, &mut plan, &store, "focus on error paths").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        posted_turn_halves(&turn, "message", "focus on error paths");

        // A blocked plan resumes drafting on reply.
        orch.on_plan_done(
            &mut plan,
            &store,
            done(DonePhase::Plan, DoneStatus::Blocked, None),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::Blocked);
        let turn = message_plan(&orch, &mut plan, &store, "here is the missing detail").unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        posted_turn_halves(&turn, "message", "here is the missing detail");

        // The review gate refuses a side-channel message.
        let mut in_review = plan_in_review(&orch, &store, "plan-2");
        let err = message_plan(&orch, &mut in_review, &store, "sneak past the gate")
            .expect_err("review gate has send-notes");
        assert!(err.to_string().contains("review gate"), "{err}");
    }

    #[tokio::test]
    async fn resume_plan_redispatches_an_interrupted_plan_remaking_its_workspace() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A plan interrupted mid-draft: its store docs survive, its scratch
        // docs dir is gone (what a restart leaves behind).
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        // Move it back to a working phase then interrupt it.
        send_plan_notes(&orch, &mut plan, &store, "revise").unwrap();
        plan.plan.apply(crate::plan::PlanEvent::Interrupt).unwrap();
        let stale = plan.workspace.take().unwrap();
        std::fs::remove_dir_all(&stale.docs_dir).unwrap();

        let turn = resume_plan(&orch, &mut plan, &store).unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let workspace = plan
            .workspace
            .as_ref()
            .expect("resume remade the workspace");
        assert_eq!(workspace.checkout, repo, "still the primary checkout");
        assert!(
            workspace.docs_dir.join(".build/plan.md").exists(),
            "docs materialized"
        );
        // The re-plan instruction travels whether the agent is the one that was
        // interrupted or a fresh replacement.
        let prompt = dispatch_turn_halves(&turn, "revise");
        assert!(
            prompt.contains("Add a greeting"),
            "resume re-plans the same goal: {prompt}"
        );
    }

    #[tokio::test]
    async fn abandon_plan_drops_the_docs_dir_and_keeps_the_store_docs() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let docs_dir = plan_docs_dir(&plan);

        orch.abandon_plan(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::Abandoned);
        assert_eq!(plan.workspace, None);
        assert!(!docs_dir.exists(), "the scratch docs are gone");
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n"),
            "canonical docs survive an abandon"
        );
    }

    #[tokio::test]
    async fn dispatch_run_stage_enforces_the_sequential_gate_and_pins_start_sha() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");
        // Un-approve the second doc (a mid-run revision resets approval the
        // same way) so the gate has something to refuse.
        plan.stages[1].state = StageDocState::Planned;

        // The next stage's doc is not approved yet → refused.
        let err = orch
            .dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .expect_err("an unapproved stage cannot dispatch");
        assert!(err.to_string().contains("not approved"), "{err}");
        assert_eq!(
            run.run.state,
            RunState::StageGate,
            "no state change on refusal"
        );

        // Approve it → the sequential gate opens (stage one validated).
        orch.approve_plan_stage(&mut plan, "second").unwrap();
        let turn = orch
            .dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.current_stage_id.as_deref(), Some("second"));
        let second = run.stage_progress("second").unwrap();
        assert_eq!(second.state, StageProgressState::Building);
        assert_eq!(
            second.start_sha.as_deref(),
            Some(worktree_head(&run.worktree.path).as_str()),
            "the stage diff pins to HEAD at dispatch"
        );
        let prompt = dispatch_turn_halves(&turn, "build");
        assert!(prompt.contains(".build/plan/02-second.md"), "{prompt}");
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

    #[tokio::test]
    async fn dispatch_run_stage_rejects_a_stage_whose_predecessor_has_not_validated() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        // Stage one fails validation → the run parks at the gate, stage one
        // `Validated{passed:false}`.
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        orch.on_run_done(&mut run, &plan.stages, done_validate(false, "- nope", ""))
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);

        let err = orch
            .dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .expect_err("stage one has not passed validation");
        assert!(
            err.to_string().contains("has not passed validation"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn fix_run_stage_respawns_with_findings_and_keeps_the_start_sha() {
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
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(false, "- migration missing", ""),
        )
        .unwrap();
        let start_before = run.stage_progress("first").unwrap().start_sha.clone();

        let turn = orch
            .fix_run_stage(&mut run, &plan.stages, "first", "add the migration")
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        let first = run.stage_progress("first").unwrap();
        assert_eq!(first.state, StageProgressState::Building);
        assert_eq!(
            first.start_sha, start_before,
            "the fix keeps the stage's start sha"
        );
        let prompt = dispatch_turn_halves(&turn, "build");
        assert!(
            prompt.contains("- migration missing"),
            "findings drive the fix: {prompt}"
        );
        assert!(
            prompt.contains("add the migration"),
            "the note is the steer: {prompt}"
        );

        // Nothing to fix on a stage without a failed validation.
        let err = orch
            .fix_run_stage(&mut run, &plan.stages, "second", "")
            .expect_err("second has no progress to fix");
        assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
    }

    /// Requesting changes hands the caller a turn to deliver; it never ends the
    /// worktree's agent nor spawns a replacement. The turn carries both halves
    /// of the cold/warm rule: the full run-context prompt for an agent that had
    /// to be spawned, and the caller's bare instruction for one already in the
    /// conversation.
    #[tokio::test]
    async fn run_request_changes_returns_a_revise_turn_and_never_respawns() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
        let consumed = orch
            .on_run_done(
                &mut single,
                &[],
                done(DonePhase::Build, DoneStatus::Completed, None),
            )
            .unwrap();
        assert_eq!(single.run.state, RunState::Review);
        assert!(
            consumed.next.is_none(),
            "opening review says nothing to the agent — it is the human's move"
        );

        let turn = orch
            .run_request_changes(&mut single, &[], "tweak it", None)
            .unwrap();
        assert_eq!(single.run.state, RunState::Building);
        let cold = posted_turn_halves(&turn, "revise", "tweak it");
        assert!(
            cold.contains("phase=\"revise\""),
            "a cold agent gets the whole revise prompt: {cold}"
        );

        // A stage awaiting its validation verdict must not be redirected.
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
        let err = orch
            .run_request_changes(&mut run, &plan.stages, "no", None)
            .expect_err("cannot redirect a validating stage");
        assert!(err.to_string().contains("awaiting validation"), "{err}");
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
    async fn message_run_redirects_building_continues_and_refuses_gates() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
        assert!(orch
            .message_run(&mut run, &[], "  ")
            .unwrap_err()
            .to_string()
            .contains("empty"));

        // Building → the run keeps working and the message becomes a turn.
        let turn = orch
            .message_run(&mut run, &[], "also handle the empty case")
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        posted_turn_halves(&turn, "message", "also handle the empty case");

        // The review gate refuses a message (request-changes is the verb there).
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        let err = orch
            .message_run(&mut run, &[], "sneak past")
            .expect_err("review gate refuses messages");
        assert!(err.to_string().contains("review gate"), "{err}");
    }

    #[tokio::test]
    async fn spawned_plan_and_run_prompts_put_the_ambiguity_rule_before_silent_directives() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let (_, plan_turn) = drafting_plan_and_turn(&orch, &store, "plan-1", "Add a greeting");
        dispatch_turn_halves(&plan_turn, "plan");
        let plan_prompt = plan_turn.cold;
        let plan = approved_plan(&orch, &store, "plan-of-run-1");
        let (_, run_turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
        dispatch_turn_halves(&run_turn, "build");
        let run_prompt = run_turn.cold;

        for (path, prompt) in [("plan", plan_prompt), ("run", run_prompt)] {
            let ambiguity_rule = prompt
                .find("either a question or a directive")
                .unwrap_or_else(|| {
                    panic!("{path} spawn prompt lacks the ambiguity rule: {prompt}")
                });
            let silent_directive_allowance = prompt
                .find("directive without replying")
                .unwrap_or_else(|| {
                    panic!("{path} spawn prompt lacks the silent-directive allowance: {prompt}")
                });
            assert!(
                ambiguity_rule < silent_directive_allowance,
                "{path}: the ambiguity rule must precede the silent-directive allowance so an \
                 in-order reader hits the carve-out before committing to silence: {prompt}"
            );
        }
    }

    #[test]
    fn conversation_prompt_instructs_clarifying_reply_for_ambiguous_comments() {
        let prompt = conversation_prompt("do the work");
        assert!(prompt.contains("Build conversation protocol"), "{prompt}");
        assert!(
            prompt.contains("either a question or a directive"),
            "ambiguous reviewer messages must trigger a clarifying reply: {prompt}"
        );
        assert!(
            prompt.contains("one-line clarifying reply"),
            "the reply must be a one-liner, not a silent code change: {prompt}"
        );
    }

    #[tokio::test]
    async fn resume_run_redispatches_single_stage_and_multi_stage_builds() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A single-stage run interrupted mid-build → resumes the whole-run build.
        let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
        single.run.apply(crate::run::RunEvent::Interrupt).unwrap();
        let turn = orch.resume_run(&mut single, &[]).unwrap();
        assert_eq!(single.run.state, RunState::Building);
        let prompt = dispatch_turn_halves(&turn, "resume");
        assert!(prompt.contains("single stage work"), "{prompt}");

        // Multi-stage run interrupted mid stage-build → resumes THAT stage.
        let orch2 = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees2"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
            std::fs::canonicalize(repo.join("README.md")).unwrap(),
        );
        let plan = approved_multi_stage_plan(&orch2, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch2, &store, &plan, "run-1");
        run.run.apply(crate::run::RunEvent::Interrupt).unwrap();
        let turn = orch2.resume_run(&mut run, &plan.stages).unwrap();
        assert_eq!(run.run.state, RunState::Building);
        let prompt = dispatch_turn_halves(&turn, "resume");
        assert!(
            prompt.contains(".build/plan/01-first.md"),
            "resumes stage one: {prompt}"
        );
    }

    #[tokio::test]
    async fn adopt_run_lands_in_review_as_a_plan_less_run() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-user", "user/thing");
        std::fs::write(external.path.join("notes.txt"), "pre-Build work\n").unwrap();

        let adoptable =
            AdoptableCheckout::judge(&external, "main", AdoptionScope::ExternalWorktree).unwrap();
        orch.prepare_adoption(&adoptable, "main", "run-ad").unwrap();
        let run = orch
            .adopt_run(RunId::new("run-ad"), &adoptable, "main", Default::default())
            .unwrap();
        assert_eq!(run.run.state, RunState::Review);
        assert_eq!(run.run.plan_id, None, "an adopted run has no plan");
        assert_eq!(run.run.goal, "user/thing");
        assert_eq!(
            run.base_sha, None,
            "adopted runs baseline on the merge-base"
        );
        assert!(run.adopted);
        assert_eq!(
            last_commit_subject(&external.path),
            "Checkpoint: adopted by Build"
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
        assert_eq!(second.run.state, RunState::Review);
        assert!(
            !repo.join(".git/MERGE_HEAD").exists(),
            "a failed merge is aborted"
        );
    }

    #[tokio::test]
    async fn abandon_run_removes_the_worktree_but_keeps_the_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
        let branch = run.worktree.branch();
        let path = run.worktree.path.clone();

        orch.abandon_run_keeping_checkout(&mut run).unwrap();
        orch.discard_checkout(&run.worktree, /* keep_branch */ true);
        assert_eq!(run.run.state, RunState::Abandoned);
        assert!(!path.exists(), "the worktree is removed");
        // The branch survives — a run's work outlives an abandon so it can be
        // re-attempted (the run entity's documented contract).
        let branches = Command::new("git")
            .args(["branch", "--list", &branch])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).contains(&branch),
            "the branch is kept on abandon"
        );
    }

    #[tokio::test]
    async fn mid_run_stage_revision_writes_back_to_the_plan_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        // The upcoming stage's doc was approved; a reviewer left a comment.
        plan.stages[1].state = StageDocState::Approved;
        let comment_id = comment_on(&mut plan, "second");
        let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");

        // The revision runs in the RUN's worktree; the run's coarse state is
        // untouched (it merely lends its worktree).
        let turn = orch
            .send_run_stage_notes(&mut run, &plan, "second")
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(run.revising_stage_id.as_deref(), Some("second"));
        // The revision is a turn for the run worktree's agent — the comments
        // themselves travel through MCP, so the turn only points at them.
        let cold = posted_turn_halves(&turn, "revise", THREAD_NOTIFICATION);
        assert!(
            cold.contains(".build/plan/02-second.md"),
            "a cold agent is pointed at the stage doc: {cold}"
        );

        // While a revision is in flight, a revise report must NOT go through
        // on_run_done — it is a store write-back, not a build report.
        let revise = DoneReport {
            phase: DonePhase::Revise,
            status: DoneStatus::Completed,
            summary: "revised".into(),
            outputs: DoneOutputs {
                comment_resolutions: Some(vec![crate::mcp::CommentResolution {
                    comment_id: comment_id.clone(),
                    response: "reworked the section".into(),
                }]),
                ..DoneOutputs::default()
            },
        };
        let guard = orch
            .on_run_done(&mut run, &plan.stages, revise.clone())
            .expect_err("on_run_done rejects a revision in flight");
        assert!(
            guard.to_string().contains("consume_run_stage_revision"),
            "{guard}"
        );

        // The agent revised the doc in the run's worktree; consuming ingests it
        // back to the plan store, resets the stale approval, resolves the comment.
        std::fs::write(
            run.worktree.path.join(".build/plan/02-second.md"),
            "# Stage: Second (reworked)\n",
        )
        .unwrap();
        orch.consume_run_stage_revision(&mut run, &mut plan, &store, &revise)
            .unwrap();
        assert_eq!(run.revising_stage_id, None);
        assert_eq!(
            run.run.state,
            RunState::StageGate,
            "the build did not advance"
        );
        assert_eq!(
            plan.stages[1].state,
            StageDocState::Planned,
            "a revised doc resets its stale approval"
        );
        assert_eq!(
            comment_by_id(&plan, &comment_id).state,
            crate::thread::DocCommentState::Addressed
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/02-second.md")
                .as_deref(),
            Some("# Stage: Second (reworked)\n"),
            "the revision reached the canonical store copy"
        );
    }

    #[tokio::test]
    async fn send_run_stage_notes_is_only_legal_at_the_stage_gate() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        comment_on(&mut plan, "first");
        // A run still building its first stage is not at the gate.
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        let err = orch
            .send_run_stage_notes(&mut run, &plan, "first")
            .expect_err("not at the stage gate");
        assert!(err.to_string().contains("stage gate"), "{err}");
    }
}
