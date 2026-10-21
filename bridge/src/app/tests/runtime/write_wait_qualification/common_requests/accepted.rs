//! Consume exact native save/install evidence; never reconstruct a whole record.
//!
//! This proves only the state represented by this observer. A comment additionally
//! needs verify_saved(decoded, SavedEffect::Comment(actual_store_comment)); the
//! task observer does not prove that the comment body or author was persisted.
use super::*;
use crate::app::accepted_records::{
    AcceptedRecord, AcceptedTaskFields, AgentRecordWitness, TaskRecordWitness,
    TaskStatusDisposition,
};

impl CommonRequest {
    pub fn verify_accepted_record(
        &self,
        decoded: &Value,
        record: AcceptedRecord,
    ) -> Result<OracleWitness, String> {
        ensure(
            !matches!(&self.effect, ExpectedEffect::Comment { .. }),
            "a comment requires verify_accepted_comment and its independent saved comment",
        )?;
        accepted_record_witness(self, decoded, record)
    }

    pub fn verify_accepted_comment(
        &self,
        decoded: &Value,
        record: AcceptedRecord,
        saved: crate::tracker::TaskComment,
    ) -> Result<OracleWitness, String> {
        ensure(
            matches!(&self.effect, ExpectedEffect::Comment { .. }),
            "saved comment supplied for a non-comment request",
        )?;
        let accepted = accepted_record_witness(self, decoded, record)?;
        let comment = self.verify_saved(decoded, SavedEffect::Comment(saved))?;
        match (accepted, comment) {
            (
                OracleWitness::State { mut assertions, .. },
                OracleWitness::State {
                    assertions: saved, ..
                },
            ) => {
                assertions.extend(saved);
                Ok(OracleWitness::State {
                    oracle: format!("native-accepted-task-and-saved-comment/{}", self.method()),
                    assertions,
                })
            }
            _ => Err("comment and task record must both produce native state evidence".into()),
        }
    }
}

fn accepted_record_witness(
    request: &CommonRequest,
    decoded: &Value,
    record: AcceptedRecord,
) -> Result<OracleWitness, String> {
    ensure(
        decoded["ok"] == true && decoded["error"].is_null(),
        "accepted-record evidence requires the actual positive final reply",
    )?;
    if matches!(request.request, ActualRequest::Rpc { .. }) {
        ensure(
            decoded["id"].as_str() == Some(request.original_id.as_str()),
            "accepted-record RPC reply belongs to another original",
        )?;
    }
    let result = decoded.get("result").ok_or("positive reply lacks result")?;
    let mut fields = match record {
        AcceptedRecord::Agent(record) => verify_agent(request, result, record)?,
        AcceptedRecord::Task(record) => verify_task(request, result, record)?,
        AcceptedRecord::Seen(_) | AcceptedRecord::Presence(_) => {
            return Err("record kind does not prove requested Agent/Task effect".into());
        }
    };
    fields.push(assertion(
        "original_id",
        WitnessScalar::Text(request.original_id.clone()),
    ));
    fields.push(assertion("native_revision", WitnessScalar::Null));
    Ok(OracleWitness::State {
        oracle: format!("native-accepted-record/{}", request.method()),
        assertions: fields,
    })
}

fn verify_original(request: &CommonRequest, originals: &[String]) -> Result<(), String> {
    // Common topic/name/task writes are independent semantic commands, not
    // joined service work. Additional or repeated attribution is a failure;
    // the caller archives the untouched raw record before invoking this oracle.
    ensure(
        originals.len() == 1 && originals[0] == request.original_id,
        "accepted common record must belong to exactly this original",
    )
}

fn verify_agent(
    request: &CommonRequest,
    result: &Value,
    record: AgentRecordWitness,
) -> Result<Vec<OracleAssertion>, String> {
    verify_original(request, &record.original_ids)?;
    ensure(
        record.native_revision.is_none(),
        "agent has no native revision",
    )?;
    let (owner, agent, field, asked, accepted) = match &request.effect {
        ExpectedEffect::Topic {
            owner,
            agent,
            topic,
        } => (owner, agent, "topic", topic, record.topic.as_deref()),
        ExpectedEffect::Name { owner, agent, name } => {
            ensure(
                record.name_asked,
                "accepted name did not clear the naming request",
            )?;
            (owner, agent, "name", name, record.name.as_deref())
        }
        _ => return Err("agent witness cannot prove this requested effect".into()),
    };
    verify_agent_request(request, field, agent, asked)?;
    ensure(
        &record.owner == owner && &record.agent == agent,
        "accepted agent owner or ID differs from actual request target",
    )?;
    ensure(
        accepted == Some(asked.as_str()) && result[field].as_str() == accepted,
        "accepted agent field differs from the requested or returned value",
    )?;
    Ok(vec![
        assertion("saved_owner", WitnessScalar::Text(record.owner)),
        assertion("saved_agent_id", WitnessScalar::Text(record.agent)),
        assertion(
            &format!("saved_{field}"),
            WitnessScalar::Text(asked.clone()),
        ),
        assertion("saved_name_asked", WitnessScalar::Bool(record.name_asked)),
    ])
}

