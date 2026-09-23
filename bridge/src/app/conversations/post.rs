use super::inputs::{
    append_operation_reviewer_messages, normalize_post_viewing_contexts, optional_choice_revision,
    optional_operation_id, parse_option_choice, parse_thread_post_messages, parse_viewing_context,
    required_operation_id, with_post_receipt, PostOrigin, ReviewerMessage,
};
use super::read::thread_detail;
use super::{AppState, ConversationAddress};
use crate::app::{
    named_agent_id, plan_state_str, require_str, run_state_str, ImplementationTarget,
    PendingAgentTurn, PlanDraftingStarted, TurnText, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::operation::OperationPayload;
use crate::operation::{
    thread_post_request_hash, DeliveryIntent, OperationReceipt, OperationRequester,
    OperationStatus, THREAD_POST_METHOD,
};
use crate::plan::{PlanEvent, PlanState};
use crate::run::{RunEvent, RunState};
use serde_json::Value;

pub(in crate::app) fn post_operation_value(receipt: &OperationReceipt) -> Value {
    let mut value = receipt.wire_value();
    let object = value
        .as_object_mut()
        .expect("an operation receipt serializes as an object");
    let status = object
        .remove("status")
        .expect("an operation receipt carries status");
    object.insert("operation_status".to_string(), status);
    value
}

pub(in crate::app) fn with_operation_error(mut value: Value, error: String) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("operation_error".to_string(), Value::String(error));
    }
    value
}

impl AppState {
    /// Post a reviewer message to an entity's conversation WITHOUT dispatching
    /// work — the review-surface write path. Resolves a plan OR a run (the
    /// `thread.revision` idiom), appends the body as an unread user message,
    /// and nudges the worktree's live agent in place through its PTY so it
    /// calls `read_unread_messages` — whatever the entity is parked as, because
    /// that agent is the one the human is looking at. Never ends or spawns a
    /// session and never moves plan/run state — with no live agent the message
    /// simply waits for the next session's catch-up. Refused only where no
    /// conversation remains to post to: a terminal or unknown entity.
    pub(crate) fn thread_post(&mut self, params: &Value) -> Result<Value, String> {
        self.post_to_thread(params, PostOrigin::default())
    }

    /// `thread.post`, asked for by an agent rather than by the human.
    ///
    /// The same write, with two things added: the message wears the sender, so
    /// the agent reading it can tell a hand-off from the user speaking, and the
    /// operation remembers who asked for it.
    pub(in crate::app) fn thread_post_from_agent(
        &mut self,
        params: &Value,
        requester: OperationRequester,
    ) -> Result<Value, String> {
        let sender = self.sender_identity(&requester.entity_id, &requester.agent_id);
        self.post_to_thread(params, PostOrigin::asked_by(requester, sender))
    }

    /// `thread.post`, carrying the issue that assigning one handed over.
    ///
    /// The same write, with the envelope added — and, when an AGENT did the
    /// assigning, the sender and the requester a hand-off already carries. The
    /// human assigning an issue is the human speaking, so that case wears an
    /// envelope and no sender.
    pub(in crate::app) fn thread_post_handing_over_issue(
        &mut self,
        params: &Value,
        issue: crate::thread::IssueEnvelope,
        requester: Option<OperationRequester>,
    ) -> Result<Value, String> {
        let origin = match requester {
            Some(requester) => {
                let sender = self.sender_identity(&requester.entity_id, &requester.agent_id);
                let mut origin = PostOrigin::asked_by(requester, sender);
                origin.from_issue = Some(issue);
                origin
            }
            None => PostOrigin::handing_over(issue, None),
        };
        self.post_to_thread(params, origin)
    }

