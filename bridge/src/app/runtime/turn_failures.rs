//! Provider failures use the ordinary report channel, with notices for work
//! the failed agent can no longer hand back itself.

use crate::app::{sha256_hex, AppState};
use crate::mcp::{DoneReport, DoneStatus};
use crate::operation::OperationReceipt;
use crate::thread::{MessageRole, TaskEnvelope};
use std::collections::{HashMap, HashSet};

struct ConsumedOperation {
    operation_id: String,
    thread_id: String,
    sequence: u64,
}

#[derive(Default)]
pub(in crate::app) struct FailureNoticeLedger {
    consumed: HashMap<String, ConsumedOperation>,
    reported: HashSet<(String, String)>,
    /// A live session's completed-turn baseline when a managed block begins.
    /// The durable Thread marker outlives this map; a new launch resets counters.
    compaction_baselines: HashMap<String, (String, u64)>,
}

impl FailureNoticeLedger {
    pub(in crate::app) fn forget_consumed(&mut self, agent_id: &str) {
        self.consumed.remove(agent_id);
    }

    fn compaction_completion_recovers(&self, agent_id: &str, launch_id: &str, count: u64) -> bool {
        !self
            .compaction_baselines
            .get(agent_id)
            .is_some_and(|(baseline_launch, baseline)| {
                baseline_launch == launch_id && count <= *baseline
            })
    }

    pub(in crate::app) fn forget_agent(&mut self, agent_id: &str) {
        self.compaction_baselines.remove(agent_id);
        self.forget_consumed(agent_id);
        self.reported.retain(|(agent, _)| agent != agent_id);
    }
}

impl AppState {
    /// Both Sent and ownership-checked Seen pass this door. Older cumulative
    /// receipts cannot replace a newer operation in the same generation.
    pub(in crate::app) fn record_consumed_failure_operation(
        &mut self,
        owner: &str,
        agent_id: &str,
        operation_id: &str,
    ) {
        let Some(receipt) = self.operation_receipt(operation_id).ok().flatten() else {
            return;
        };
        let (delivery_owner, delivery_agent) = receipt.delivery_target();
        if receipt.delivery_message_span().is_none() {
            return;
        }
        let Ok(address) = self.resolve_conversation_address(owner, Some(agent_id)) else {
            return;
        };
        if delivery_owner != owner
            || delivery_agent != agent_id
            || receipt.conversation_id != address.conversation_id
        {
            return;
        }
        let Ok(thread) = self.conversation_at(&address) else {
            return;
        };
        let thread_id = thread.id.clone();
        let older = self
            .agent_failures
            .consumed
            .get(agent_id)
            .is_some_and(|current| {
                current.thread_id == thread_id && current.sequence >= receipt.posted_sequence
            });
        if !older {
            self.agent_failures.consumed.insert(
                agent_id.into(),
                ConsumedOperation {
                    operation_id: operation_id.into(),
                    thread_id,
                    sequence: receipt.posted_sequence,
                },
            );
        }
    }

    pub(in crate::app) fn record_agent_turn_failure(
        &mut self,
        owner: &str,
        agent_id: &str,
        operation_id: Option<&str>,
        summary: &str,
    ) {
        self.record_agent_failure(owner, agent_id, operation_id, summary, None);
    }

    pub(in crate::app) fn record_agent_compaction_failure(
        &mut self,
        owner: &str,
        agent_id: &str,
        operation_id: Option<&str>,
        summary: &str,
    ) {
        let baseline = self
            .live_agent_session(owner, agent_id)
            .and_then(|session| {
                let launch_id = session.compaction_launch_identity()?.id.clone();
                let status = session.status_changed()?;
                let count = status.borrow().successful_turn_count;
                Some((launch_id, count))
            });
        if let Some(baseline) = baseline {
            self.agent_failures
                .compaction_baselines
                .insert(agent_id.into(), baseline);
        }
        self.record_agent_failure(
            owner,
            agent_id,
            operation_id,
            summary,
            Some(crate::mcp::InternalFailureKind::ManagedCompactionUnavailable),
        );
    }