fn verify_agent_request(
    request: &CommonRequest,
    field: &str,
    agent: &str,
    asked: &str,
) -> Result<(), String> {
    let ActualRequest::Mcp {
        agent: caller,
        action,
    } = &request.request
    else {
        return Err("common topic/name evidence requires the actual Coding MCP request".into());
    };
    ensure(
        caller == agent,
        "topic/name MCP caller differs from saved agent",
    )?;
    let matched = match action {
        BridgeAction::SetTopic { topic } => field == "topic" && topic == asked,
        BridgeAction::SetName { name } => field == "name" && name == asked,
        _ => false,
    };
    ensure(
        matched,
        "topic/name expected effect differs from actual MCP action",
    )
}

fn verify_task(
    request: &CommonRequest,
    result: &Value,
    record: TaskRecordWitness,
) -> Result<Vec<OracleAssertion>, String> {
    verify_original(request, &record.original_ids)?;
    ensure(
        record.native_revision.is_none(),
        "task timestamp is not a native revision",
    )?;
    let (task, status) = match &request.effect {
        ExpectedEffect::TaskStatus { task, status, .. } => (task, Some(status.as_str())),
        ExpectedEffect::Comment { task, .. } => (task, None),
        _ => return Err("task witness cannot prove this requested effect".into()),
    };
    ensure(
        &record.accepted.id == task,
        "accepted task ID differs from requested task",
    )?;
    verify_task_projection(&record.accepted, &result["task"])?;
    if let Some(status) = status {
        ensure(
            record.accepted.status == status,
            "accepted status differs from requested status",
        )?;
    }
    verify_task_request(request, task, status)?;
    let mut fields = disposition_fields(&record, status.is_none())?;
    fields.extend([
        assertion("saved_task_id", WitnessScalar::Text(record.accepted.id)),
        assertion("saved_status", WitnessScalar::Text(record.accepted.status)),
        assertion(
            "saved_updated_at",
            WitnessScalar::Text(record.accepted.updated_at),
        ),
        assertion(
            "native_task_projection_matches_reply",
            WitnessScalar::Bool(true),
        ),
    ]);
    Ok(fields)
}

fn verify_task_projection(accepted: &AcceptedTaskFields, reply: &Value) -> Result<(), String> {
    ensure(
        !accepted.project_path.is_empty(),
        "accepted task project path is empty",
    )?;
    time::OffsetDateTime::parse(
        &accepted.updated_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|error| format!("invalid native task updated_at: {error}"))?;
    let actual = serde_json::to_value(accepted).map_err(|error| error.to_string())?;
    // project_path is an internal address; the RPC deliberately exposes project_id
    // instead. Never compare it to a fabricated value or infer a native revision.
    for field in [
        "id",
        "number",
        "state",
        "status",
        "title",
        "body",
        "priority",
        "labels",
        "updated_at",
    ] {
        ensure(
            reply.get(field) == actual.get(field),
            &format!("accepted native task {field} differs from this original's reply"),
        )?;
    }
    Ok(())
}

fn verify_task_request(
    request: &CommonRequest,
    task: &str,
    status: Option<&str>,
) -> Result<(), String> {
    let actor = match &request.effect {
        ExpectedEffect::TaskStatus { actor, .. } | ExpectedEffect::Comment { actor, .. } => actor,
        _ => return Err("task request lacks an actor expectation".into()),
    };
    let matched = match (&request.request, actor) {
        (ActualRequest::Rpc { method, params }, ActorExpectation::User) => {
            params["task_id"].as_str() == Some(task)
                && match status {
                    Some(status) => {
                        *method == "tasks.update" && params["status"].as_str() == Some(status)
                    }
                    None => *method == "tasks.comment",
                }
        }
        (ActualRequest::Mcp { agent, action }, ActorExpectation::Agent(expected)) => {
            agent == expected && mcp_task_matches(action, task, status)
        }
        _ => false,
    };
    ensure(matched, "task effect/actor differs from the actual request")
}

fn mcp_task_matches(action: &BridgeAction, task: &str, status: Option<&str>) -> bool {
    match (action, status) {
        (
            BridgeAction::TrackerMoveTask {
                task_id,
                status: asked,
                ..
            },
            Some(status),
        ) => task_id == task && asked == status,
        (BridgeAction::TrackerCommentTask { task_id, .. }, None) => task_id == task,
        _ => false,
    }
}

fn disposition_fields(
    record: &TaskRecordWitness,
    is_comment: bool,
) -> Result<Vec<OracleAssertion>, String> {
    match &record.status_disposition {
        TaskStatusDisposition::StatusNoop => Ok(vec![
            assertion(
                "task_status_disposition",
                WitnessScalar::Text("noop".into()),
            ),
            assertion("saved_move_event_id", WitnessScalar::Null),
        ]),
        TaskStatusDisposition::StatusChanged { event_id, from, to } => {
            ensure(
                !is_comment,
                "a plain common comment unexpectedly moved its task",
            )?;
            ensure(
                !event_id.is_empty() && from != to && to == &record.accepted.status,
                "accepted native moved event does not prove the accepted transition",
            )?;
            Ok(vec![
                assertion(
                    "task_status_disposition",
                    WitnessScalar::Text("changed".into()),
                ),
                assertion("saved_move_event_id", WitnessScalar::Text(event_id.clone())),
                assertion("saved_move_from", WitnessScalar::Text(from.clone())),
                assertion("saved_move_to", WitnessScalar::Text(to.clone())),
            ])
        }
    }
}
