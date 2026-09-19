use crate::app::{announce_isolation_downgrade, err, record_session_death_in_thread, AppState};
use crate::mcp::{DoneReport, DoneStatus};
use crate::orchestrator::{ActivePlan, ActiveRun, ReportOutcome};
use crate::plan::StageDoc;
use crate::run::StageProgressState;
use crate::store::now_rfc3339;

/// How a harness's session ended, as the idle sweep saw it: the exit code, and
/// the last thing it painted. A crash's only explanation is usually on its own
/// screen — codex refusing to start a required MCP server, a provider saying
/// the account is out of quota — and a bare code throws that away.
#[derive(Debug, Clone)]
pub(in crate::app) struct HarnessExit {
    pub(in crate::app) code: i32,
    pub(in crate::app) epitaph: Option<String>,
}

pub(in crate::app) fn append_plan_stage_announcements(
    thread: &mut crate::thread::Thread,
    plan_id: &str,
    stages: &[(usize, StageDoc)],
) {
    let now = now_rfc3339();
    for (index, stage) in stages {
        let explanation = if stage.summary.trim().is_empty() {
            format!("**Stage {}: {}**", index + 1, stage.title)
        } else {
            format!(
                "**Stage {}: {}**\n\n{}",
                index + 1,
                stage.title,
                stage.summary.trim()
            )
        };
        thread.post_agent_with_links(
            explanation,
            None,
            vec![crate::thread::ThreadLink::IssueStage {
                issue_id: plan_id.to_string(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            }],
            &now,
        );
    }
}

pub(in crate::app) fn record_current_stage_started(
    thread: &mut crate::thread::Thread,
    active: &ActiveRun,
    stages: &[StageDoc],
) {
    let Some(plan_id) = active.run.plan_id.as_ref().map(|id| id.0.clone()) else {
        return;
    };
    let Some(stage_id) = active.current_stage_id.as_deref() else {
        return;
    };
    let Some(stage) = stages.iter().find(|stage| stage.id == stage_id) else {
        return;
    };
    thread.push_event_with_links(
        crate::thread::ThreadEventKind::StageStarted,
        Some(format!("Started plan stage “{}”", stage.title)),
        None,
        None,
        vec![
            crate::thread::ThreadLink::IssueStage {
                issue_id: plan_id.clone(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            },
            crate::thread::ThreadLink::Implementation {
                issue_id: plan_id,
                implementation_id: active.run.id.0.clone(),
            },
        ],
        now_rfc3339(),
    );
}

/// Close the conversation's open session, if one is open. Nothing to close is
/// the normal case for a thread whose agent never started, so it is silence,
/// not an error.
///
/// The mirror of [`open_session_lineage`], and the ONLY way a session ends: a
/// session is the life of an agent PROCESS, so it closes when that process
/// does (the tab pump's EOF) or when Build kills it — never when the agent
/// merely finishes a turn.
pub(in crate::app) fn finish_open_session(
    thread: &mut crate::thread::Thread,
    agent_id: &str,
    now: &str,
) {
    let Some(instance) = thread.open_session_instance(agent_id) else {
        return;
    };
    thread.finish_session_instance(&instance, now);
}

/// The conversation's open session, if one is open — the agent process
/// speaking right now, which is what an event it produces belongs to.
pub(in crate::app) fn open_session_id(
    thread: &crate::thread::Thread,
    agent_id: &str,
) -> Option<String> {
    thread
        .open_session_instance(agent_id)
        .map(|instance| instance.id)
}

/// Close every conversation an abandoned run was holding open. The run is out
/// of the map by now, so its lineage closes on the record this call holds
/// rather than through the owner lookup.
pub(in crate::app) fn close_abandoned_run_conversations(active: &mut ActiveRun) {
    let now = now_rfc3339();
    if let Some(primary) = active.agents.primary_mut() {
        let agent_id = primary.id.clone();
        finish_open_session(&mut primary.thread, &agent_id, &now);
        primary.thread.push_event(
            crate::thread::ThreadEventKind::Abandoned,
            Some("Run abandoned".to_string()),
            None,
            None,
            now.clone(),
        );
    }
    // A branch may carry several agents and the decide phase took every one of
    // them. `Abandoned` closed the first agent's turn (and, for a planned
    // implementation, its Issue's — see `mirror_run_outcome_to_issue`); the
    // agents beside it were told nothing, so each one that died mid-turn is
    // closed on its own conversation. Every other teardown path removes the run
    // from the board entirely, so there is no row left to read as working.
    for agent in active.agents.iter_mut() {
        if agent.thread.working_since().is_some() {
            record_session_death_in_thread(&mut agent.thread, &now);
        }
    }
}

/// What an issue's conversation says when the branch implementing it is gone
/// and nothing was merged out of it. `how` is the way it went: abandoned by the
/// user, deleted outside Build, finished off the board.
pub(in crate::app) fn abandoned_branch_summary(branch: &str, how: &str) -> String {
    format!(
        "The branch {branch} was {how} without being merged, so this issue is waiting for work \
         again"
    )
}

/// Whether one of an implementation's events travels to the Issue that owns
/// it. Exactly the attention class: what needs the human is news wherever they
/// are watching from, what merely reports progress belongs to the run.
///
/// Events only, because an outcome is no longer one: a report the agent made on
/// a planned implementation is written straight onto the Issue's conversation
/// as the agent's own message (`record_report_in_thread`, whose caller picks
/// that conversation), which is the same timeline this mirror copies onto and
/// the same one unread entry. What still travels this way is what Build
/// observed about the implementation itself — an abandoned branch.
pub(in crate::app) fn run_outcome_mirrors_to_issue(event: crate::thread::ThreadEventKind) -> bool {
    event.class() == crate::thread::EventClass::Attention
}

/// What the daemon says about a `done` the branch's lifecycle does not accept.
///
/// Branch state belongs to the branch, not to whoever is talking to it
/// (Decisions §Entity model): a branch carries several agents, and one
/// dispatched onto a branch that is already sitting at a review gate finishing
/// its own instruction is that model working exactly as designed. So the report
/// is recorded, its attention event fires, the branch stays where it is — and
/// the line says so, instead of reading like a rejected transition somebody
/// needs to go and fix.
pub(in crate::app) fn out_of_phase_log(
    run_id: &str,
    illegal: &crate::run::IllegalRunTransition,
) -> String {
    format!(
        "on_agent_done {run_id}: dispatched-agent report recorded; branch state unchanged \
         ({:?} while the branch is {:?})",
        illegal.event, illegal.from
    )
}

/// The conversation the reporting run agent is canonically bound to. A planned
/// run's primary continues its Issue lineage; additional agents own independent
/// conversations on the run.
///
/// The run's own is reached through the mint door: the report came from an
/// agent of this run, so it must land somewhere even if the human emptied the
/// roster mid-turn.
pub(in crate::app) fn run_report_conversation<'a>(
    run_id: &str,
    reporting_agent_id: Option<&str>,
    active: &'a mut ActiveRun,
    issue: Option<&'a mut ActivePlan>,
) -> &'a mut crate::thread::Thread {
    match reporting_agent_id {
        None => match issue {
            Some(issue) => issue.agents.sole_thread_mut(),
            None => {
                let choice = active.model_choice.clone();
                &mut active
                    .agents
                    .ensure_primary(run_id, choice, &now_rfc3339())
                    .thread
            }
        },
        Some(reporting_agent_id) => {
            let reporting_agent = active
                .agents
                .by_id(reporting_agent_id)
                .expect("authenticated reporting agent belongs to the run");
            let conversation_id = reporting_agent.conversation_id().to_string();
            if conversation_id == reporting_agent.id {
                return &mut active
                    .agents
                    .by_id_mut(reporting_agent_id)
                    .expect("reporting agent was just resolved")
                    .thread;
            }
            let issue = issue.expect("an aliased run conversation belongs to its Issue");
            &mut issue
                .agents
                .by_id_mut(&conversation_id)
                .expect("reporting agent's canonical Issue conversation exists")
                .thread
        }
    }
}

