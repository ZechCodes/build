use crate::app::WorktreeLifecycleJob;
use crate::app::{
    abandoned_branch_summary, close_abandoned_run_conversations, err, has_agent_choice,
    model_choice_from, parse_viewing_context, reconcile_missing_run_worktree,
    record_current_stage_started, require_str, run_state_str, thread_detail, AppState, DigestScope,
    ImplementationCaller, PendingAgentTurn, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::lifecycle::{
    AdoptCheckout, AdoptImplementation, AdoptionTarget, DiscardCheckout, DiscardedCheckout,
    ImplementationCheckout, OpenImplementation, PendingRow,
};
use crate::models::ModelChoice;
use crate::orchestrator::{ActiveRun, AdoptionScope, AgentTurn, ImplementableIssue, RunSource};
use crate::run::{run_transition, RunEvent, RunId, RunState};
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

/// A run opened around a checkout that is ready for it: the record, the agent
/// its first turn is addressed to, and what the Issue's conversation says about
/// where the work went.
pub(in crate::app) struct OpenedImplementation {
    pub(in crate::app) run_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) issue_id: String,
    pub(in crate::app) active: ActiveRun,
    pub(in crate::app) turn: AgentTurn,
    pub(in crate::app) agent_id: String,
    pub(in crate::app) checkout_event: crate::thread::ThreadEventKind,
    pub(in crate::app) checkout_summary: String,
}

/// `run.create` asked: it hears the run it opened, and a failure is its own
/// answer — nothing was armed on the way in.
pub(in crate::app) struct RunOpenedView {
    pub(in crate::app) detail: ThreadDetail,
}

impl ImplementationCaller for RunOpenedView {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String> {
        let active = state.runs.get(run_id).ok_or("unknown run_id")?;
        Ok(state.run_view(run_id, active, self.detail, DigestScope::Detail))
    }

    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> String {
        error
    }
}

impl AppState {
    pub(in crate::app) fn open_adoption(
        &mut self,
        adopted: &crate::lifecycle::AdoptionPrepared,
    ) -> Result<ActiveRun, String> {
        let active = self
            .orch_for(&adopted.project_id)?
            .adopt_run(
                RunId::new(&adopted.run_id),
                &adopted.checkout,
                &adopted.base_branch,
                adopted.model_choice.clone(),
            )
            .map_err(err)?;
        self.projects
            .bind_entity(adopted.run_id.clone(), adopted.project_id.clone());
        let (was_dismissed, first_observed_at) = self.take_row_dismissal(
            &adopted.project_id,
            Some(&adopted.checkout.branch),
            adopted.scope == AdoptionScope::PrimaryCheckout,
        );
        if was_dismissed || first_observed_at.is_some() {
            self.board.attention_mut().transfer_adopted_row(
                &adopted.run_id,
                first_observed_at,
                was_dismissed,
            );
            self.persist_attention();
        }
        self.note_worktree_gone(&adopted.project_id, &adopted.checkout.path);
        Ok(active)
    }
}

impl AppState {
    // ---- Run surface ----------------------------------------------------------

