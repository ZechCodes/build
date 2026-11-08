//! Replay the durable provider-failure outbox independently of normal resume.

use crate::app::{sha256_hex, AppState, PendingAgentTurn, TurnText};
use crate::mcp::{DoneReport, DoneStatus};
use crate::operation::OperationReceipt;
use crate::provider_recovery::{
    self, RecoveryEpisode, RecoveryRecipient, RecoveryScope, RecoveryStage,
};
use crate::thread::{MessageOutcome, TaskEnvelope, ThreadItem, ThreadMessage};

impl AppState {
    pub(in crate::app) fn replay_provider_recovery_episodes(&mut self) -> Result<(), String> {
        let episodes = provider_recovery::all_episodes(&self.state_root)?;
        let mut first_error = None;
        for episode in episodes {
            if let Err(error) = self.replay_provider_episode(episode) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Restoring a project releases its durable failure notices without a restart.
    pub(in crate::app) fn replay_provider_recovery_for_project(
        &mut self,
        project: &str,
    ) -> Result<bool, String> {
        let episodes = provider_recovery::independent_episodes(&self.state_root)?;
        let mut first_error = None;
        let mut complete = true;
        for episode in episodes {
            let result = episode
                .and_then(|episode| self.replay_provider_episode_for_project(project, episode));
            match result {
                Ok(settled) => complete &= settled,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(complete), Err)
    }

    fn replay_provider_episode_for_project(
        &mut self,
        project: &str,
        episode: RecoveryEpisode,
    ) -> Result<bool, String> {
        let source = self.projects.project_id_of(&episode.scope.entity_id) == Some(project);
        let recipient = episode.recipient.as_ref().is_some_and(|recipient| {
            self.projects.project_id_of(&recipient.scope.entity_id) == Some(project)
        });
        if !source && !recipient {
            return Ok(true);
        }
        let scope = episode.scope.clone();
        let id = episode.id.clone();
        self.replay_provider_episode(episode)?;
        // A fenced or live episode can return successfully without delivering.
        // Keep the restore obligation until the durable checkpoint settles.
        let saved = provider_recovery::read_episode(&self.state_root, &scope.agent_id, &id)?;
        Ok(saved.is_none_or(|saved| !saved.active && !saved.needs_notice))
    }

    fn provider_episode_archived(&self, episode: &RecoveryEpisode) -> bool {
        self.refuse_archived_entity(&episode.scope.entity_id)
            .is_err()
            || episode.recipient.as_ref().is_some_and(|recipient| {
                self.refuse_archived_entity(&recipient.scope.entity_id)
                    .is_err()
            })
    }

    pub(in crate::app) fn record_provider_recovery_failure(
        &mut self,
        owner: &str,
        agent: &str,
        episode_id: &str,
    ) -> Result<(), String> {
        let episode = provider_recovery::read_episode(&self.state_root, agent, episode_id)?
            .ok_or("provider failure checkpoint is missing")?;
        if episode.scope != self.current_recovery_scope(owner, agent)? {
            return Err(
                "provider failure checkpoint belongs to another conversation generation".into(),
            );
        }
        if episode.stage != RecoveryStage::Terminal {
            return Err("live provider failure has no terminal outbox checkpoint".into());
        }
        if episode.needs_notice {
            self.persist_provider_failure_notice(episode)
        } else {
            self.queue_recovery_recipient(episode.recipient.as_ref())
        }
    }

    pub(in crate::app) fn provider_recovery_blocks_resume(
        &self,
        owner: &str,
        agent: &str,
    ) -> Result<bool, String> {
        let scope = self.current_recovery_scope(owner, agent)?;
        Ok(provider_recovery::episodes(&self.state_root, &scope)?
            .iter()
            .any(|episode| episode.active))
    }

    fn current_recovery_scope(&self, owner: &str, agent: &str) -> Result<RecoveryScope, String> {
        let address = self.resolve_conversation_address(owner, Some(agent))?;
        let thread_id = self.conversation_at(&address)?.id.clone();
        Ok(RecoveryScope {
            entity_id: owner.into(),
            agent_id: agent.into(),
            conversation_id: address.conversation_id,
            thread_id,
        })
    }

    fn recovery_scope_stands(&self, scope: &RecoveryScope) -> bool {
        self.current_recovery_scope(&scope.entity_id, &scope.agent_id)
            .is_ok_and(|current| current == *scope)
    }

    fn replay_provider_episode(&mut self, episode: RecoveryEpisode) -> Result<(), String> {
        if self.provider_episode_archived(&episode) || self.provider_episode_is_live(&episode) {
            return Ok(());
        }
        if !self.recovery_scope_stands(&episode.scope) {
            provider_recovery::update_episode(
                &self.state_root,
                &episode.scope.agent_id,
                &episode.id,
                |record| {
                    record.active = false;
                    record.needs_notice = false;
                },
            )?;
            return self.queue_recovery_recipient(episode.recipient.as_ref());
        }
        let episode = provider_recovery::update_episode(
            &self.state_root,
            &episode.scope.agent_id,
            &episode.id,
            provider_recovery::abandon,
        )?
        .ok_or("provider failure checkpoint disappeared")?;
        if episode.needs_notice {
            self.persist_provider_failure_notice(episode)
        } else {
            self.queue_recovery_recipient(episode.recipient.as_ref())
        }
    }

    fn provider_episode_is_live(&self, episode: &RecoveryEpisode) -> bool {
        if !episode.active
            || !matches!(
                episode.stage,
                RecoveryStage::Pending | RecoveryStage::InFlight
            )
            || !self.recovery_scope_stands(&episode.scope)
        {
            return false;
        }
        self.session_registry
            .live_agent_snapshots()
            .iter()
            .any(|tab| {
                tab.instance.as_ref().is_some_and(|instance| {
                    instance.entity_id == episode.scope.entity_id
                        && instance.agent_id == episode.scope.agent_id
                        && instance.conversation_id == episode.scope.conversation_id
                }) && !matches!(
                    tab.session.status(),
                    crate::harness::AgentStatus::Ended { .. }
                ) && tab.session.provider_recovery_episode().as_deref() == Some(episode.id.as_str())
            })
    }

    fn persist_provider_failure_notice(
        &mut self,
        mut episode: RecoveryEpisode,
    ) -> Result<(), String> {
        if self.provider_episode_archived(&episode) {
            return Ok(());
        }
        let scope = &episode.scope;
        // A journal entry carries its own provenance. An operationless old
        // episode cannot borrow the current task or a newer consumed message.
        let operation = episode.operation_id.as_deref().and_then(|operation| {
            self.failure_operation(
                &scope.entity_id,
                &scope.agent_id,
                &scope.thread_id,
                Some(operation),
            )
        });
        let key =
            sha256_hex(format!("{}:{}:{}", scope.agent_id, scope.thread_id, episode.id).as_bytes());
        if episode.recipient.is_none() {
            let recipient = self.choose_recovery_recipient(scope, operation.as_ref(), &key)?;
            if recipient.as_ref().is_some_and(|recipient| {
                self.refuse_archived_entity(&recipient.scope.entity_id)
                    .is_err()
            }) {
                return Ok(());
            }
            episode = provider_recovery::update_episode(
                &self.state_root,
                &scope.agent_id,
                &episode.id,
                |record| {
                    if record.recipient.is_none() {
                        record.recipient = recipient;
                    }
                },
            )?
            .ok_or("provider failure checkpoint disappeared")?;
        }
        let task = operation.as_ref().and_then(|operation| {
            self.task_for_failed_operation(
                &episode.scope.entity_id,
                &episode.scope.agent_id,
                &operation.operation_id,
            )
        });
        let body = failure_notice(&episode.scope.agent_id, task.as_ref(), &episode.summary);
        self.persist_recovery_outcome(&episode, operation.as_ref(), &key)?;
        if let Some(task) = &task {
            self.comment_terminal_agent_failure(task.clone(), &key, &body)?;
        }
        self.persist_recovery_recipient(&episode, task.as_ref(), &body)?;
        self.queue_recovery_recipient(episode.recipient.as_ref())?;
        if let Some(operation) = operation {
            self.take_task_failure_authority(
                &episode.scope.entity_id,
                &episode.scope.agent_id,
                &operation.operation_id,
            );
        }
        provider_recovery::update_episode(
            &self.state_root,
            &episode.scope.agent_id,
            &episode.id,
            |record| record.needs_notice = false,
        )?;
        Ok(())
    }

    fn choose_recovery_recipient(
        &mut self,
        scope: &RecoveryScope,
        operation: Option<&OperationReceipt>,
        key: &str,
    ) -> Result<Option<RecoveryRecipient>, String> {
        let target = self.failure_notice_target(&scope.entity_id, &scope.agent_id, operation);
        let Some((owner, agent)) = target else {
            self.check_recovery_notice_target(scope)?;
            return Ok(None);
        };
        Ok(Some(RecoveryRecipient {
            scope: self.current_recovery_scope(&owner, &agent)?,
            message_id: format!("provider-failure-notice-{key}"),
        }))
    }

    fn check_recovery_notice_target(&self, scope: &RecoveryScope) -> Result<(), String> {
        let Some(project) = self.projects.project_id_of(&scope.entity_id) else {
            return Ok(());
        };
        let owner = self
            .project_conversation_run(project)
            .ok_or("provider failure coordinator is unavailable")?;
        let pin = self
            .project_pin(&owner)
            .ok_or("provider failure coordinator is missing")?;
        if pin != scope.agent_id {
            return Err("provider failure notice could not select its recipient".into());
        }
        Ok(())
    }

    fn persist_recovery_outcome(
        &mut self,
        episode: &RecoveryEpisode,
        operation: Option<&OperationReceipt>,
        key: &str,
    ) -> Result<(), String> {
        let scope = &episode.scope;
        let message_id = format!("provider-failure-report-{key}");
        let exists = self.recovery_message(scope, &message_id)?.is_some();
        let newer = !episode.active || self.recovery_has_newer_work(scope, operation)?;
        self.edit_agent_conversation(&scope.entity_id, &scope.agent_id, |thread, _| {
            if !exists && !thread_has_message(thread, &message_id) {
                thread.post_outcome(
                    MessageOutcome::Failed,
                    &episode.summary,
                    None,
                    crate::store::now_rfc3339(),
                );
                set_last_message_id(thread, &message_id);
            }
            Ok(())
        })?;
        let working = self
            .entity_agents(&scope.entity_id)?
            .by_id(&scope.agent_id)
            .is_some_and(|agent| agent.working_since.is_some());
        if !newer && (!exists || working) {
            let mut report = DoneReport::new(DoneStatus::Failed, &episode.summary);
            report.message_id = Some(message_id);
            self.done_deferring_for_agent(&scope.entity_id, &scope.agent_id, report);
            // The legacy report router logs persistence errors. This checked
            // flush keeps the outbox pending until the resulting owner saves.
            self.edit_agent_conversation(&scope.entity_id, &scope.agent_id, |_, _| Ok(()))?;
        }
        Ok(())
    }

    fn recovery_has_newer_work(
        &self,
        scope: &RecoveryScope,
        failed: Option<&OperationReceipt>,
    ) -> Result<bool, String> {
        let Some(failed) = failed else {
            return Ok(false);
        };
        if let Some(store) = &self.store {
            return store
                .has_seen_operation_after(
                    &scope.conversation_id,
                    failed.posted_sequence,
                    &failed.operation_id,
                )
                .map_err(|error| error.to_string());
        }
        Ok(self.agent_conversation(&scope.entity_id, Some(&scope.agent_id))?.items.iter().any(|item| matches!(item,
            ThreadItem::Message(message) if message.sequence > failed.posted_sequence
                && message.operation_id.as_deref().is_some_and(|operation| operation != failed.operation_id)
                && message.seen_at.is_some()
        )))
    }

    fn persist_recovery_recipient(
        &mut self,
        episode: &RecoveryEpisode,
        task: Option<&crate::tracker::Task>,
        body: &str,
    ) -> Result<(), String> {
        let Some(recipient) = &episode.recipient else {
            return Ok(());
        };
        if self
            .refuse_archived_entity(&recipient.scope.entity_id)
            .is_err()
            || !self.recovery_scope_stands(&recipient.scope)
        {
            return Ok(());
        }
        let exists = self
            .recovery_message(&recipient.scope, &recipient.message_id)?
            .is_some();
        let envelope = task.map(|task| TaskEnvelope {
            task_id: task.id.clone(),
            number: task.number,
            title: task.title.clone(),
            links: task.links.clone(),
        });
        self.edit_agent_conversation(
            &recipient.scope.entity_id,
            &recipient.scope.agent_id,
            |thread, _| {
                if !exists && !thread_has_message(thread, &recipient.message_id) {
                    thread.post_user_from_build(body, crate::store::now_rfc3339());
                    set_last_message_id(thread, &recipient.message_id);
                    if let Some(envelope) = envelope {
                        thread.wear_task(envelope);
                    }
                }
                Ok(())
            },
        )
    }

    fn queue_recovery_recipient(
        &mut self,
        recipient: Option<&RecoveryRecipient>,
    ) -> Result<(), String> {
        let Some(recipient) = recipient else {
            return Ok(());
        };
        if self
            .refuse_archived_entity(&recipient.scope.entity_id)
            .is_err()
            || !self.recovery_scope_stands(&recipient.scope)
        {
            return Ok(());
        }
        let Some(message) = self.recovery_message(&recipient.scope, &recipient.message_id)? else {
            return Ok(());
        };
        // seen_at belongs to the agent's exact native/read acknowledgement.
        // Human attention cursors cannot suppress this pending wake.
        if message.seen_at.is_some() {
            return Ok(());
        }
        let addressed =
            self.current_service_agent(&recipient.scope.entity_id, &recipient.scope.agent_id)?;
        self.delivery_queue
            .enqueue_after_settle_window(PendingAgentTurn {
                operation_id: None,
                root: addressed.root,
                owner: addressed.entity_id,
                agent_id: addressed.agent_id,
                conversation_id: addressed.conversation_id,
                say: Some(TurnText::reading_unread(addressed.model_choice.provider)),
                model_choice: addressed.model_choice,
                choice_revision: addressed.choice_revision,
                interrupt: false,
                phase: "agent_failure",
                wants_catch_up: true,
                survives_refusal: false,
            });
        Ok(())
    }

    fn recovery_message(
        &self,
        scope: &RecoveryScope,
        id: &str,
    ) -> Result<Option<ThreadMessage>, String> {
        if let Some(store) = &self.store {
            return store
                .thread_message_by_id(&scope.conversation_id, id)
                .map_err(|error| error.to_string());
        }
        Ok(self
            .agent_conversation(&scope.entity_id, Some(&scope.agent_id))?
            .items
            .iter()
            .find_map(|item| match item {
                ThreadItem::Message(message) if message.id == id => Some(message.clone()),
                _ => None,
            }))
    }
}

fn thread_has_message(thread: &crate::thread::Thread, id: &str) -> bool {
    thread
        .items
        .iter()
        .any(|item| matches!(item, ThreadItem::Message(message) if message.id == id))
}

fn set_last_message_id(thread: &mut crate::thread::Thread, id: &str) {
    if let Some(ThreadItem::Message(message)) = thread.items.last_mut() {
        message.id = id.into();
    }
}

/// What the coordinator is told about an agent that failed, and the task it
/// failed on when there was one.
fn failure_notice(agent_id: &str, task: Option<&crate::tracker::Task>, summary: &str) -> String {
    use crate::templates::{fill, MESSAGE_AGENT_FAILED, MESSAGE_AGENT_FAILED_ON_TASK};
    let Some(task) = task else {
        return fill(
            MESSAGE_AGENT_FAILED,
            &[("agent_id", agent_id), ("summary", summary)],
        );
    };
    let number = task.number.to_string();
    fill(
        MESSAGE_AGENT_FAILED_ON_TASK,
        &[
            ("agent_id", agent_id),
            ("number", number.as_str()),
            ("summary", summary),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::failure_notice;
    use crate::tracker::{Actor, Task};

    #[test]
    fn a_failure_notice_names_the_agent_and_the_task_it_failed_on() {
        let mut task = Task::drafted("/repo", "Kanban drag", Actor::User, "2026-09-20T15:00:00Z");
        task.number = 13;
        assert_eq!(
            failure_notice("agent-1", Some(&task), "{summary} went away"),
            "Agent @agent:agent-1 failed while handling #13: {summary} went away"
        );
        assert_eq!(
            failure_notice("agent-1", None, "stream disconnected"),
            "Agent @agent:agent-1 failed: stream disconnected"
        );
    }
}
