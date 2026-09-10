use crate::app::{
    has_agent_choice, model_choice_from, require_str, AppState, PendingAgentTurn, RouteRecorded,
    RoutedCapture, RunAdopted, TurnText, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::lifecycle::holders::ProjectCheckouts;
#[cfg(test)]
use crate::lifecycle::{fail_dispatch_at, BranchDispatchStep};
use crate::lifecycle::{DispatchCheckout, DispatchTarget, LifecycleEpilogue, PendingRow};
use crate::models::ModelChoice;
use crate::orchestrator::ActiveRun;
use crate::store::now_rfc3339;
use serde_json::{json, Value};

/// The agent one dispatch put on a branch, and the branch it is working.
pub(in crate::app) struct DispatchedAgent {
    pub(in crate::app) branch: String,
    pub(in crate::app) agent_id: String,
}

impl DispatchedAgent {
    /// What every dispatch answers with, whichever way it reached its run.
    pub(in crate::app) fn json(&self, project_id: &str, run_id: &str) -> Value {
        json!({
            "project_id": project_id,
            "branch": self.branch,
            "run_id": run_id,
            "agent_id": self.agent_id,
        })
    }
}

/// `branch.dispatch`'s apply half: the checkout is checkpointed and scaffolded,
/// and the run that owns it — with the agent that will hear the instruction —
/// is opened here, under the mutex, where the records live.
pub struct BranchDispatched {
    pub adopted: RunAdopted,
    pub instruction: String,
    pub routed: Option<RoutedCapture>,
    pub checkouts: ProjectCheckouts,
    /// [`ResolvedIsolation::downgrade`], said on the dispatched run's own
    /// conversation.
    pub downgrade: Option<String>,
}

impl LifecycleEpilogue for BranchDispatched {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.open_dispatched_run(*self)
    }
}

/// The holder read found an existing run; validate that snapshot before joining.
pub struct BranchJoined {
    pub project_id: String,
    pub run_id: String,
    pub branch: String,
    pub root: std::path::PathBuf,
    pub instruction: String,
    pub model_choice: ModelChoice,
    pub explicit_choice: bool,
    pub routed: Option<RoutedCapture>,
    pub checkouts: ProjectCheckouts,
}

impl LifecycleEpilogue for BranchJoined {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.validate_checkout_snapshot(&self.project_id, &self.checkouts)?;
        #[cfg(test)]
        fail_dispatch_at(state.dispatch_fault, BranchDispatchStep::Post)?;
        let choice = if self.explicit_choice {
            self.model_choice.clone()
        } else {
            state.entity_model_choice(&self.run_id)?
        };
        state.join_dispatched_run(*self, choice)
    }
}

