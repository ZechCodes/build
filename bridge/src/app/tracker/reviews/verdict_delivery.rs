//! A deciding verdict is an immutable outbox entry, independently delivered to
//! the author through the same durable operations as ordinary conversation input.
use crate::api::v1::reviews::ReviewRetryDeliveryParams;
use crate::app::tracker::TaskWrite;
use crate::app::AppState;
use crate::operation::OperationStatus;
use crate::store::ReviewVerdictDelivery;
use crate::tracker::{Actor, TaskComment};
use serde_json::{json, Value};

const DELIVERY_PAGE: usize = 20;

impl AppState {
    pub(in crate::app) fn drain_review_verdict_deliveries(&mut self) -> Result<(), String> {
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        let (deliveries, next) = store
            .review_verdict_delivery_page(DELIVERY_PAGE, self.review_verdict_poll_after)
            .map_err(|error| error.to_string())?;
        self.review_verdict_poll_after = next;
        for delivery in deliveries {
            if let Err(error) = self.deliver_review_verdict(&delivery) {
                store
                    .fail_review_verdict_delivery(&delivery.comment_id, &error)
                    .map_err(|error| error.to_string())?;
                self.note_verdict_delivery_changed(&delivery);
            }
        }
        Ok(())
    }

    fn deliver_review_verdict(&mut self, delivery: &ReviewVerdictDelivery) -> Result<(), String> {
        if let Some(receipt) = self.operation_receipt(&delivery.operation_id)? {
            if self
                .verdict_agent_owner(&receipt.agent_id, &delivery.project_path)
                .is_none()
                && self
                    .tracker_store()?
                    .redirect_review_verdict_delivery(&delivery.comment_id, &delivery.operation_id)
                    .map_err(|error| error.to_string())?
            {
                let redirected = self
                    .tracker_store()?
                    .load_review_verdict_delivery(&delivery.comment_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Verdict delivery disappeared")?;
                return self.deliver_review_verdict(&redirected);
            }
            return match receipt.status {
                OperationStatus::Delivered if receipt.execution_error.is_none() => {
                    self.tracker_store()?
                        .finish_review_verdict_delivery(
                            &delivery.comment_id,
                            Some(&receipt.agent_id),
                        )
                        .map_err(|error| error.to_string())?;
                    self.note_verdict_delivery_changed(delivery);
                    Ok(())
                }
                OperationStatus::Uncertain => Err(
                    "Verdict delivery is uncertain; retry explicitly to resend the same operation."
                        .into(),
                ),
                OperationStatus::Delivered => Err(receipt
                    .execution_error
                    .unwrap_or_else(|| "Verdict delivery failed.".into())),
                OperationStatus::Queued | OperationStatus::Claimed => Ok(()),
            };
        }
        let (project, task) = self.tracker_task(&delivery.task_id)?;
        let Some((owner, agent)) = self.review_verdict_recipient(delivery)? else {
            self.note_unreceived_verdict(&project, task, delivery)?;
            return Err(
                "Nobody received the verdict: no author or workspace agent is available.".into(),
            );
        };
        self.tracker_store()?
            .bind_review_verdict_delivery(&delivery.comment_id, &agent)
            .map_err(|error| error.to_string())?;
        let mut outgoing = delivery.clone();
        if outgoing.retry_context.is_some() {
            outgoing.retry_context = Some(
                self.tracker_store()?
                    .current_review_verdict_retry_context(&outgoing.task_id)
                    .map_err(|error| error.to_string())?,
            );
        }
        let params = self.current_thread_params(&json!({
            "entity_id":owner,"agent_id":agent,"operation_id":delivery.operation_id,
            "messages":verdict_messages(&verdict_message(&outgoing)),
        }))?;
        self.thread_post_frozen_task(&json!({
            "params":params,"envelope":super::super::dispatch::envelope_of(&task),
            "sender":null,"requester":null,
        }))?;
        self.note_verdict_delivery_changed(delivery);
        Ok(())
    }

    pub(in crate::app) fn review_verdict_recipient(
        &self,
        delivery: &ReviewVerdictDelivery,
    ) -> Result<Option<(String, String)>, String> {
        self.review_verdict_recipient_for_task(
            &delivery.task_id,
            &delivery.workspace_id,
            &delivery.project_path,
            delivery.recipient_agent_id.as_deref(),
        )
    }

    pub(in crate::app) fn review_verdict_recipient_for_task(
        &self,
        task: &str,
        workspace: &str,
        project_path: &str,
        bound_recipient: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        let recorded = match bound_recipient {
            Some(_) => None,
            None => self
                .tracker_store()?
                .review_verdict_author(task)
                .map_err(|error| error.to_string())?,
        };
        let author = bound_recipient.or(recorded.as_deref());
        if let Some(agent) = author {
            if let Some(owner) = self.verdict_agent_owner(agent, project_path) {
                return Ok(Some((owner, agent.to_string())));
            }
        }
        let Some(workspace) = self.workspaces.get(workspace) else {
            return Ok(None);
        };
        let Some(owner) = self.workspace_conversation_owner(workspace) else {
            return Ok(None);
        };
        Ok(self
            .entity_agents(&owner)?
            .primary()
            .map(|agent| (owner.clone(), agent.id.clone())))
    }

    fn verdict_agent_owner(&self, agent: &str, project_path: &str) -> Option<String> {
        let owner = self.entity_of_agent(agent)?;
        let run = self.runs.get(&owner)?;
        if run.run.state.is_terminal() {
            return None;
        }
        let project = self
            .projects
            .project_id_of(&owner)
            .and_then(|id| self.projects.get(id))?;
        (project.repo_path.to_string_lossy() == project_path).then_some(owner)
    }

    fn note_unreceived_verdict(
        &mut self,
        project: &str,
        mut task: crate::tracker::Task,
        delivery: &ReviewVerdictDelivery,
    ) -> Result<(), String> {
        let id = self
            .tracker_store()?
            .reserve_review_verdict_fallback_comment(&delivery.comment_id)
            .map_err(|error| error.to_string())?;
        if self
            .tracker_store()?
            .load_tracker_comment(&id)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(());
        }
        task.set_watched(true);
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::Build, task);
        write.comments.push(TaskComment {
            review_notice: Some(crate::tracker::ReviewNoticeKind::VerdictUndelivered),
            id,
            task_id: delivery.task_id.clone(),
            author: Actor::Build,
            body: crate::templates::fill(
                crate::templates::MESSAGE_REVIEW_VERDICT_UNRECEIVED,
                &[
                    ("revision", &delivery.revision.to_string()),
                    ("verdict_comment", &delivery.comment_id),
                ],
            ),
            anchor: None,
            reply_to: Some(delivery.comment_id.clone()),
            opinion: None,
            mentions_user: true,
            notifies_user: false,
            refs: vec![],
            attachments: vec![],
            created_at: now.clone(),
            author_context: None,
        });
        self.commit_task_write(project, write, &now)?;
        Ok(())
    }

    pub(crate) fn review_retry_verdict_delivery(
        &mut self,
        params: ReviewRetryDeliveryParams,
        _actor: Actor,
    ) -> Result<Value, String> {
        params.validate()?;
        let (project, _) = self.tracker_task(&params.task_id)?;
        let store = self.tracker_store()?.clone();
        let delivery = store
            .load_review_verdict_delivery(&params.comment_id)
            .map_err(|error| error.to_string())?
            .ok_or("Unknown verdict delivery")?;
        if delivery.task_id != params.task_id {
            return Err("Verdict delivery belongs to another task".into());
        }
        store
            .retry_review_verdict_delivery(&params.task_id, &params.comment_id)
            .map_err(|error| error.to_string())?;
        let delivery = store
            .load_review_verdict_delivery(&params.comment_id)
            .map_err(|error| error.to_string())?
            .ok_or("Unknown verdict delivery")?;
        if let Some(receipt) = self.operation_receipt(&delivery.operation_id)? {
            if matches!(
                receipt.status,
                OperationStatus::Uncertain | OperationStatus::Delivered
            ) {
                self.transition_delivery_operation(
                    &delivery.operation_id,
                    receipt.status,
                    OperationStatus::Queued,
                    None,
                )?;
                if let Some(intent) = &receipt.delivery {
                    self.queue_message_delivery(intent, Some(&receipt));
                }
            }
        }
        self.drain_review_verdict_deliveries()?;
        self.note_tasks_changed(&project, &params.task_id);
        let review = store
            .load_review(&params.task_id)
            .map_err(|error| error.to_string())?
            .ok_or("Unknown review")?;
        super::projection::result(&store, review)
    }

    fn note_verdict_delivery_changed(&self, delivery: &ReviewVerdictDelivery) {
        if let Some(project) = self
            .projects
            .iter()
            .find(|project| project.repo_path.to_string_lossy() == delivery.project_path)
        {
            self.note_tasks_changed(&project.id, &delivery.task_id);
        }
    }
}

