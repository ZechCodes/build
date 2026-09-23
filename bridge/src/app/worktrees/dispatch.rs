use crate::app::{
    has_agent_choice, model_choice_from, require_str, AppState, PendingAgentTurn, RouteRecorded,
    RoutedCapture, TurnText, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::lifecycle::holders::ProjectCheckouts;
#[cfg(test)]
use crate::lifecycle::{fail_dispatch_at, BranchDispatchStep};
use crate::lifecycle::{DispatchCheckout, DispatchTarget, PendingRow};
use crate::models::ModelChoice;
use crate::orchestrator::ActiveRun;
use crate::store::now_rfc3339;
use serde_json::{json, Value};

/// The words one dispatch hands over, and who is handing them over.
///
/// A dispatch a router decided carries the router's identity onto the message
/// it lands as, so the agent reading it — and the human watching — can tell an
/// instruction passed between agents from the user speaking. A dispatch the
/// human made carries nothing: the words are their own.
pub(in crate::app) struct DispatchInstruction {
    pub(in crate::app) words: String,
    pub(in crate::app) from_agent: Option<crate::thread::AgentIdentity>,
    pub(in crate::app) agent_name: Option<String>,
}

impl DispatchInstruction {
    /// The instruction as the route that asked for it leaves it.
    pub(in crate::app) fn routed(words: String, routed: Option<&RoutedCapture>) -> Self {
        Self {
            words,
            from_agent: routed.and_then(|routed| routed.from_agent.clone()),
            agent_name: routed.and_then(|routed| routed.agent_name.clone()),
        }
    }
}

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
    pub(crate) fn branch_dispatch(&mut self, params: &Value) -> Result<Value, String> {
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
        let mut routed = routed;
        if let Some(route) = routed.as_mut().filter(|route| route.from_agent.is_some()) {
            let name = route
                .agent_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or("Build cannot start an agent without a name.")?;
            route.agent_name = Some(crate::agent::agent_name_from(name)?);
        }
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
            model_choice: requested_choice.clone(),
            resolved: self.resolved_isolation(&project_id),
            #[cfg(test)]
            fault: self.dispatch_fault,
        };
        let row = PendingRow::creating(
            mutation.run_id.clone(),
            Some(project_id.clone()),
            branch.unwrap_or_else(|| instruction.clone()),
        )
        .on_branch(mutation.target.branch().to_string())
        .isolated_as(mutation.resolved.isolation);
        let checkouts = mutation.checkouts.clone();
        self.defer_lifecycle(
            row,
            mutation,
            crate::app::runtime::lifecycle::DispatchSettlement {
                project_id,
                instruction,
                model_choice: requested_choice,
                explicit_choice: has_agent_choice(params),
                routed,
                checkouts,
            },
        )
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
        dispatched: crate::lifecycle::DispatchedCheckout,
        instruction: String,
        routed: Option<RoutedCapture>,
        checkouts: ProjectCheckouts,
    ) -> Result<Value, String> {
        let crate::lifecycle::DispatchedCheckout {
            adoption: adopted,
            downgrade,
        } = dispatched;
        self.validate_checkout_snapshot(&adopted.project_id, &checkouts)?;
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Open)?;
        let project_id = adopted.project_id.clone();
        let run_id = adopted.run_id.clone();
        let choice = adopted.model_choice.clone();
        let instruction = DispatchInstruction::routed(instruction, routed.as_ref());
        let human_dispatched = instruction.from_agent.is_none();
        let route =
            self.record_dispatch_route(routed, &project_id, &run_id, &adopted.checkout.branch)?;
        let mut active = self.open_adoption(&adopted)?;
        let agent = self.dispatch_to_run(
            &run_id,
            &mut active,
            instruction,
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
        // Only the human's own dispatch is the human acting on this run. One
        // agent handing work to another is the work happening, and the work
        // happening must not tell the inbox they have been here.
        if human_dispatched {
            self.touch_attention(&run_id);
        }
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
        project_id: String,
        joined: crate::lifecycle::JoinedCheckout,
        instruction: String,
        choice: ModelChoice,
        routed: Option<RoutedCapture>,
    ) -> Result<Value, String> {
        let crate::lifecycle::JoinedCheckout {
            run_id,
            branch,
            root,
        } = joined;
        let instruction = DispatchInstruction::routed(instruction, routed.as_ref());
        let human_dispatched = instruction.from_agent.is_none();
        let route = self.record_dispatch_route(routed, &project_id, &run_id, &branch)?;
        let mut active = self.take_run(&run_id)?;
        let agent = self.dispatch_to_run(&run_id, &mut active, instruction, choice, &branch, root);
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Settle)?;
        self.finish_run_mutation(run_id.to_string(), active)?;
        // The human acting, or an agent handing work over: see
        // `open_dispatched_run`.
        if human_dispatched {
            self.touch_attention(&run_id);
        }
        Ok(RouteRecorded::answer(
            route,
            agent.json(&project_id, &run_id),
        ))
    }

    /// Add the agent a dispatch speaks through to a run that has a checkout,
    /// and hand it the words — as the human's, or as the words of the agent
    /// that decided to send them. The half every dispatch shares — the branch Build
    /// already ran, and the one it has just taken ownership of.
    ///
    /// The run is mutated where its owner is holding it, and handed back
    /// unwritten: whoever took it out of the map is the one that puts it back,
    /// in the single write that settles the dispatch.
    pub(in crate::app) fn dispatch_to_run(
        &mut self,
        run_id: &str,
        active: &mut ActiveRun,
        instruction: DispatchInstruction,
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
        if let Some(name) = instruction.agent_name {
            agent.name = Some(name);
            agent.name_asked = true;
        }
        let model_choice = agent.choice.clone();
        let choice_revision = agent.choice_revision;
        let conversation_id = agent.conversation_id().to_string();
        match instruction.from_agent {
            Some(sender) => agent
                .thread
                .post_user_from_agent(instruction.words, sender, &now),
            None => agent.thread.post_user(instruction.words, None, &now),
        };
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