pub(in crate::app) fn record_report_in_thread(
    thread: &mut crate::thread::Thread,
    report: &DoneReport,
    orchestration_error: Option<&str>,
) {
    let now = now_rfc3339();
    // A report is a status on the agent's own message: it said this, so there
    // is one record of it and the conversation carries it.
    let (outcome, summary) = match orchestration_error {
        // Still the agent's report, and still its outcome: Build's note about
        // why it could not be applied rides the same body.
        Some(error) => (
            crate::thread::MessageOutcome::Failed,
            format!(
                "{}\n\nBuild could not apply the report: {error}",
                report.summary
            ),
        ),
        None => (
            match report.status {
                DoneStatus::Blocked => crate::thread::MessageOutcome::Blocked,
                DoneStatus::Failed => crate::thread::MessageOutcome::Failed,
                DoneStatus::Completed => crate::thread::MessageOutcome::Completed,
            },
            report.summary.clone(),
        ),
    };
    if report.message_id.as_deref().is_some_and(|message_id| {
        thread.mark_agent_message_outcome(message_id, outcome, &summary, None)
    }) {
        return;
    }
    thread.post_outcome(outcome, summary, None, &now);
}

/// An entity went quiet (or its agent exited) without reporting: record the
/// reason. The session lineage is deliberately left alone — a quiet agent is
/// still an agent, and one that exited has already had its session closed by
/// the pump that saw the EOF.
pub(in crate::app) fn record_idle_in_thread(
    thread: &mut crate::thread::Thread,
    exit: Option<&HarnessExit>,
) {
    let now = now_rfc3339();
    let (event, summary) = match exit {
        Some(exit) => (
            crate::thread::ThreadEventKind::RunFailed,
            match &exit.epitaph {
                Some(said) => {
                    format!("Agent exited unexpectedly with code {}: {said}", exit.code)
                }
                None => format!("Agent exited unexpectedly with code {}", exit.code),
            },
        ),
        None => (
            crate::thread::ThreadEventKind::IdleUnreported,
            "Agent went quiet without reporting done".to_string(),
        ),
    };
    thread.push_event(event, Some(summary), None, None, now);
}

