//! Waiting-on tools validate their whole request before forwarding scoped actions.
use super::*;

pub(super) fn extend_tools(tools: &mut Vec<Value>) {
    let ids = json!({"type":"array","maxItems":20,"items":{"type":"string","minLength":1},"description":"Same-project prerequisite task IDs. Done or closed resolves each edge; closed work requires assessment."});
    for tool in tools.iter_mut() {
        match tool["name"].as_str() {
            Some("create_task" | "link_task") => {
                tool["inputSchema"]["properties"]["waiting_on"] = ids.clone();
                let description = tool["description"].as_str().unwrap_or_default().to_string();
                tool["description"] = json!(format!("{description} waiting_on is atomic with the other links or creation. A held assignment creates no agent or workspace until every prerequisite is Done or closed."));
            }
            Some("assign_task") => {
                tool["inputSchema"]["properties"]["retry_deferred_id"] = json!({"type":"string","description":"Retry the exact saved failed delivery, preserving target, note, requester and generation. Mutually exclusive with assignee and note."});
                tool["inputSchema"]["required"] = json!(["task_id"]);
                tool["inputSchema"]["oneOf"] = json!([{"required":["assignee"],"not":{"required":["retry_deferred_id"]}}, {"required":["retry_deferred_id"],"not":{"anyOf":[{"required":["assignee"]},{"required":["note"]}]}}]);
            }
            _ => {}
        }
    }
    tools.push(json!({"name":"unlink_task","description":"Remove waiting-on edges only. Last-edge removal releases an existing deferred assignment; an unassigned task starts no worker.","inputSchema":{"type":"object","additionalProperties":false,"properties":{"task_id":{"type":"string"},"waiting_on":ids,"track":{"type":"boolean"}},"required":["task_id","waiting_on"]}}));
    tools.push(json!({"name":"list_task_dependents","description":"Read the tasks waiting on this same-project task. Bounded cursor pages include task summaries.","inputSchema":{"type":"object","additionalProperties":false,"properties":{"task_id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string"}},"required":["task_id"]}}));
}

pub(super) fn handle_call(id: &Value, name: &str, params: Option<&Value>) -> Option<Handled> {
    if !matches!(
        name,
        "create_task" | "link_task" | "unlink_task" | "list_task_dependents" | "assign_task"
    ) {
        return None;
    }
    if let Err(error) = refuse_authority(name, params) {
        return Some(refused(id.clone(), error));
    }
    if matches!(name, "create_task" | "link_task") && argument(params, "waiting_on").is_none() {
        return None;
    }
    if name == "assign_task" && argument(params, "retry_deferred_id").is_none() {
        return None;
    }
    Some(match action(name, params) {
        Ok(action) => acted(id.clone(), action),
        Err(error) => refused(id.clone(), error),
    })
}

fn refuse_authority(name: &str, params: Option<&Value>) -> Result<(), String> {
    let fields: &[&str] = match name {
        "create_task" => &[
            "track",
            "notify_user",
            "title",
            "body",
            "status",
            "labels",
            "priority",
            "attachments",
            "mention_user",
            "waiting_on",
        ],
        "link_task" => &[
            "task_id",
            "track",
            "workspace_id",
            "branch",
            "commit",
            "conversation_id",
            "parent_task_id",
            "parent_issue_id",
            "waiting_on",
        ],
        "unlink_task" => &["task_id", "waiting_on", "track"],
        "list_task_dependents" => &["task_id", "limit", "cursor"],
        "assign_task" => &[
            "task_id",
            "assignee",
            "note",
            "retry_deferred_id",
            "track",
            "notify_user",
        ],
        _ => &[],
    };
    let arguments = params
        .and_then(|p| p.get("arguments"))
        .and_then(Value::as_object)
        .ok_or("arguments must be an object")?;
    for key in arguments.keys() {
        if !fields.contains(&key.as_str()) {
            return Err(format!("{key} is not a caller field"));
        }
    }
    Ok(())
}

fn ids(params: Option<&Value>) -> Result<Vec<String>, String> {
    let values = argument(params, "waiting_on").ok_or("waiting_on is required")?;
    let values = values.as_array().ok_or("waiting_on must be an array")?;
    if values.len() > 100 {
        return Err("waiting_on exceeds 100 input ids".into());
    }
    let mut ids = values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 128)
                .map(str::to_string)
                .ok_or_else(|| "waiting_on needs nonempty task ids".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    if ids.len() > 20 {
        return Err("waiting_on exceeds 20 distinct tasks".into());
    }
    Ok(ids)
}

fn action(name: &str, params: Option<&Value>) -> Result<BridgeAction, String> {
    match name {
        "create_task" | "link_task" => {
            let waiting_on = ids(params)?;
            let mut without = params.cloned().unwrap_or(Value::Null);
            without["arguments"]
                .as_object_mut()
                .ok_or("arguments must be an object")?
                .remove("waiting_on");
            let handled = DoneServer::handle_task_tools_call(&json!(1), name, Some(&without))
                .ok_or("unknown tool")?;
            let action = handled
                .action
                .ok_or_else(|| handled.reply.unwrap_or_default())?;
            Ok(BridgeAction::TrackerWithWaitingOn {
                wrapped: Box::new(action),
                waiting_on,
            })
        }
        "unlink_task" => Ok(BridgeAction::TrackerUnlinkWaitingOn {
            task_id: current_id(&required_argument(params, "task_id")?),
            waiting_on: ids(params)?,
            track: optional_flag(params, "track"),
        }),
        "list_task_dependents" => {
            let cursor = argument(params, "cursor")
                .map(|value| {
                    value
                        .as_str()
                        .filter(|cursor| !cursor.trim().is_empty())
                        .map(str::to_string)
                        .ok_or("cursor must be a nonempty string")
                })
                .transpose()?;
            let limit = argument(params, "limit")
                .map(|v| {
                    v.as_u64()
                        .filter(|v| (1..=100).contains(v))
                        .ok_or("limit must be 1 to 100")
                })
                .transpose()?;
            Ok(BridgeAction::TrackerTaskDependents {
                task_id: current_id(&required_argument(params, "task_id")?),
                limit,
                cursor,
            })
        }
        "assign_task" => {
            if argument(params, "assignee").is_some()
                || argument(params, "note").is_some()
                || argument(params, "operation_id").is_some()
            {
                return Err(
                    "retry_deferred_id is mutually exclusive with assignee, note and operation_id"
                        .into(),
                );
            }
            Ok(BridgeAction::TrackerRetryTaskDelivery {
                task_id: current_id(&required_argument(params, "task_id")?),
                retry_deferred_id: required_argument(params, "retry_deferred_id")?,
                track: optional_flag(params, "track"),
                notify_user: optional_flag(params, "notify_user"),
            })
        }
        _ => Err("unknown waiting-on tool".into()),
    }
}