    pub(in crate::app) fn clear_managed_compaction_failure_after_turn(
        &mut self,
        owner: &str,
        agent_id: &str,
        launch_id: &str,
        successful_turn_count: u64,
    ) {
        if self.agent_failures.compaction_completion_recovers(
            agent_id,
            launch_id,
            successful_turn_count,
        ) {
            self.clear_managed_compaction_failure(owner, agent_id);
        }
    }

    /// Recovery ends the durable episode on the canonical conversation. The
    /// already-loaded marker makes a redundant clear require no store access.
    pub(in crate::app) fn clear_managed_compaction_failure(&mut self, owner: &str, agent_id: &str) {
        self.agent_failures.compaction_baselines.remove(agent_id);
        if !self
            .agent_conversation(owner, Some(agent_id))
            .is_ok_and(|thread| thread.managed_compaction_failure_reported)
        {
            return;
        }
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _| {
            thread.managed_compaction_failure_reported = false;
            Ok(())
        }) {
            eprintln!("clear recovered managed-compaction failure: {error}");
        }
    }

    fn record_agent_failure(
        &mut self,
        owner: &str,
        agent_id: &str,
        operation_id: Option<&str>,
        summary: &str,
        failure_kind: Option<crate::mcp::InternalFailureKind>,
    ) {
        let Ok(thread) = self.agent_conversation(owner, Some(agent_id)) else {
            return;
        };
        let thread_id = thread.id.clone();
        let operation = self.failure_operation(owner, agent_id, &thread_id, operation_id);
        let source = operation_id
            .map(str::to_string)
            .or_else(|| {
                operation
                    .as_ref()
                    .map(|receipt| receipt.operation_id.clone())
            })
            .unwrap_or_else(|| operationless_failure_source(thread, agent_id));
        let key = sha256_hex(format!("{agent_id}:{thread_id}:{source}").as_bytes());
        if !self
            .agent_failures
            .reported
            .insert((agent_id.into(), key.clone()))
        {
            return;
        }
        let newer_turn = operation.as_ref().is_some_and(|failed| {
            self.agent_failures
                .consumed
                .get(agent_id)
                .is_some_and(|current| {
                    current.thread_id == thread_id && current.sequence > failed.posted_sequence
                })
        });
        let task = operation.as_ref().and_then(|operation| {
            self.take_task_failure_authority(owner, agent_id, &operation.operation_id)
        });
        let body = format!(
            "Agent @agent:{agent_id} failed{}: {summary}",
            task.as_ref()
                .map(|task| format!(" while handling #{}", task.number))
                .unwrap_or_default()
        );
        // A Failed report cannot queue completion Git work; it disarms auto
        // advance and stops this agent's execution clock like an MCP report.
        self.report_turn_failure(owner, agent_id, summary, newer_turn, failure_kind);
        if let Some(task) = task.as_ref() {
            if let Err(error) = self.comment_terminal_agent_failure(task.clone(), &key, &body) {
                eprintln!("record provider failure on task {}: {error}", task.id);
            }
        }
        self.notify_agent_failure(owner, agent_id, operation.as_ref(), task.as_ref(), &body);
    }

    pub(in crate::app) fn failure_operation(
        &self,
        owner: &str,
        agent_id: &str,
        thread_id: &str,
        named_operation: Option<&str>,
    ) -> Option<OperationReceipt> {
        let operation_id = named_operation
            .or_else(|| self.task_failure_operation(agent_id))
            .or_else(|| {
                self.agent_failures
                    .consumed
                    .get(agent_id)
                    .filter(|current| current.thread_id == thread_id)
                    .map(|current| current.operation_id.as_str())
            })?;
        let receipt = self.operation_receipt(operation_id).ok()??;
        receipt.delivery_message_span()?;
        let (delivery_owner, delivery_agent) = receipt.delivery_target();
        let address = self
            .resolve_conversation_address(owner, Some(agent_id))
            .ok()?;
        (delivery_owner == owner
            && delivery_agent == agent_id
            && receipt.conversation_id == address.conversation_id)
            .then_some(receipt)
    }

    fn report_turn_failure(
        &mut self,
        owner: &str,
        agent_id: &str,
        summary: &str,
        newer_turn: bool,
        failure_kind: Option<crate::mcp::InternalFailureKind>,
    ) {
        let mut report = DoneReport::new(DoneStatus::Failed, summary);
        report.failure_kind = failure_kind;
        if newer_turn {
            // The failed turn still owes its report; the succeeding turn keeps
            // its execution clock and its owner's lifecycle.
            if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _| {
                crate::app::record_report_in_thread(thread, &report, None);
                Ok(())
            }) {
                eprintln!("record delayed provider failure: {error}");
            }
        } else {
            self.done_deferring_for_agent(owner, agent_id, report);
        }
    }

    fn notify_agent_failure(
        &mut self,
        owner: &str,
        agent_id: &str,
        operation: Option<&OperationReceipt>,
        task: Option<&crate::tracker::Task>,
        body: &str,
    ) {
        let Some((entity, target)) = self.failure_notice_target(owner, agent_id, operation) else {
            return;
        };
        let envelope = task.map(|task| TaskEnvelope {
            task_id: task.id.clone(),
            number: task.number,
            title: task.title.clone(),
            links: task.links.clone(),
        });
        if let Err(error) =
            self.post_from_build_and_wake(&entity, &target, "agent_failure", |thread, now| {
                thread.post_user_from_build(body, now);
                if let Some(envelope) = envelope {
                    thread.wear_task(envelope);
                }
            })
        {
            eprintln!("notify {target} about provider failure: {error}");
        }
    }

    pub(in crate::app) fn failure_notice_target(
        &mut self,
        owner: &str,
        failed_agent: &str,
        operation: Option<&OperationReceipt>,
    ) -> Option<(String, String)> {
        let project = self.projects.project_id_of(owner)?.to_string();
        let failed_address = self
            .resolve_conversation_address(owner, Some(failed_agent))
            .ok()?;
        if let Some(requester) = operation.and_then(|operation| operation.requested_by.as_ref()) {
            let address = self
                .resolve_conversation_address(&requester.entity_id, Some(&requester.agent_id))
                .ok();
            if address.is_some_and(|address| {
                address.conversation_id == requester.conversation_id
                    && address.conversation_id != failed_address.conversation_id
            }) && self.projects.project_id_of(&requester.entity_id) == Some(project.as_str())
            {
                return Some((requester.entity_id.clone(), requester.agent_id.clone()));
            }
        }
        let coordinator_owner = self.project_conversation_run(&project).or_else(|| {
            self.project_ensure_conversation(&serde_json::json!({"project_id":project}))
                .ok()
                .and_then(|created| created["run_id"].as_str().map(str::to_string))
        })?;
        let coordinator = self.project_pin(&coordinator_owner)?;
        (coordinator != failed_agent).then(|| (coordinator_owner.clone(), coordinator.into()))
    }
}