impl AppState {
    /// `branch.dispatch` — one call from "here is what I want done" to an agent
    /// doing it (Decisions §Capture and router, "One-call dispatch").
    ///
    /// The router decides a destination and then has to reach it, and reaching
    /// it is four mutations that each create something: cut or find the
    /// checkout, take ownership of it, put an agent on it, hand that agent the
    /// words. A caller driving those one at a time owns the unwinding when the
    /// third fails — and the router is an agent, which is the worst possible
    /// owner for a half-built branch. So the four are one verb, and the verb
    /// owns the unwinding.
    ///
    /// `branch` names where the work goes; with none, the instruction names the
    /// branch it cuts. The agent is always brand new: an instruction is never
    /// dropped into a conversation someone else is having.
    pub(in crate::app) fn branch_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        self.dispatch_branch(params, None)
    }

    /// `branch.dispatch`, and the capture it is the destination of when a route
    /// is what asked for it. Every dispatch runs its git through the drain: a
    /// router reaching a branch is the same verb as a browser dispatching one.
    pub(in crate::app) fn dispatch_branch(
        &mut self,
        params: &Value,
        routed: Option<RoutedCapture>,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        if !self.projects.iter().any(|project| project.id == project_id) {
            return Err(format!("branch.dispatch: unknown project_id: {project_id}"));
        }
        let instruction = require_str(params, "instruction")?;
        if instruction.trim().is_empty() {
            return Err(
                "branch.dispatch: the instruction is empty — there is nothing to dispatch"
                    .to_string(),
            );
        }
        let branch = params
            .get("branch")
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string);
        // The provider is parsed before anything is created, so an unrunnable
        // one refuses instead of leaving a branch nothing can work on.
        let requested_choice = model_choice_from(params, self.default_harness)?;
        // The ref, before anything is reserved or cut: it is what one dispatch
        // reserves against another, and deriving it in the git phase left two
        // calls on one instruction racing into `git worktree add`.
        let target = DispatchTarget::of(branch.as_deref(), &instruction)?;

        let mutation = DispatchCheckout {
            project: self.orch_for(&project_id)?.clone(),
            base_branch: self.base_for(&project_id)?,
            checkouts: self.project_checkouts(&project_id)?,
            project_id: project_id.clone(),
            run_id: format!("run-{}", uuid::Uuid::new_v4()),
            target,
            instruction: instruction.clone(),
            model_choice: requested_choice,
            explicit_choice: has_agent_choice(params),
            routed,
            resolved: self.resolved_isolation(&project_id),
            #[cfg(test)]
            fault: self.dispatch_fault,
        };
        let row = PendingRow::creating(
            mutation.run_id.clone(),
            Some(project_id),
            branch.unwrap_or(instruction),
        )
        .on_branch(mutation.target.branch().to_string())
        .isolated_as(mutation.resolved.isolation);
        self.defer_lifecycle(row, Box::new(mutation))
    }

    /// Open the run `branch.dispatch` just checkpointed a checkout for, and put
    /// its agent to work — the apply half of [`BranchDispatched`], and the only
    /// half that touches state.
    ///
    /// One store write, made after every decision: the git has already cut a
    /// branch, checked the repository out into it and written a checkpoint
    /// commit, so a second fallible step here would be a way to strand all of
    /// that under no run at all. The route a capture took to get here is one
    /// of those decisions, and is written first: nothing that can refuse sits
    /// on the far side of the write.
    pub(in crate::app) fn open_dispatched_run(
        &mut self,
        dispatched: BranchDispatched,
    ) -> Result<Value, String> {
        let BranchDispatched {
            adopted,
            instruction,
            routed,
            checkouts,
            downgrade,
        } = dispatched;
        self.validate_checkout_snapshot(&adopted.project_id, &checkouts)?;
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Open)?;
        let project_id = adopted.project_id.clone();
        let run_id = adopted.run_id.clone();
        let choice = adopted.model_choice.clone();
        let route =
            self.record_dispatch_route(routed, &project_id, &run_id, &adopted.checkout.branch)?;
        let mut active = adopted.open_run(self)?;
        let agent = self.dispatch_to_run(
            &run_id,
            &mut active,
            &instruction,
            choice,
            &adopted.checkout.branch,
            adopted.checkout.path.clone(),
        );
        if let Some(reason) = downgrade {
            self.note_isolation_downgrade(&run_id, &mut active, &reason);
        }
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Settle)?;
        self.finish_run_mutation(run_id.clone(), active)?;
        self.touch_attention(&run_id);
        Ok(RouteRecorded::answer(
            route,
            agent.json(&project_id, &run_id),
        ))
    }

    /// `branch.dispatch` onto a branch Build already runs: the run is there,
    /// its checkout is there, and no git runs at all — so the run is taken,
    /// told, and put back under this one acquisition. The route is written
    /// before the run is taken, so a refused route leaves the run in its map.
    pub(in crate::app) fn join_dispatched_run(
        &mut self,
        joined: BranchJoined,
        choice: ModelChoice,
    ) -> Result<Value, String> {
        let BranchJoined {
            project_id,
            run_id,
            branch,
            instruction,
            root,
            routed,
            ..
        } = joined;
        let route = self.record_dispatch_route(routed, &project_id, &run_id, &branch)?;
        let mut active = self.take_run(&run_id)?;
        let agent = self.dispatch_to_run(&run_id, &mut active, &instruction, choice, &branch, root);
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Settle)?;
        self.finish_run_mutation(run_id.to_string(), active)?;
        self.touch_attention(&run_id);
        Ok(RouteRecorded::answer(
            route,
            agent.json(&project_id, &run_id),
        ))
    }

    /// Add the agent a dispatch speaks through to a run that has a checkout,
    /// and hand it the words. The half every dispatch shares — the branch Build
    /// already ran, and the one it has just taken ownership of.
    ///
    /// The run is mutated where its owner is holding it, and handed back
    /// unwritten: whoever took it out of the map is the one that puts it back,
    /// in the single write that settles the dispatch.
    pub(in crate::app) fn dispatch_to_run(
        &mut self,
        run_id: &str,
        active: &mut ActiveRun,
        instruction: &str,
        choice: ModelChoice,
        branch: &str,
        root: std::path::PathBuf,
    ) -> DispatchedAgent {
        let now = now_rfc3339();
        // A dispatch always adds the agent it is about to speak to — an
        // adoption mints none, and a branch Build already runs keeps the agents
        // it has.
        let agent_id = active.agents.add(run_id, choice, &now).id.clone();
        let branch = branch.to_string();
        let agent = active
            .agents
            .resolve_mut(Some(&agent_id))
            .expect("the agent was just put on this roster");
        let model_choice = agent.choice.clone();
        let choice_revision = agent.choice_revision;
        let conversation_id = agent.conversation_id().to_string();
        agent.thread.post_user(instruction, None, &now);
        // Told the same way `agent.start` tells an agent what is waiting for
        // it: the instruction is already durable on the thread, so a warm
        // harness gets the read-your-messages nudge `thread.post` writes, and a
        // cold one gets that nudge wrapped in the packet it has no other way to
        // reconstruct. The spawn itself happens in `DeliveryRunner`, with the
        // state lock free and this frame already answered.
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root,
            owner: run_id.to_string(),
            agent_id: agent_id.clone(),
            conversation_id,
            model_choice,
            choice_revision,
            interrupt: false,
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "dispatch",
            wants_catch_up: true,
            survives_refusal: false,
        });
        DispatchedAgent { branch, agent_id }
    }
}