    fn post_to_thread(&mut self, params: &Value, origin: PostOrigin) -> Result<Value, String> {
        let normalized_params = normalize_post_viewing_contexts(params)?;
        let params = &normalized_params;
        let entity_id = require_str(params, "entity_id")?;
        if self.plans.contains_key(&entity_id) {
            return Err(crate::app::issues::ISSUES_RETIRED_ERROR.to_string());
        }
        if !self.plans.contains_key(&entity_id) && !self.runs.contains_key(&entity_id) {
            return Err("unknown conversation owner".to_string());
        }
        let origin = PostOrigin {
            operation_id: optional_operation_id(params)?,
            ..origin
        };
        let addressed = named_agent_id(params)?;
        if self.entity_agents(&entity_id)?.is_empty() {
            if addressed.is_some() || params.get("conversation_id").is_some() {
                return Err("thread.post: addressed agent does not exist".to_string());
            }
            self.ensure_primary_agent(&entity_id)?;
        }
        let address = self.resolve_conversation_params(&entity_id, params)?;
        if let Some(retry) =
            self.retry_thread_post(params, origin.operation_id.as_deref(), &address)?
        {
            return Ok(retry);
        }
        if let Some(expected) = optional_choice_revision(params)? {
            let current = self
                .entity_agents(&address.entity_id)?
                .resolve(Some(&address.agent_id))?
                .choice_revision;
            if expected != current {
                return Err(format!(
                    "stale choice_revision {expected}; agent {} is at {current}",
                    address.agent_id
                ));
            }
        }
        let choice = parse_option_choice(params)?;
        let interrupt = params
            .get("interrupt")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if self.plans.contains_key(&entity_id) {
            return self.thread_post_plan(params, address, origin.operation_id, choice, interrupt);
        }
        if self.runs.contains_key(&entity_id) {
            return self.thread_post_run(params, address, origin, choice, interrupt);
        }
        Err("unknown conversation owner".to_string())
    }

    pub(in crate::app) fn retry_thread_post(
        &self,
        params: &Value,
        operation_id: Option<&str>,
        address: &ConversationAddress,
    ) -> Result<Option<Value>, String> {
        let Some(operation_id) = operation_id else {
            return Ok(None);
        };
        let Some(receipt) = self.operation_receipt(operation_id)? else {
            return Ok(None);
        };
        let request_hash = thread_post_request_hash(
            params,
            &address.entity_id,
            &address.agent_id,
            &address.conversation_id,
        );
        if !receipt.matches_request(THREAD_POST_METHOD, &request_hash)
            || receipt.entity_id != address.entity_id
            || receipt.agent_id != address.agent_id
            || receipt.conversation_id != address.conversation_id
        {
            return Err("thread.post: operation_id already used with different request".into());
        }
        Ok(Some(post_operation_value(&receipt)))
    }