impl HarnessExit {
    /// The crash as one line of `last_error`.
    pub(in crate::app) fn describe(&self) -> String {
        match &self.epitaph {
            Some(said) => format!(
                "agent exited unexpectedly (exit code {}): {said}",
                self.code
            ),
            None => format!("agent exited unexpectedly (exit code {})", self.code),
        }
    }
}

/// The stage a failed report leaves behind, for its `StageFailed` event:
/// `(stage id, stage doc path, what went wrong)`.
type StageFailure = (String, String, String);
/// The stage a report just completed, for its `StageCompleted` event:
/// `(stage id, completion sha, stage doc path)`.
type StageCompletion = (String, Option<String>, Option<String>);

fn failed_stage_event(
    active: &ActiveRun,
    plan_docs: &[StageDoc],
    report: &DoneReport,
    rejected: bool,
) -> Option<StageFailure> {
    if report.status != DoneStatus::Failed && !rejected {
        return None;
    }
    let stage_id = active.current_stage_id.as_deref()?;
    let doc = plan_docs.iter().find(|doc| doc.id == stage_id)?;
    Some((
        stage_id.to_string(),
        doc.path.clone(),
        report.summary.clone(),
    ))
}

fn completed_stage_event(active: &ActiveRun, plan_docs: &[StageDoc]) -> Option<StageCompletion> {
    let stage_id = active.current_stage_id.as_deref()?;
    let progress = active
        .stage_progress(stage_id)
        .filter(|progress| progress.state == StageProgressState::Completed)?;
    Some((
        stage_id.to_string(),
        progress.completion_sha.clone(),
        plan_docs
            .iter()
            .find(|doc| doc.id == stage_id)
            .map(|doc| doc.path.clone()),
    ))
}

fn record_stage_events(
    conversation: &mut crate::thread::Thread,
    issue_id: &str,
    run_id: &str,
    failed: Option<StageFailure>,
    completed: Option<StageCompletion>,
) {
    let implementation = crate::thread::ThreadLink::Implementation {
        issue_id: issue_id.to_string(),
        implementation_id: run_id.to_string(),
    };
    if let Some((stage_id, stage_path, summary)) = failed {
        conversation.push_event_with_links(
            crate::thread::ThreadEventKind::StageFailed,
            Some(summary),
            None,
            None,
            vec![
                crate::thread::ThreadLink::IssueStage {
                    issue_id: issue_id.to_string(),
                    stage_id,
                    path: stage_path,
                },
                implementation.clone(),
            ],
            now_rfc3339(),
        );
    }
    if let Some((stage_id, completion_sha, stage_path)) = completed {
        let mut links = vec![implementation];
        if let Some(path) = stage_path {
            links.push(crate::thread::ThreadLink::IssueStage {
                issue_id: issue_id.to_string(),
                stage_id: stage_id.clone(),
                path,
            });
        }
        if let Some(sha) = completion_sha {
            links.push(crate::thread::ThreadLink::Commit { sha });
        }
        conversation.push_event_with_links(
            crate::thread::ThreadEventKind::StageCompleted,
            Some(format!("Completed stage {stage_id}")),
            None,
            None,
            links,
            now_rfc3339(),
        );
    }
}