fn operationless_failure_source(thread: &crate::thread::Thread, agent_id: &str) -> String {
    let session = thread
        .open_session_instance(agent_id)
        .map(|session| session.id)
        .unwrap_or_default();
    let sequence = thread
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            crate::thread::ThreadItem::Message(message) if message.role == MessageRole::User => {
                Some(message.sequence)
            }
            _ => None,
        })
        .unwrap_or_default();
    format!("{session}:{sequence}")
}

#[cfg(test)]
mod compaction_episode_tests {
    use super::*;

    #[test]
    fn compaction_episode_recovery_requires_a_new_success_on_the_same_launch() {
        let mut ledger = FailureNoticeLedger::default();
        ledger
            .compaction_baselines
            .insert("agent".into(), ("launch".into(), 1));
        assert!(
            !ledger.compaction_completion_recovers("agent", "launch", 1),
            "an older successful result cannot clear the new blocked episode"
        );
        assert!(ledger.compaction_completion_recovers("agent", "launch", 2));
        assert!(
            ledger.compaction_completion_recovers("agent", "new-launch", 1),
            "a reopened session starts a new success counter"
        );
        ledger.forget_agent("agent");
        assert!(!ledger.compaction_baselines.contains_key("agent"));
        assert!(ledger.compaction_completion_recovers("agent", "launch", 1));
    }
}