fn verdict_message(delivery: &ReviewVerdictDelivery) -> String {
    let anchors = delivery
        .batch
        .iter()
        .map(|comment| {
            let anchor = &comment.anchor;
            let side = serde_json::to_value(anchor.side).expect("side serializes");
            let commit = anchor.commit.as_deref().unwrap_or("range");
            format!(
                "{}: {}:{} side={} commit={}",
                comment.comment_id, anchor.path, anchor.line, side, commit
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let payload = json!({
        "task_id":delivery.task_id,"task_number":delivery.task_number,
        "snapshot_id":delivery.snapshot_id,"revision":delivery.revision,
        "verdict":delivery.verdict,"verdict_comment_id":delivery.comment_id,
        "reason":delivery.reason,"reviewer":delivery.reviewer,
        "retry_context":delivery.retry_context,
        "batch":delivery.batch.iter().map(|comment|json!({
            "comment_id":comment.comment_id,"anchor":comment.anchor,
        })).collect::<Vec<_>>(),
    });
    let instruction = verdict_instruction(delivery);
    crate::templates::fill(
        crate::templates::MESSAGE_REVIEW_VERDICT,
        &[
            ("number", &delivery.task_number.to_string()),
            ("revision", &delivery.revision.to_string()),
            (
                "verdict",
                serde_json::to_value(delivery.verdict)
                    .expect("verdict serializes")
                    .as_str()
                    .unwrap(),
            ),
            ("comment_id", &delivery.comment_id),
            ("anchors", &anchors),
            (
                "payload",
                &serde_json::to_string_pretty(&payload).expect("delivery serializes"),
            ),
            ("instruction", &instruction),
        ],
    )
}

fn verdict_messages(body: &str) -> Vec<Value> {
    if body.len() <= 32_000 {
        return vec![json!({"body":body})];
    }
    let mut messages = Vec::new();
    let mut remaining = body;
    while !remaining.is_empty() {
        let mut end = remaining.len().min(30_000);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        let (part, tail) = remaining.split_at(end);
        // Boundary markers preserve whitespace even though ordinary message
        // intake trims the outside of every input message.
        messages.push(json!({"body":format!("Review verdict continuation:\n<part>{part}</part>")}));
        remaining = tail;
    }
    messages
}

fn verdict_instruction(delivery: &ReviewVerdictDelivery) -> String {
    if let Some(context) = &delivery.retry_context {
        let status = serde_json::to_value(context.current_status).expect("status serializes");
        return crate::templates::fill(
            crate::templates::MESSAGE_REVIEW_VERDICT_SUPERSEDED,
            &[
                ("status", status.as_str().expect("status is a string")),
                ("revision", &context.current_revision.to_string()),
            ],
        );
    }
    match delivery.verdict {
        crate::tracker::ReviewVerdict::RequestChanges => "Fix each comment, commit, `push_review`; reply on comments you disagree with.",
        crate::tracker::ReviewVerdict::Approve => "Approved. Take this forward as the work requires (for example `merge_review`, a deploy, or closing the work task), then comment on the review what you did.",
        crate::tracker::ReviewVerdict::Reject => "Rejected: stop work on this; do not merge. The rejection reason is in the verdict payload below.",
    }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_verdict_messages_preserve_unicode_and_whitespace_within_transport_limit() {
        let original = " λ review reason \n".repeat(5_000);
        let messages = verdict_messages(&original);
        assert!(messages.len() > 1);
        let rebuilt = messages
            .iter()
            .map(|message| {
                let body = message["body"].as_str().unwrap();
                assert!(body.len() <= 32_000);
                body.strip_prefix("Review verdict continuation:\n<part>")
                    .unwrap()
                    .strip_suffix("</part>")
                    .unwrap()
            })
            .collect::<String>();
        assert_eq!(rebuilt, original);
    }
}