impl AppState {
    /// A run agent reported `done`. A mid-run stage-doc revision (revising_stage_id
    /// set) is a cross-entity store write-back to the owning plan; every other
    /// report advances the run on `on_run_done`, and a completed stage may then
    /// auto-advance the next approved stage when run-all is armed.
    pub(in crate::app) fn on_run_agent_done(
        &mut self,
        run_id: &str,
        reporting_agent_id: Option<&str>,
        report: DoneReport,
    ) {
        let Some(mut active) = self.runs.remove(run_id) else {
            return;
        };
        if active.revising_stage_id.is_some() && report.status == DoneStatus::Completed {
            self.consume_run_stage_revision(run_id, reporting_agent_id, active, report);
            return;
        }
        let plan_docs = self.owning_plan_stage_docs(&active);
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let report_for_thread = report.clone();
        let stage_before = active
            .current_stage_id
            .as_deref()
            .and_then(|stage_id| active.stage_progress(stage_id))
            .map(|progress| progress.state);
        let outcome = self.apply_run_report(run_id, &mut active, &plan_docs, report);
        let diff_revision = if outcome.is_ok() && report_for_thread.status == DoneStatus::Completed
        {
            self.project_of(run_id).ok().and_then(|project_id| {
                self.orch_for(&project_id)
                    .and_then(|orch| orch.run_diff(&active).map_err(err))
                    .ok()
                    .map(|diff| diff.patch().to_string())
            })
        } else {
            None
        };
        let failed_stage_event =
            failed_stage_event(&active, &plan_docs, &report_for_thread, outcome.is_err());
        let completed_stage_event = if matches!(outcome, Ok(ReportOutcome::Applied))
            && stage_before == Some(StageProgressState::Building)
        {
            completed_stage_event(&active, &plan_docs)
        } else {
            None
        };
        let mut issue = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.remove(issue_id));
        let conversation =
            run_report_conversation(run_id, reporting_agent_id, &mut active, issue.as_mut());
        record_report_in_thread(
            conversation,
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if let Some(issue_id) = issue_id.as_deref() {
            record_stage_events(
                conversation,
                issue_id,
                run_id,
                failed_stage_event,
                completed_stage_event,
            );
        }
        if let Some(patch) = diff_revision {
            conversation.add_revision(crate::thread::ArtifactKind::Diff, &patch, &now_rfc3339());
        }
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        if let (Some(issue_id), Some(issue)) = (issue_id, issue) {
            let persisted = self.finish_plan_mutation(issue_id, issue);
            if let Err(e) = persisted {
                eprintln!("on_agent_done {run_id}: issue conversation persist failed: {e}");
            }
        }
        self.auto_advance_run(run_id);
        if let Some(issue_id) = self
            .runs
            .get(run_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.clone())
        {
            if let Err(error) = self.refresh_issue_scheduler_activity(&issue_id) {
                eprintln!("issue scheduler {issue_id}: {error}");
            }
        }
    }

    /// Hand the report to the run's lifecycle. Build's agents are persistent
    /// and a branch carries several, so a report arrives whenever any of them
    /// finishes a turn — a turn the human started at a review gate, or one a
    /// dispatch handed a second agent. No lifecycle event accepts those, and
    /// none should: enforcement is by observation, so the report is recorded
    /// on the conversation and the branch's own state stays put.
    fn apply_run_report(
        &mut self,
        run_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
        report: DoneReport,
    ) -> Result<ReportOutcome, String> {
        let outcome = self.project_of(run_id).and_then(|project_id| {
            self.orch_for(&project_id)?
                .on_run_done(active, plan_docs, report)
                .map_err(err)
        });
        match &outcome {
            Err(e) => eprintln!("on_agent_done {run_id}: {e}"),
            Ok(ReportOutcome::OutOfPhase(illegal)) => {
                eprintln!("{}", out_of_phase_log(run_id, illegal))
            }
            Ok(ReportOutcome::Applied) => {}
        }
        outcome
    }

