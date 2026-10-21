use super::*;
impl Builder<'_> {
    pub(super) fn spa(&mut self, context: &SlotContext<'_>) -> Result<Parts, String> {
        let slot = context.slot;
        let id = slot.original_id.clone();
        let owner = slot.owner;
        let target = context.target;
        let task = context.task.clone();
        let second = slot.sequence / 40;
        let position = slot.sequence % 40;
        let trial = self.trial;
        let mut durable_operation_id = None;
        let (request, effect) = match (second * 8 + position) % 5 {
            0 => {
                let text = body("SPA post", &id)?;
                let operation = format!(
                    "qual.{}.trial{}.write.{:05}",
                    match trial.envelope {
                        Envelope::Small => "small",
                        Envelope::Large => "large",
                    },
                    trial.number,
                    slot.sequence
                );
                if !operation.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
                }) {
                    return Err("invalid durable operation_id syntax".into());
                }
                durable_operation_id = Some(operation.clone());
                (
                    ActualRequest::Rpc {
                        method: "thread.post",
                        params: json!({"entity_id":target.owner,"agent_id":target.agent,"thread_id":target.thread,"conversation_id":target.conversation,"operation_id":operation,"body":text,"interrupt":false}),
                    },
                    ExpectedEffect::Posted {
                        owner: target.owner.clone(),
                        agent: target.agent.clone(),
                        thread: target.conversation.clone(),
                        body: text,
                        role: "user",
                        operation_id: Some(operation),
                    },
                )
            }
            1 => (
                ActualRequest::Rpc {
                    method: "entity.seen",
                    params: json!({"entity_id":target.owner,"agent_id":target.agent,"thread_id":target.thread,"conversation_id":target.conversation}),
                },
                ExpectedEffect::Seen {
                    owner: target.owner.clone(),
                    agent: target.agent.clone(),
                    thread: target.conversation.clone(),
                },
            ),
            2 => (
                ActualRequest::Rpc {
                    method: "user.present",
                    params: json!({}),
                },
                ExpectedEffect::UserPresent,
            ),
            3 => {
                let text = body("SPA comment", &id)?;
                (
                    ActualRequest::Rpc {
                        method: "tasks.comment",
                        params: json!({"task_id":task,"body":text}),
                    },
                    ExpectedEffect::Comment {
                        task,
                        body: text,
                        actor: ActorExpectation::User,
                    },
                )
            }
            _ => {
                let status = if self.task_status[owner] % 2 == 0 {
                    "in_progress"
                } else {
                    "backlog"
                }
                .to_string();
                self.task_status[owner] += 1;
                (
                    ActualRequest::Rpc {
                        method: "tasks.update",
                        params: json!({"task_id":task,"status":status}),
                    },
                    ExpectedEffect::TaskStatus {
                        task,
                        status,
                        actor: ActorExpectation::User,
                    },
                )
            }
        };
        Ok(Parts {
            request,
            effect,
            durable_operation_id,
            predecessor: None,
        })
    }
}