    pub(in crate::app) fn thread_post_plan(
        &mut self,
        params: &Value,
        address: ConversationAddress,
        operation_id: Option<String>,
        choice: Option<crate::thread::OptionChoice>,
        interrupt: bool,
    ) -> Result<Value, String> {
        let entity_id = address.entity_id.clone();
        let active = self.plans.get(&entity_id).ok_or("unknown plan_id")?;
        if active.plan.state.is_terminal() {
            return Err(format!(
                "thread.post: plan is {} — the conversation is closed",
                plan_state_str(&active.plan.state)
            ));
        }
        let attachments = self.parse_message_attachments(&entity_id, params)?;
        let messages = match &choice {
            Some(choice) => vec![ReviewerMessage {
                body: active
                    .agents
                    .resolve(Some(&address.conversation_id))?
                    .thread
                    .option_reply_text(choice)?,
                anchor: None,
                viewing_context: parse_viewing_context(params.get("viewing_context"))?,
                from_agent: None,
                from_issue: None,
            }],
            None => parse_thread_post_messages(
                params,
                crate::thread::ArtifactKind::Plan,
                !attachments.is_empty(),
            )?,
        };
        let resume = matches!(
            active.plan.state,
            PlanState::Blocked | PlanState::Failed | PlanState::IdleUnreported
        );
        let choice_revision = active
            .agents
            .resolve(Some(&address.agent_id))?
            .choice_revision;
        let implementation = named_agent_id(params)?
            .is_none()
            .then(|| {
                self.current_issue_implementation_id(&entity_id)
                    .and_then(|run_id| {
                        self.runs.get(&run_id).and_then(|run| {
                            run.agents
                                .agents()
                                .iter()
                                .find(|agent| agent.conversation_id() == address.conversation_id)
                                .map(|agent| ImplementationTarget {
                                    run_id,
                                    worktree_path: run.worktree.path.clone(),
                                    agent_id: agent.id.clone(),
                                    model_choice: agent.choice.clone(),
                                    choice_revision: agent.choice_revision,
                                })
                        })
                    })
            })
            .flatten();
        let mut active = self.take_plan(&entity_id)?;
        let previous_thread = active
            .agents
            .resolve(Some(&address.conversation_id))?
            .thread
            .clone();
        let previous_state = active.plan.state;
        let inert = active.plan.state == PlanState::Created && active.workspace.is_none();
        let mut delivery = if inert {
            let project_id = self.project_of(&entity_id)?;
            Some(DeliveryIntent {
                root: self.repo_path_for(&project_id)?,
                owner_id: entity_id.clone(),
                agent_id: address.agent_id.clone(),
                model_choice: active.agents.sole().choice.clone(),
                choice_revision: active.agents.sole().choice_revision,
                interrupt,
                payload: None,
            })
        } else if let Some(target) = implementation.as_ref() {
            Some(DeliveryIntent {
                root: target.worktree_path.clone(),
                owner_id: target.run_id.clone(),
                agent_id: target.agent_id.clone(),
                model_choice: target.model_choice.clone(),
                choice_revision: target.choice_revision,
                interrupt,
                payload: None,
            })
        } else {
            active.workspace.as_ref().map(|workspace| DeliveryIntent {
                root: workspace.checkout.clone(),
                owner_id: entity_id.clone(),
                agent_id: address.agent_id.clone(),
                model_choice: active.agents.sole().choice.clone(),
                choice_revision: active.agents.sole().choice_revision,
                interrupt,
                payload: None,
            })
        };
        if operation_id.is_some() && delivery.is_none() {
            self.plans.insert(entity_id.clone(), active);
            return Err(
                "thread.post: this Issue has no execution session; reopen or revise it before sending"
                    .to_string(),
            );
        }
        let (posted_sequence, payload) = append_operation_reviewer_messages(
            &mut active
                .agents
                .resolve_mut(Some(&address.conversation_id))?
                .thread,
            messages,
            attachments,
            choice.as_ref(),
            operation_id.as_deref(),
        );
        if let Some(delivery) = delivery.as_mut() {
            delivery.payload = payload;
        }
        if resume {
            active
                .plan
                .apply(PlanEvent::Reply)
                .expect("parked plan reply");
        }
        let receipt = self.stage_post_acceptance(
            params,
            PostOrigin::human(operation_id),
            &address,
            choice_revision,
            posted_sequence,
            delivery.clone(),
        )?;
        let (view, persisted) =
            self.answer_plan_mutation(entity_id.clone(), active, thread_detail(params));
        if let Err(error) = persisted {
            let active = self
                .plans
                .get_mut(&entity_id)
                .expect("failed finish still restores the plan");
            active
                .agents
                .resolve_mut(Some(&address.conversation_id))?
                .thread = previous_thread;
            active.plan.state = previous_state;
            return Err(error);
        }
        self.note_user_message(&entity_id);
        if !inert {
            if let Some(delivery) = delivery {
                self.queue_message_delivery(&delivery, receipt.as_ref());
            }
        }
        if inert {
            match self.reserve_plan_drafting(
                &entity_id,
                Box::new(PlanDraftingStarted {
                    issue_id: entity_id.clone(),
                    detail: thread_detail(params),
                    posted_sequence,
                    receipt: receipt.clone(),
                }),
            ) {
                Ok(Some(job)) => return Ok(self.defer_job(job)),
                Ok(None) => {}
                Err(error) => {
                    if let Some(receipt) = receipt.as_ref() {
                        return Ok(self.settle_accepted_operation_error(receipt, error));
                    }
                    return Err(error);
                }
            }
        }
        if let Some(run_id) = implementation.map(|target| target.run_id).filter(|run_id| {
            self.runs.get(run_id).is_some_and(|run| {
                matches!(
                    run.run.state,
                    RunState::Blocked | RunState::Failed | RunState::IdleUnreported
                )
            })
        }) {
            let mut run = self.take_run(&run_id)?;
            run.run.apply(RunEvent::Reply).expect("parked run reply");
            if let Err(error) = self.finish_run_mutation(run_id, run) {
                if let Some(receipt) = receipt.as_ref() {
                    return Ok(with_operation_error(
                        with_post_receipt(view, posted_sequence, Some(receipt)),
                        error,
                    ));
                }
                return Err(error);
            }
        }
        Ok(with_post_receipt(view, posted_sequence, receipt.as_ref()))
    }