    /// Consume a mid-run stage-doc revision's `done`: it writes the revised docs
    /// back to the owning plan's canonical store copy, so both the run (whose
    /// worktree holds the docs) and the plan (whose doc state + comments update)
    /// are mutated, then both re-persisted.
    pub(in crate::app) fn consume_run_stage_revision(
        &mut self,
        run_id: &str,
        reporting_agent_id: Option<&str>,
        mut active: ActiveRun,
        report: DoneReport,
    ) {
        let report_for_thread = report.clone();
        let plan_id = active.run.plan_id.as_ref().map(|p| p.0.clone());
        let mut plan = plan_id.as_ref().and_then(|pid| self.plans.remove(pid));
        let outcome = (|| -> Result<(), String> {
            let plan = plan
                .as_mut()
                .ok_or_else(|| "run stage revision: owning plan is gone".to_string())?;
            let project_id = self.project_of(run_id)?;
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .consume_run_stage_revision(&mut active, plan, store, &report)
                .map_err(err)
        })();
        if let Err(e) = &outcome {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        record_report_in_thread(
            run_report_conversation(run_id, reporting_agent_id, &mut active, plan.as_mut()),
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() {
            if let (Some(pid), Some(plan_ref)) = (plan_id.as_deref(), plan.as_mut()) {
                if let Some(contents) = self.plan_revision_contents(pid, plan_ref) {
                    plan_ref.agents.sole_thread_mut().add_revision(
                        crate::thread::ArtifactKind::Plan,
                        &contents,
                        &now_rfc3339(),
                    );
                }
            }
        }
        if let (Some(pid), Some(plan)) = (plan_id, plan) {
            let persisted = self.finish_plan_mutation(pid, plan);
            if let Err(e) = persisted {
                eprintln!("on_agent_done {run_id}: plan persist: {e}");
            }
        }
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
        }
    }

    /// The owning plan's stage-doc manifest for a run (the caller's join by
    /// `plan_id`): empty for adopted runs, single-doc plans, and orphaned links.
    pub(in crate::app) fn owning_plan_stage_docs(&self, run: &ActiveRun) -> Vec<StageDoc> {
        run.run
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(&pid.0))
            .map(|plan| plan.stages.clone())
            .unwrap_or_default()
    }