    /// Create a run implementing an approved plan. Single-active-writer: a
    /// second concurrent run of the same plan is rejected at dispatch.
    ///
    /// A run always has a plan behind it: the goal-only dispatch is gone, and an
    /// unplanned coding session is now an agent tab (`term.create` with `kind`),
    /// driven by the human who opened it.
    pub(in crate::app) fn run_create(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let job = self.open_implementation(
            &plan_id,
            params,
            Box::new(RunOpenedView {
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// Settle everything an Issue's implementation needs before any git runs —
    /// the run's id, the checkout it works in, the model its agent runs on —
    /// and hand the git itself to the drain.
    ///
    /// Shared by `run.create` and by the Issue scheduler, which differ only in
    /// `caller`: who is waiting for the run, and what a failure leaves written
    /// on the Issue.
    pub(in crate::app) fn open_implementation(
        &mut self,
        issue_id: &str,
        params: &Value,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<WorktreeLifecycleJob, String> {
        // Targeting: an Issue can be implemented into a checkout that already
        // exists instead of one cut for it (Decisions §Issue view — the stage
        // column's assignment control).
        if let Some(worktree_id) = params
            .get("worktree_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            let worktree_id = worktree_id.to_string();
            return self.adopt_implementation_checkout(issue_id, &worktree_id, params, caller);
        }
        if !self.plans.contains_key(issue_id) {
            return Err("unknown plan_id".to_string());
        }
        let project_id = self.project_of(issue_id)?;
        let requested_choice = model_choice_from(params, self.default_harness)?;
        let base = params
            .get("base_branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.plans[issue_id].base_branch.clone());
        let has_active_run = self.runs.values().any(|r| {
            r.run.plan_id.as_ref().map(|p| p.0.as_str()) == Some(issue_id)
                && !r.run.state.is_terminal()
        });
        let store = self.require_store()?.clone();
        let plan = &self.plans[issue_id];
        let model_choice = if has_agent_choice(params) {
            requested_choice
        } else {
            plan.model_choice.clone()
        };
        let title = plan.plan.goal.clone();
        let issue = ImplementableIssue::judge(RunSource {
            plan,
            has_active_run,
        })
        .map_err(err)?;
        let project = self.orch_for(&project_id)?.clone();
        let resolved = self.resolved_isolation(&project_id);
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        // The ref this implementation is about to cut is on the row, so a
        // create or a dispatch claiming the same one collides here rather than
        // in git.
        let row = PendingRow::creating(run_id.clone(), Some(project_id.clone()), title)
            .on_branch(crate::worktree::branch_name_for(issue.slug()))
            .implementing(issue_id.to_string())
            .isolated_as(resolved.isolation);
        self.reserve_lifecycle(
            row,
            OpenImplementation {
                project,
                issue,
                base_branch: base,
                run_id: run_id.clone(),
                store,
                model_choice: model_choice.clone(),
                resolved,
            },
            crate::app::runtime::lifecycle::OpenImplementationSettlement {
                project_id,
                issue_id: issue_id.to_string(),
                run_id,
                model_choice,
                caller,
            },
        )
    }

    /// Implement an Issue into a checkout that already exists, named by the
    /// `worktree_id` the feed's branch rows carry.
    ///
    /// The branch's run adopts the implementation — one branch, one run — so a
    /// checkout Build has never seen is adopted first, and a branch already
    /// implementing a DIFFERENT Issue is refused: two Issues writing one branch
    /// would make neither one's diff readable.
    pub(in crate::app) fn adopt_implementation_checkout(
        &mut self,
        issue_id: &str,
        worktree_id: &str,
        params: &Value,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<WorktreeLifecycleJob, String> {
        if !self.plans.contains_key(issue_id) {
            return Err("unknown plan_id".to_string());
        }
        let project_id = self.project_of(issue_id)?;
        let requested_choice = model_choice_from(params, self.default_harness)?;
        let (run_id, checkout) = match self.run_owning_worktree_id(&project_id, worktree_id) {
            Some(run_id) => {
                // The primary checkout is the repository, not a worktree to
                // hand an Issue: committing stage docs there lands them on the
                // branch the human is standing on.
                if self.owns_primary_checkout(&run_id, &self.runs[&run_id]) {
                    return Err(
                        "cannot implement into the primary checkout — it is the repository, not a \
                         worktree to hand over"
                            .to_string(),
                    );
                }
                if let Some(other) = self.runs[&run_id]
                    .run
                    .plan_id
                    .as_ref()
                    .filter(|id| id.0 != issue_id)
                {
                    return Err(format!(
                        "cannot implement into {}: it is already implementing Issue {} — finish \
                         or abandon that implementation first",
                        self.runs[&run_id].worktree.branch(),
                        other.0
                    ));
                }
                let checkout = self.runs[&run_id].worktree.path.clone();
                (run_id, ImplementationCheckout::Owned(checkout))
            }
            // Nobody owns it yet, so this implementation's git takes it over
            // first — the scan that resolves the card and the checkpoint commit
            // that writes Build's ownership into it, both off the lock, and the
            // run they mint is the one the implementation is written onto.
            None => (
                format!("run-{}", uuid::Uuid::new_v4()),
                ImplementationCheckout::Unowned {
                    target: AdoptionTarget::Card {
                        worktree_id: worktree_id.to_string(),
                        excluded: self.bound_worktree_paths(),
                    },
                    base_branch: self.base_for(&project_id)?,
                },
            ),
        };

        let has_active_run = self.runs.iter().any(|(id, run)| {
            id != &run_id
                && run.run.plan_id.as_ref().map(|p| p.0.as_str()) == Some(issue_id)
                && !run.run.state.is_terminal()
        });
        let model_choice = if has_agent_choice(params) {
            requested_choice
        } else {
            self.plans[issue_id].model_choice.clone()
        };
        let store = self.require_store()?.clone();
        let plan = &self.plans[issue_id];
        let title = plan.plan.goal.clone();
        let issue = ImplementableIssue::judge(RunSource {
            plan,
            has_active_run,
        })
        .map_err(err)?;
        let project = self.orch_for(&project_id)?.clone();
        // The run stays on the board while its checkout is checkpointed: it is
        // the same run either way, and a run that vanished from every poll for
        // the length of two commits would read as one that had been abandoned.
        // What the row holds is the checkout, which nothing else may claim
        // until this hand-over is written down.
        let row = PendingRow::creating(run_id.clone(), Some(project_id.clone()), title)
            .on_checkout(worktree_id.to_string())
            .implementing(issue_id.to_string());
        self.reserve_lifecycle(
            row,
            AdoptImplementation {
                project,
                project_id: project_id.clone(),
                issue,
                run_id: run_id.clone(),
                checkout,
                store,
                model_choice: model_choice.clone(),
            },
            crate::app::runtime::lifecycle::AdoptImplementationSettlement {
                project_id,
                issue_id: issue_id.to_string(),
                run_id,
                model_choice,
                caller,
            },
        )
    }

    /// `run.create`'s apply half on a checkout that was cut for it: open the
    /// run around what the git prepared, and answer whoever asked.
    pub(in crate::app) fn open_prepared_implementation(
        &mut self,
        project_id: String,
        issue_id: String,
        run_id: String,
        opened: crate::lifecycle::ImplementationPrepared,
        model_choice: ModelChoice,
    ) -> Result<(), String> {
        let crate::lifecycle::ImplementationPrepared {
            prepared,
            downgrade,
        } = opened;
        let opened = (|| -> Result<OpenedImplementation, String> {
            let plan = self.plans.get(&issue_id).ok_or("unknown plan_id")?;
            let conversation_id = plan.agents.sole().conversation_id().to_string();
            let (mut active, turn) = self
                .orch_for(&project_id)?
                .open_prepared_run(RunId::new(&run_id), plan, prepared, model_choice)
                .map_err(err)?;
            active
                .agents
                .primary_mut()
                .expect("a dispatched run opens with its agent")
                .bind_conversation(conversation_id);
            let agent_id = active
                .agents
                .primary()
                .expect("a dispatched run opens with its agent")
                .id
                .clone();
            Ok(OpenedImplementation {
                checkout_summary: format!("Created the Issue implementation worktree for {run_id}"),
                run_id: run_id.clone(),
                project_id,
                issue_id,
                active,
                turn,
                agent_id,
                checkout_event: crate::thread::ThreadEventKind::WorktreeCreated,
            })
        })()
        .and_then(|mut opened| {
            if let Some(reason) = downgrade {
                self.note_isolation_downgrade(&run_id, &mut opened.active, &reason);
            }
            self.open_implementation_run(opened)
        });
        opened
    }

    /// The same, on a checkout an existing run already owns: the run is taken
    /// out, handed the implementation the git prepared it for, and put back.
    pub(in crate::app) fn open_adopted_implementation(
        &mut self,
        project_id: String,
        issue_id: String,
        run_id: String,
        opened: crate::lifecycle::AdoptedImplementation,
        model_choice: ModelChoice,
    ) -> Result<(), String> {
        let crate::lifecycle::AdoptedImplementation { base_sha, adopted } = opened;
        let opened = (|| -> Result<(), String> {
            // The run this is written onto: the one the branch already had, or
            // the one the adoption in this job's git phase just earned.
            let mut active = match &adopted {
                Some(adopted) => self.open_adoption(adopted)?,
                None => self.take_run(&run_id)?,
            };
            let handed_over = (|| -> Result<(AgentTurn, String), String> {
                let plan = self.plans.get(&issue_id).ok_or("unknown plan_id")?;
                self.orch_for(&project_id)?
                    .open_adopted_implementation(&mut active, plan, base_sha, model_choice)
                    .map_err(err)
            })();
            let (turn, agent_id) = match handed_over {
                Ok(opened) => opened,
                Err(error) => {
                    // Nothing was handed over. The branch keeps the run it had
                    // — or, when this job earned it one, keeps the plain
                    // adopted run its checkout is now Build's under.
                    let put_back = self.finish_run_mutation(run_id.clone(), active);
                    return Err(match put_back {
                        Ok(()) => error,
                        Err(store) => format!("{error}; and the run could not be saved: {store}"),
                    });
                }
            };
            let branch = active.worktree.branch();
            self.open_implementation_run(OpenedImplementation {
                checkout_summary: format!("Implementing into the existing checkout on {branch}"),
                run_id: run_id.clone(),
                project_id,
                issue_id,
                active,
                turn,
                agent_id,
                checkout_event: crate::thread::ThreadEventKind::WorktreeReused,
            })
        })();
        opened
    }

    /// The tail every implementation dispatch shares: address the first turn to
    /// the agent that will hear it, drive it under QA, persist the run, and
    /// record on the Issue's conversation which checkout the work went into.
    ///
    /// The answer is not built here — who asked is what decides that, and by
    /// this point they are as far apart as `run.create` and a scheduled stage.
    pub(in crate::app) fn open_implementation_run(
        &mut self,
        opened: OpenedImplementation,
    ) -> Result<(), String> {
        let OpenedImplementation {
            run_id,
            project_id,
            issue_id,
            mut active,
            turn,
            agent_id,
            checkout_event,
            checkout_summary,
        } = opened;
        let plan_docs = self.owning_plan_stage_docs(&active);

        self.projects
            .bind_entity(run_id.clone(), project_id.clone());
        self.delivery_queue.enqueue(PendingAgentTurn::for_run_agent(
            &run_id, &agent_id, &active, turn,
        ));
        if self.qa_agent {
            self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
        }
        let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
        // The checkout belongs to a run from here on, so it leaves the unbound
        // list its mutation named it on. A checkout that was already bound was
        // never in that list, so this is routinely a no-op.
        let checkout = active.worktree.path.clone();
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        self.note_worktree_gone(&project_id, &checkout);
        let mut plan = self.take_plan(&issue_id)?;
        let implementation_link = crate::thread::ThreadLink::Implementation {
            issue_id: issue_id.clone(),
            implementation_id: run_id.clone(),
        };
        plan.agents.sole_thread_mut().push_event_with_links(
            checkout_event,
            Some(checkout_summary),
            None,
            None,
            vec![
                implementation_link.clone(),
                crate::thread::ThreadLink::Worktree { worktree_id },
            ],
            now_rfc3339(),
        );
        plan.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::ImplementationStarted,
            Some(format!("Implementation started as {run_id}")),
            None,
            None,
            vec![implementation_link],
            now_rfc3339(),
        );
        if let Some(run) = self.runs.get(&run_id) {
            record_current_stage_started(plan.agents.sole_thread_mut(), run, &plan_docs);
        }
        let plan_persisted = self.finish_plan_mutation(issue_id, plan);
        plan_persisted?;
        self.auto_advance_run(&run_id);
        Ok(())
    }

    /// A freeform message to the run's agent — redirects a live session or
    /// resumes a parked one.
    pub(in crate::app) fn run_message(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let message = require_str(params, "message")?;
        let viewing_context = parse_viewing_context(params.get("viewing_context"))?;
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        // See `plan_message`: the user is speaking, so the anchor may move.
        self.note_user_message(&run_id);
        let agent_id = self.ensure_primary_agent(&run_id)?;
        self.edit_agent_conversation(&run_id, &agent_id, |thread, _artifact| {
            thread.post_user_with_context(&message, None, viewing_context, now_rfc3339());
            Ok(())
        })?;
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .message_run(&mut active, &plan_docs, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.delivery_queue
                .enqueue(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            if self.qa_agent && active.run.state == RunState::Building {
                self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.answer_run_mutation(run_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Abandon a run: end it, take its agents down, and take back the checkout
    /// it was working in — the directory, never the branch, because a run's
    /// work outlives the run so it can be re-attempted.
    ///
    /// Every refusal is spent here, before anything is torn down. What the
    /// drain runs is git that cannot fail the verb: the stage publications the
    /// removal is about to make unreadable, the wait for the agents to die, and
    /// the removal itself.
    pub(in crate::app) fn run_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        // Judged before a single agent is killed: an abandon that is not legal
        // must leave the run exactly as it found it.
        run_transition(&active.run.state, RunEvent::Abandon)
            .map_err(|illegal| illegal.to_string())?;
        // Abandoning removes the run's worktree — which for a primary run is
        // the repository. That run ends by letting go of the checkout instead.
        let keeps_checkout = self.owns_primary_checkout(&run_id, active);
        let title = active.run.goal.clone();
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let stages = self.stage_publication_query(&run_id, active);
        let project = self.orch_for(&project_id)?.clone();
        let detail = thread_detail(params);
        let settlement_run_id = run_id.clone();
        let settlement_project_id = project_id.clone();
        self.discard_run(
            run_id,
            Some(project_id),
            title,
            move |worktree| match keeps_checkout {
                true => DiscardedCheckout::Kept,
                false => DiscardedCheckout::Removed {
                    project,
                    worktree: worktree.clone(),
                },
            },
            stages,
            move |active| crate::app::runtime::lifecycle::AbandonSettlement {
                active,
                run_id: settlement_run_id,
                project_id: settlement_project_id,
                issue_id,
                detail,
            },
        )
    }

    /// Take one run off the board and let go of the checkout it was working in.
    ///
    /// `run.abandon` and `run.delete` differ in three things: what happens to
    /// the directory, what is still owed the records once it is gone, and what
    /// the row standing in the run's place is called. Everything around
    /// them — the row, the run coming out of the map, the stat the board
    /// cached, the agents that were writing into the directory — is the same
    /// decide phase, and it is this one.
    ///
    /// The caller's refusals are all spent before it gets here: `take` runs
    /// with the row already on the board and cannot fail.
    pub(in crate::app) fn discard_run<J, S>(
        &mut self,
        run_id: String,
        project_id: Option<String>,
        title: String,
        checkout: impl FnOnce(&crate::worktree::Worktree) -> DiscardedCheckout,
        before_removal: J,
        settlement: impl FnOnce(Box<ActiveRun>) -> S,
    ) -> Result<Value, String>
    where
        J: crate::lifecycle::BeforeRemoval,
        S: crate::app::runtime::lifecycle::LifecycleSettlement<J::Output>,
    {
        let worktree_path = self
            .runs
            .get(&run_id)
            .ok_or("unknown run_id")?
            .worktree
            .path
            .clone();
        let row = PendingRow::discarding(run_id.clone(), project_id, title).on_checkout(
            crate::worktree::external_worktree_id(&Self::canonical_root(&worktree_path)),
        );
        self.defer_lifecycle_holding(row, move |state| {
            let active = state
                .runs
                .remove(&run_id)
                .expect("the run was read out of the map above");
            state.invalidate_run_stat(&run_id);
            // A human who took a run off the board must not keep paying for the
            // agent that was working on it, so the kill is explicit — and the
            // removal waits it out rather than walking a directory a live child
            // is still writing into.
            let retirements = state.retire_agent_tabs(&active.worktree.path);
            let task = DiscardCheckout {
                checkout: checkout(&active.worktree),
                retirements,
                before_removal,
                run_id,
            };
            let settlement = settlement(Box::new(active));
            (task, settlement)
        })
    }

    /// Write down an abandon whose git has returned: the run's verdict, the
    /// stages the removal made unverifiable, and the Issue's lineage.
    ///
    /// The run comes back on the board here whichever way the rest goes —
    /// `answer_run_mutation` is what puts it back — so nothing below can strand
    /// it.
    pub(in crate::app) fn settle_abandoned_run(
        &mut self,
        run_id: String,
        project_id: String,
        issue_id: Option<String>,
        detail: crate::thread::ThreadDetail,
        published: crate::lifecycle::StagePublications,
        mut active: ActiveRun,
    ) -> Result<Value, String> {
        let branch = active.worktree.branch();
        let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
        let verdict = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_run_keeping_checkout(&mut active).map_err(err));
        let affected_stages = reconcile_missing_run_worktree(&mut active, &published);
        if verdict.is_ok() {
            close_abandoned_run_conversations(&mut active);
        }
        let (view, persisted) = self.answer_run_mutation(run_id.clone(), active, detail);
        verdict?;
        persisted?;
        if let Some(issue_id) = issue_id {
            self.record_abandon_on_issue(
                &issue_id,
                &run_id,
                &branch,
                worktree_id,
                &affected_stages,
            )?;
        }
        Ok(view)
    }

    /// Tell the Issue this run was implementing what it lost: the branch it was
    /// on, and the stages whose commits the removal made unverifiable.
    pub(in crate::app) fn record_abandon_on_issue(
        &mut self,
        issue_id: &str,
        run_id: &str,
        branch: &str,
        worktree_id: String,
        affected_stages: &[String],
    ) -> Result<(), String> {
        // Abandoning is deleting the branch with nothing merged out of it, so
        // the issue this was implementing comes back to the inbox — and its
        // conversation says which branch it lost and why.
        self.mirror_run_outcome_to_issue(
            run_id,
            issue_id,
            crate::thread::ThreadEventKind::Abandoned,
            abandoned_branch_summary(branch, "abandoned"),
        )?;
        let mut issue = self.take_plan(issue_id)?;
        let mut links = vec![
            crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            },
            crate::thread::ThreadLink::Worktree { worktree_id },
        ];
        links.extend(
            issue
                .stages
                .iter()
                .filter(|stage| affected_stages.contains(&stage.id))
                .map(|stage| crate::thread::ThreadLink::IssueStage {
                    issue_id: issue_id.to_string(),
                    stage_id: stage.id.clone(),
                    path: stage.path.clone(),
                }),
        );
        issue.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::WorktreeDeleted,
            Some(format!(
                "Issue worktree deleted; {} unpublished stage(s) are incomplete",
                affected_stages.len()
            )),
            None,
            None,
            links,
            now_rfc3339(),
        );
        self.finish_plan_mutation(issue_id.to_string(), issue)?;
        Ok(())
    }

    /// Delete a terminal run from the board: prune any leftover worktree,
    /// remove the durable record, drop the bookkeeping. Terminal runs only
    /// (merged/abandoned/archived/failed) — a live run must be abandoned first.
    pub(in crate::app) fn run_delete(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let state = active.run.state;
        if !matches!(
            state,
            RunState::Merged | RunState::Abandoned | RunState::Archived | RunState::Failed
        ) {
            return Err(format!(
                "run.delete: run is {} — only terminal runs \
                 (merged/abandoned/archived/failed) can be deleted",
                run_state_str(&state)
            ));
        }
        // A planned implementation is durable Issue lineage: its immutable
        // stage boundaries and publication evidence must outlive card cleanup.
        // Archived lineages are already filtered from board.list, so preserve
        // the record while keeping the legacy delete call idempotently useful.
        if active.run.plan_id.is_some() {
            return Ok(json!({ "ok": true, "retained_as_issue_lineage": true }));
        }
        let checkout_path = active.worktree.path.clone();
        // A failed run still holds its worktree; deleting an adopted run's card
        // must never delete the user's files (delete removes the card, not the
        // worktree it was minted around).
        let prunes_checkout = checkout_path.exists() && !active.adopted;
        let title = active.run.goal.clone();
        // A run recovered after its repository was moved or deleted has no
        // project mapping at all, and that stale card is exactly what a delete
        // is for. There is then no orchestrator to prune with, so the delete
        // clears the card and leaves whatever is on disk alone.
        let project_id = self.projects.project_id_of(&run_id).map(str::to_string);
        let project = project_id
            .as_deref()
            .and_then(|id| self.orch_for(id).ok())
            .cloned();

        let settlement_run_id = run_id.clone();
        let settlement_project_id = project_id.clone();
        self.discard_run(
            run_id,
            project_id,
            title,
            move |worktree| match (prunes_checkout, project) {
                (true, Some(project)) => DiscardedCheckout::Pruned {
                    project,
                    worktree: worktree.clone(),
                },
                _ => DiscardedCheckout::Kept,
            },
            (),
            move |active| crate::app::runtime::lifecycle::DeleteSettlement {
                active,
                run_id: settlement_run_id,
                project_id: settlement_project_id,
                checkout: checkout_path,
            },
        )
    }

    /// Forget every trace of a run whose record has been deleted. The map entry
    /// itself went in the decide phase; this is the bookkeeping beside it.
    pub(in crate::app) fn forget_run(&mut self, run_id: &str) {
        self.projects.unbind_entity(run_id);
        self.board.attention_mut().remove_entity_clocks(run_id);
        self.board.diff_mut().remove_run_files_changed_at(run_id);
        self.invalidate_run_stat(run_id);
    }

    /// Mint a plan-less run around an existing checkout (`plan_id` None): one of
    /// the project's external worktrees (`worktree_id`), or its primary
    /// checkout (`primary: true`) — the repo root as a super-worktree.
    ///
    /// The primary checkout has exactly one owner per project, enforced here.
    /// External adoption can rely on a client-side latch because a worktree
    /// card is adopted from one place; the repo root is reachable from every
    /// reload and every second browser, and they must all converge on the run
    /// that already owns it.
    pub(in crate::app) fn run_adopt(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let model_choice = model_choice_from(params, self.default_harness)?;
        let base = self.base_for(&project_id)?;
        let adopting_primary = params
            .get("primary")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let target = if adopting_primary {
            if let Some(run_id) = self.primary_run_of(&project_id) {
                return Ok(self.owning_run_view(&run_id, params));
            }
            AdoptionTarget::Primary {
                repo_path: self.repo_path_for(&project_id)?,
            }
        } else {
            let worktree_id = require_str(params, "worktree_id")?;
            if let Some(run_id) = self.run_owning_worktree_id(&project_id, &worktree_id) {
                return Ok(self.owning_run_view(&run_id, params));
            }
            AdoptionTarget::Card {
                worktree_id,
                excluded: self.bound_worktree_paths(),
            }
        };
        let checkout_id = target.checkout_id();
        // The repo root is reachable from every reload and every second
        // browser, and a card is adoptable from more than one surface. An asker
        // who arrives while the checkout is being taken over is told so, and
        // is handed no run id: the run that will carry it is not in the map
        // until the adoption's epilogue lands, and an adoption that fails never
        // mints it at all. The asker asks again — the same thing it does when
        // its own adopt outlived its timer — and by then the owner is real and
        // `primary_run_of` / `run_owning_worktree_id` above answer with it.
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let row = target.reserve(
            run_id.clone(),
            &project_id,
            self.checkout_title(&project_id, &checkout_id),
        );
        if self.row_claiming(&row).is_some() {
            return Ok(json!({ "adopting": true }));
        }
        let project = self.orch_for(&project_id)?.clone();
        self.defer_lifecycle(
            row,
            AdoptCheckout {
                project,
                project_id,
                base_branch: base,
                run_id,
                target,
                model_choice,
            },
            crate::app::runtime::lifecycle::AdoptionSettlement {
                detail: thread_detail(params),
            },
        )
    }

    /// The view an adopting caller gets when the checkout it named already has
    /// an owner: that run, in full.
    pub(in crate::app) fn owning_run_view(&self, run_id: &str, params: &Value) -> Value {
        let active = self
            .runs
            .get(run_id)
            .expect("the caller found this run by scanning the map");
        self.run_view(run_id, active, thread_detail(params), DigestScope::Detail)
    }

    /// What to call a checkout on the row standing in for it: the branch the
    /// last scan saw it on, or the project it belongs to when no card does.
    pub(in crate::app) fn checkout_title(&self, project_id: &str, checkout_id: &str) -> String {
        self.external_scan_of(project_id)
            .and_then(|cache| {
                cache
                    .worktrees
                    .iter()
                    .find(|checkout| checkout.id == checkout_id)
            })
            .and_then(|checkout| checkout.branch.clone())
            .unwrap_or_else(|| self.project_name_by_id(project_id))
    }

    /// Un-adopt: drop the run record and its binding, leaving every file
    /// untouched. Legal on adopted runs in any non-terminal state.
    pub(in crate::app) fn run_release(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        if !active.adopted {
            return Err("run.release: only adopted runs can be released".to_string());
        }
        if active.run.state.is_terminal() {
            return Err(format!(
                "run.release: run is {} — use run.delete to clear it off the board",
                run_state_str(&active.run.state)
            ));
        }
        if let Some(store) = &self.store {
            store
                .delete_run(&run_id)
                .map_err(|e| format!("run store: {e}"))?;
        }
        let project_id = self.projects.project_id_of(&run_id).map(str::to_string);
        let active = self.runs.remove(&run_id).expect("checked above");
        // Un-adopting hands the worktree back to the human; Build's agent in it
        // reported `done` to a run that no longer exists, so it goes with the
        // run. Reopening the Agent tab there adopts again. The receipts are
        // dropped: the tabs left the registry, which is what makes the agents
        // unaddressable, and nothing here is waiting to delete a directory.
        self.retire_agent_tabs(&active.worktree.path);
        self.forget_run(&run_id);
        // Build touches no disk here, so the checkout it hands back is
        // described by the scan this claims rather than by an amendment.
        if let Some(project_id) = project_id {
            self.rescan_external_worktrees(&project_id);
        }
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }
}