    pub(in crate::app) fn thread_post_run(
        &mut self,
        params: &Value,
        address: ConversationAddress,
        origin: PostOrigin,
        choice: Option<crate::thread::OptionChoice>,
        interrupt: bool,
    ) -> Result<Value, String> {
        let entity_id = address.entity_id.clone();
        let active = self.runs.get(&entity_id).ok_or("unknown run_id")?;
        if active.run.state.is_terminal() {
            return Err(format!(
                "thread.post: run is {} — the conversation is closed",
                run_state_str(&active.run.state)
            ));
        }
        let attachments = self.parse_message_attachments(&entity_id, params)?;
        let messages = match &choice {
            Some(choice) => vec![ReviewerMessage {
                body: self.conversation_at(&address)?.option_reply_text(choice)?,
                anchor: None,
                viewing_context: parse_viewing_context(params.get("viewing_context"))?,
                from_agent: None,
                from_issue: None,
            }],
            None => parse_thread_post_messages(
                params,
                crate::thread::ArtifactKind::Diff,
                !attachments.is_empty(),
            )?,
        };
        let sender = origin.sender();
        let handed_over = origin.issue();
        let messages: Vec<ReviewerMessage> = messages
            .into_iter()
            .map(|message| {
                message
                    .sent_by(sender.as_ref())
                    .about_issue(handed_over.as_ref())
            })
            .collect();
        let resume = matches!(
            active.run.state,
            RunState::Blocked | RunState::Failed | RunState::IdleUnreported
        );
        let choice_revision = active
            .agents
            .resolve(Some(&address.agent_id))?
            .choice_revision;
        let delivery = DeliveryIntent {
            root: active.worktree.path.clone(),
            owner_id: entity_id.clone(),
            agent_id: address.agent_id.clone(),
            model_choice: active.agents.turn_choice(&address.agent_id)?,
            choice_revision,
            interrupt,
            payload: None,
        };
        let legacy_delivery = origin.operation_id.is_none().then(|| delivery.clone());
        let (posted_sequence, receipt) = self.append_addressed_post(
            params,
            &address,
            choice_revision,
            origin,
            messages,
            attachments,
            choice.as_ref(),
            Some(delivery),
        )?;
        // Only the human's own words are the human being here. One agent
        // handing work to another must not move the inbox anchor under the
        // reader — the same rule a dispatch an agent made follows.
        if sender.is_none() {
            self.note_user_message(&entity_id);
            if address.conversation_entity_id != entity_id {
                self.note_user_message(&address.conversation_entity_id);
            }
        }
        if let Some(delivery) = receipt
            .as_ref()
            .and_then(|receipt| receipt.delivery.as_ref())
        {
            self.queue_message_delivery(delivery, receipt.as_ref());
        } else if let Some(delivery) = legacy_delivery {
            self.queue_message_delivery(&delivery, None);
        }
        let mut active = self.take_run(&entity_id)?;
        if resume {
            active.run.apply(RunEvent::Reply).expect("parked run reply");
        }
        let (view, persisted) = self.answer_run_mutation(entity_id, active, thread_detail(params));
        if let Err(error) = persisted {
            if let Some(receipt) = receipt.as_ref() {
                return Ok(with_operation_error(
                    with_post_receipt(view, posted_sequence, Some(receipt)),
                    error,
                ));
            }
            return Err(error);
        }
        Ok(with_post_receipt(view, posted_sequence, receipt.as_ref()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn append_addressed_post(
        &mut self,
        params: &Value,
        address: &ConversationAddress,
        choice_revision: u64,
        origin: PostOrigin,
        messages: Vec<ReviewerMessage>,
        attachments: Vec<crate::thread::MessageAttachment>,
        choice: Option<&crate::thread::OptionChoice>,
        mut delivery: Option<DeliveryIntent>,
    ) -> Result<(Option<u64>, Option<OperationReceipt>), String> {
        if self.plans.contains_key(&address.conversation_entity_id) {
            let mut active = self.take_plan(&address.conversation_entity_id)?;
            let previous_thread = active
                .agents
                .resolve(Some(&address.conversation_id))?
                .thread
                .clone();
            let (sequence, payload) = append_operation_reviewer_messages(
                &mut active
                    .agents
                    .resolve_mut(Some(&address.conversation_id))?
                    .thread,
                messages,
                attachments,
                choice,
                origin.operation_id.as_deref(),
            );
            if let Some(delivery) = delivery.as_mut() {
                delivery.payload = payload;
            }
            let receipt = self.stage_post_acceptance(
                params,
                origin,
                address,
                choice_revision,
                sequence,
                delivery,
            )?;
            if let Err(error) =
                self.finish_plan_mutation(address.conversation_entity_id.clone(), active)
            {
                self.plans
                    .get_mut(&address.conversation_entity_id)
                    .expect("failed finish still restores the plan")
                    .agents
                    .resolve_mut(Some(&address.conversation_id))?
                    .thread = previous_thread;
                return Err(error);
            }
            return Ok((sequence, receipt));
        }
        let mut active = self.take_run(&address.conversation_entity_id)?;
        let previous_thread = active
            .agents
            .resolve(Some(&address.conversation_id))?
            .thread
            .clone();
        let (sequence, payload) = append_operation_reviewer_messages(
            &mut active
                .agents
                .resolve_mut(Some(&address.conversation_id))?
                .thread,
            messages,
            attachments,
            choice,
            origin.operation_id.as_deref(),
        );
        if let Some(delivery) = delivery.as_mut() {
            delivery.payload = payload;
        }
        let receipt = self.stage_post_acceptance(
            params,
            origin,
            address,
            choice_revision,
            sequence,
            delivery,
        )?;
        if let Err(error) = self.finish_run_mutation(address.conversation_entity_id.clone(), active)
        {
            self.runs
                .get_mut(&address.conversation_entity_id)
                .expect("failed finish still restores the run")
                .agents
                .resolve_mut(Some(&address.conversation_id))?
                .thread = previous_thread;
            return Err(error);
        }
        Ok((sequence, receipt))
    }

    pub(in crate::app) fn stage_post_acceptance(
        &mut self,
        params: &Value,
        origin: PostOrigin,
        address: &ConversationAddress,
        choice_revision: u64,
        posted_sequence: Option<u64>,
        delivery: Option<DeliveryIntent>,
    ) -> Result<Option<OperationReceipt>, String> {
        let Some(operation_id) = origin.operation_id else {
            return Ok(None);
        };
        let posted_sequence = posted_sequence.ok_or("thread.post did not append a message")?;
        let receipt = OperationReceipt {
            operation_id,
            method: THREAD_POST_METHOD.to_string(),
            entity_id: address.entity_id.clone(),
            agent_id: address.agent_id.clone(),
            conversation_id: address.conversation_id.clone(),
            choice_revision,
            posted_sequence,
            message_start_sequence: delivery
                .as_ref()
                .and_then(|delivery| delivery.payload.as_ref())
                .map(|payload| payload.start_sequence)
                .unwrap_or(posted_sequence),
            status: OperationStatus::Queued,
            execution_error: None,
            request_hash: thread_post_request_hash(
                params,
                &address.entity_id,
                &address.agent_id,
                &address.conversation_id,
            ),
            delivery,
            requested_by: origin.requested_by,
        };
        self.operation_ledger
            .stage_acceptance(address.conversation_entity_id.clone(), receipt.clone());
        Ok(Some(receipt))
    }

    pub(in crate::app) fn queue_message_delivery(
        &mut self,
        delivery: &DeliveryIntent,
        receipt: Option<&OperationReceipt>,
    ) {
        let operation_id = receipt.map(|receipt| receipt.operation_id.clone());
        let root = Self::canonical_root(&delivery.root);
        // A queued legacy turn snapshots pending messages just before sending.
        // An in-flight turn already froze its input, so a later post needs its
        // own follow-up even while the same session is still opening/writing.
        if operation_id.is_none()
            && self.delivery_queue.queued().any(|turn| {
                turn.operation_id.is_none()
                    && turn.owner == delivery.owner_id
                    && turn.agent_id == delivery.agent_id
            })
        {
            return;
        }
        let conversation_id = receipt
            .map(|receipt| receipt.conversation_id.clone())
            .or_else(|| {
                self.resolve_conversation_address(&delivery.owner_id, Some(&delivery.agent_id))
                    .ok()
                    .map(|address| address.conversation_id)
            })
            .unwrap_or_default();
        let operation_prompt = receipt.and_then(|receipt| {
            delivery.payload.as_ref().map(|payload| {
                let payload = OperationPayload {
                    tells_sender_context: crate::agent::is_project_agent(&delivery.agent_id),
                    ..payload.clone()
                };
                TurnText {
                    cold: payload.delivery_prompt(
                        &receipt.operation_id,
                        true,
                        delivery.model_choice.provider,
                    ),
                    warm: payload.delivery_prompt(
                        &receipt.operation_id,
                        false,
                        delivery.model_choice.provider,
                    ),
                }
            })
        });
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id,
            root,
            owner: delivery.owner_id.clone(),
            agent_id: delivery.agent_id.clone(),
            conversation_id,
            model_choice: delivery.model_choice.clone(),
            choice_revision: delivery.choice_revision,
            interrupt: delivery.interrupt,
            say: operation_prompt.or_else(|| {
                Some(TurnText {
                    cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                    warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
                })
            }),
            phase: "revive",
            wants_catch_up: receipt.is_none(),
            survives_refusal: receipt.is_some(),
        });
    }

    pub(crate) fn thread_operation(&self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let operation_id = required_operation_id(params)?;
        let receipt = self
            .operation_receipt(&operation_id)?
            .ok_or_else(|| format!("unknown operation_id: {operation_id}"))?;
        if receipt.entity_id != entity_id {
            return Err("thread.operation: operation does not belong to entity".into());
        }
        if let Some(agent_id) = named_agent_id(params)? {
            if receipt.agent_id != agent_id {
                return Err("thread.operation: operation does not belong to agent".into());
            }
        }
        Ok(receipt.wire_value())
    }

    pub(in crate::app) fn operation_receipt(
        &self,
        operation_id: &str,
    ) -> Result<Option<OperationReceipt>, String> {
        match self.store.as_ref() {
            Some(store) => store
                .operation(operation_id)
                .map_err(|error| format!("operation store: {error}")),
            None => Ok(self.operation_ledger.cached(operation_id).cloned()),
        }
    }

    pub(in crate::app) fn transition_delivery_operation(
        &mut self,
        operation_id: &str,
        expected: OperationStatus,
        next: OperationStatus,
        execution_error: Option<&str>,
    ) -> Result<bool, String> {
        let changed = match self.store.as_ref() {
            Some(store) => store
                .transition_operation(operation_id, expected, next, execution_error)
                .map_err(|error| format!("operation store: {error}"))?,
            None => self
                .operation_ledger
                .cached_has_status(operation_id, expected),
        };
        if changed {
            // Store success is authoritative. This update is unconditional with
            // respect to stale cached status and absent cache is not a failure.
            if let Some(entity_id) = self.operation_ledger.record_transition_if_cached(
                operation_id,
                next,
                execution_error,
            ) {
                self.note_entity_changed(&entity_id);
            }
            let status = match (next, execution_error) {
                (OperationStatus::Uncertain, _) => {
                    Some(crate::thread::MessageDeliveryStatus::Uncertain)
                }
                (_, Some(_)) => Some(crate::thread::MessageDeliveryStatus::Failed),
                _ => None,
            };
            if let Some(status) = status {
                if let Err(error) = self.record_operation_delivery_status(operation_id, status) {
                    eprintln!("record delivery status {operation_id}: {error}");
                }
            }
        }
        Ok(changed)
    }
    pub(in crate::app) fn settle_accepted_operation_error(
        &mut self,
        receipt: &OperationReceipt,
        error: String,
    ) -> Value {
        let _ = self.transition_delivery_operation(
            &receipt.operation_id,
            OperationStatus::Queued,
            OperationStatus::Delivered,
            Some(&error),
        );
        let settled = self
            .operation_receipt(&receipt.operation_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let mut settled = receipt.clone();
                settled.status = OperationStatus::Delivered;
                settled.execution_error = Some(error.clone());
                settled
            });
        with_operation_error(post_operation_value(&settled), error)
    }
}