    /// Canonical conversation owner for a run. Planned runs are implementation
    /// lineage of the Issue and therefore project the Issue thread; planless
    /// adopted runs remain independent worktree entities. `None` for a branch
    /// with no agents — it has no conversation until somebody speaks to it.
    pub(in crate::app) fn conversation_thread_for_run<'a>(
        &'a self,
        run: &'a ActiveRun,
    ) -> Option<&'a crate::thread::Thread> {
        let primary = run.agents.primary()?;
        let conversation_id = primary.conversation_id();
        if conversation_id == primary.id {
            return Some(&primary.thread);
        }
        let owner = self.entity_of_agent(conversation_id)?;
        self.entity_agents(&owner)
            .ok()?
            .by_id(conversation_id)
            .map(|agent| &agent.thread)
    }

    /// Write to the conversation a run speaks in — its Issue's when it has one,
    /// its own otherwise — and persist whichever record owns it.
    ///
    /// The Issue's thread is what every surface of a planned run renders, so a
    /// report written anywhere else is invisible: the run reads as busy while
    /// nothing is happening in it.
    pub(in crate::app) fn record_on_run_conversation(
        &mut self,
        active: &mut ActiveRun,
        write: impl FnOnce(&mut crate::thread::Thread),
    ) -> Result<(), String> {
        let Some(primary) = active.agents.primary() else {
            return Ok(());
        };
        let agent_id = primary.id.clone();
        let conversation_id = primary.conversation_id().to_string();
        if conversation_id == agent_id {
            write(&mut active.agents.primary_mut().expect("just resolved").thread);
            return Ok(());
        }
        let owner = self.entity_of_agent(&conversation_id).ok_or_else(|| {
            format!("agent {agent_id} is bound to missing conversation {conversation_id}")
        })?;
        if self.plans.contains_key(&owner) {
            let mut issue = self.take_plan(&owner)?;
            write(&mut issue.agents.resolve_mut(Some(&conversation_id))?.thread);
            return self.finish_plan_mutation(owner, issue);
        }
        Err(format!(
            "run conversation {conversation_id} is not owned by an issue"
        ))
    }

    /// Announce on `active`'s conversation, and on the log, that the checkout
    /// it was just given is not the isolation the settings asked for.
    ///
    /// Says, never fails. A volume that cannot clone is a fact to tell the
    /// human, and by the time it is told the checkout stands, the branch is
    /// cut and the record is written — so a telling that does not land is a
    /// line in the log, never a create undone or a run left off the board.
    /// Every creation site announces through here, so no caller can choose
    /// another policy.
    pub(in crate::app) fn note_isolation_downgrade(
        &mut self,
        run_id: &str,
        active: &mut ActiveRun,
        reason: &str,
    ) {
        let note = announce_isolation_downgrade(reason);
        let written = self.record_on_run_conversation(active, |thread| {
            thread.push_event(
                crate::thread::ThreadEventKind::WorktreeCreated,
                Some(note),
                None,
                None,
                now_rfc3339(),
            );
        });
        if let Err(error) = written {
            eprintln!("{run_id}: the isolation fallback went unrecorded: {error}");
        }
    }

    /// Tell the Issue where its implementation got to.
    ///
    /// The Issue's conversation is the place the human follows work they asked
    /// for, and an implementation is a different conversation entirely — so an
    /// outcome that needs them (done, blocked, failed, merged, abandoned) is
    /// mirrored there as an event naming the implementation it came from.
    /// Progress is not mirrored: the Issue's surfaces already read where the
    /// work got to off the implementation itself.
    ///
    /// The feed's dedup rule keeps this from asking twice: while an
    /// implementation is live the Issue has no row of its own, so a mirrored
    /// outcome makes exactly one entry unread.
    pub(in crate::app) fn mirror_run_outcome_to_issue(
        &mut self,
        run_id: &str,
        issue_id: &str,
        event: crate::thread::ThreadEventKind,
        summary: String,
    ) -> Result<(), String> {
        if !run_outcome_mirrors_to_issue(event) {
            return Ok(());
        }
        let Ok(mut issue) = self.take_plan(issue_id) else {
            return Ok(());
        };
        issue.agents.sole_thread_mut().push_event_with_links(
            event,
            Some(summary),
            None,
            None,
            vec![crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            }],
            now_rfc3339(),
        );
        self.finish_plan_mutation(issue_id.to_string(), issue)
    }

    /// Tell an issue that the branch implementing it is gone, and that nothing
    /// was merged out of it.
    ///
    /// The issue is about to come BACK to the inbox — the branch was what had
    /// been speaking for it — and a row that reappears with no explanation
    /// reads as the list losing track of its own work. So the conversation
    /// records what happened, naming the branch, in the one place the user will
    /// look when they wonder why this is in front of them again.
    ///
    /// Attention-class on purpose: the issue needs somebody to decide what
    /// happens to it next, which is the definition of unread.
    pub(in crate::app) fn note_implementation_abandoned(
        &mut self,
        issue_id: &str,
        run_id: &str,
        branch: &str,
        how: &str,
    ) {
        let Ok(mut issue) = self.take_plan(issue_id) else {
            return;
        };
        let worktree_id = self
            .runs
            .get(run_id)
            .map(|run| crate::worktree::external_worktree_id(&run.worktree.path));
        let mut links = vec![crate::thread::ThreadLink::Implementation {
            issue_id: issue_id.to_string(),
            implementation_id: run_id.to_string(),
        }];
        if let Some(worktree_id) = worktree_id {
            links.push(crate::thread::ThreadLink::Worktree { worktree_id });
        }
        issue.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::Abandoned,
            Some(abandoned_branch_summary(branch, how)),
            None,
            None,
            links,
            now_rfc3339(),
        );
        let persisted = self.finish_plan_mutation(issue_id.to_string(), issue);
        if let Err(error) = persisted {
            eprintln!("{issue_id}: could not record the abandoned branch {branch}: {error}");
        }
    }
}
