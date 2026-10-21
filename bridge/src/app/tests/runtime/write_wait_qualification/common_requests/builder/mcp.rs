use super::*;
impl Builder<'_> {
    pub(super) fn mcp(&mut self, context: &SlotContext<'_>) -> Result<Parts, String> {
        let slot = context.slot;
        let id = slot.original_id.clone();
        let owner = slot.owner;
        let target = context.target;
        let task = context.task.clone();
        let second = slot.sequence / 40;
        let position = slot.sequence % 40;
        let targets = self.targets;
        let (request, effect) = match (second + position - 8) % 6 {
            0 => {
                let topic = format!("Qualify waits {}", slot.sequence);
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::SetTopic {
                            topic: topic.clone(),
                        },
                    },
                    ExpectedEffect::Topic {
                        owner: target.owner.clone(),
                        agent: target.agent.clone(),
                        topic,
                    },
                )
            }
            1 => {
                let name = format!("Qualifier Q{owner}{}", slot.sequence);
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::SetName { name: name.clone() },
                    },
                    ExpectedEffect::Name {
                        owner: target.owner.clone(),
                        agent: target.agent.clone(),
                        name,
                    },
                )
            }
            2 => {
                let status = if self.task_status[owner] % 2 == 0 {
                    "in_progress"
                } else {
                    "backlog"
                }
                .to_string();
                self.task_status[owner] += 1;
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::TrackerMoveTask {
                            task_id: task.clone(),
                            status: status.clone(),
                            track: Some(false),
                        },
                    },
                    ExpectedEffect::TaskStatus {
                        task,
                        status,
                        actor: ActorExpectation::Agent(target.agent.clone()),
                    },
                )
            }
            3 => {
                let text = body("MCP comment", &id)?;
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::TrackerCommentTask {
                            task_id: task.clone(),
                            body: text.clone(),
                            attachments: Vec::new(),
                            refs: Vec::new(),
                            track: Some(false),
                            notify_user: Some(false),
                            mention_user: Some(false),
                        },
                    },
                    ExpectedEffect::Comment {
                        task,
                        body: text,
                        actor: ActorExpectation::Agent(target.agent.clone()),
                    },
                )
            }
            4 => {
                let text = body("MCP post", &id)?;
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::PostThreadMessage {
                            still_working: true,
                            body: text.clone(),
                            options: Vec::new(),
                            attachments: Vec::new(),
                        },
                    },
                    ExpectedEffect::Posted {
                        owner: target.owner.clone(),
                        agent: target.agent.clone(),
                        thread: target.conversation.clone(),
                        body: text,
                        role: "agent",
                        operation_id: None,
                    },
                )
            }
            _ => {
                let sibling = targets
                    .active
                    .iter()
                    .find(|candidate| {
                        candidate.project == target.project && candidate.agent != target.agent
                    })
                    .ok_or("same-project live sibling missing")?;
                let text = body("MCP handoff", &id)?;
                (
                    ActualRequest::Mcp {
                        agent: target.agent.clone(),
                        action: BridgeAction::MessageAgent {
                            agent_id: sibling.agent.clone(),
                            body: text.clone(),
                            attachments: Vec::new(),
                        },
                    },
                    ExpectedEffect::Handoff {
                        caller: target.agent.clone(),
                        recipient_owner: sibling.owner.clone(),
                        recipient: sibling.agent.clone(),
                        thread: sibling.conversation.clone(),
                        body: text,
                    },
                )
            }
        };
        Ok(Parts {
            request,
            effect,
            durable_operation_id: None,
            predecessor: None,
        })
    }
}
