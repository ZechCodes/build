use super::post::post_operation_value;
use crate::app::{optional_nonempty_string, require_str};
use crate::mcp::BridgeAction;
use crate::operation::{OperationPayload, OperationReceipt};
use crate::store::now_rfc3339;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub(in crate::app) struct ReviewerMessage {
    pub body: String,
    pub anchor: Option<crate::thread::MessageAnchor>,
    pub viewing_context: Option<crate::thread::ViewingContext>,
    /// The agent that wrote these words, when one did. Every parser leaves it
    /// `None` — the human is what a post is until something says otherwise —
    /// and only [`ReviewerMessage::sent_by`] fills it in.
    pub from_agent: Option<crate::thread::AgentIdentity>,
    /// The task these words handed over, when assigning one is what sent
    /// them. Every parser leaves it `None`; only a dispatch fills it in.
    pub from_task: Option<crate::thread::TaskEnvelope>,
}

impl ReviewerMessage {
    /// Say whose words these are. The role stays the user's: that is the side
    /// of the conversation an instruction arrives on whoever wrote it.
    pub(in crate::app) fn sent_by(mut self, sender: Option<&crate::thread::AgentIdentity>) -> Self {
        self.from_agent = sender.cloned();
        self
    }

    /// Say which task these words handed over. The role stays the user's for
    /// the same reason: a task arriving is an instruction arriving.
    pub(in crate::app) fn about_task(mut self, task: Option<&crate::thread::TaskEnvelope>) -> Self {
        self.from_task = task.cloned();
        self
    }
}

/// What a post is recorded as beyond its words: the operation it is, who wrote
/// it, and who asked for it.
///
/// One value rather than three arguments because they travel together the
/// whole way down — the sender rides onto the message, and the requester onto
/// the receipt, where it stays as the history of who wanted this.
#[derive(Debug, Clone, Default)]
pub(in crate::app) struct PostOrigin {
    pub operation_id: Option<String>,
    /// The agent that wrote the words, when an agent did.
    pub from_agent: Option<crate::thread::AgentIdentity>,
    /// The agent that asked for this post, when an agent did.
    pub requested_by: Option<crate::operation::OperationRequester>,
    /// The task this post handed over, when assigning one is what sent it.
    ///
    /// Independent of the two above: the human assigning a task carries an
    /// envelope and no sender, an agent assigning one carries both.
    pub from_task: Option<crate::thread::TaskEnvelope>,
}

impl PostOrigin {
    /// A post with an operation id and no sender: the human's, which is what
    /// every post off the wire is.
    pub(in crate::app) fn human(operation_id: Option<String>) -> Self {
        PostOrigin {
            operation_id,
            from_agent: None,
            from_task: None,
            requested_by: None,
        }
    }

    /// A post that hands a task over. The sender, if any, is added by the
    /// caller that knows there was one: a dispatch the human asked for has
    /// none, and a dispatch an agent asked for wears it.
    pub(in crate::app) fn handing_over(
        task: crate::thread::TaskEnvelope,
        operation_id: Option<String>,
    ) -> Self {
        PostOrigin {
            operation_id,
            from_agent: None,
            requested_by: None,
            from_task: Some(task),
        }
    }

    /// A post one agent asked another for. It wears the asker — stamped by the
    /// caller, which is the only place that knows where the asker was speaking
    /// from — and the receipt remembers the conversation it was asked from.
    pub(in crate::app) fn asked_by(
        requester: crate::operation::OperationRequester,
        sender: crate::thread::AgentIdentity,
    ) -> Self {
        PostOrigin {
            operation_id: None,
            from_agent: Some(sender),
            requested_by: Some(requester),
            from_task: None,
        }
    }

    /// The sender as a message wears it.
    pub(in crate::app) fn sender(&self) -> Option<crate::thread::AgentIdentity> {
        self.from_agent.clone()
    }

    /// The task a post hands over, as a message wears it.
    pub(in crate::app) fn task(&self) -> Option<crate::thread::TaskEnvelope> {
        self.from_task.clone()
    }
}

pub(in crate::app) const MAX_OPERATION_ID_BYTES: usize = 128;

pub(in crate::app) fn apply_thread_action(
    thread: &mut crate::thread::Thread,
    action: BridgeAction,
    now: &str,
) -> Result<Value, String> {
    match action {
        BridgeAction::PostThreadMessage {
            body,
            still_working,
            options,
        } => {
            let body = body.trim();
            if body.is_empty() {
                return Err("message body must not be empty".to_string());
            }
            if body.len() > 32_000 {
                return Err("message body exceeds 32000 bytes".to_string());
            }
            let message_id = thread.post_agent_offering(body, options, now, still_working);
            Ok(json!({ "message_id": message_id }))
        }
        // A search spans every conversation the agent may read, which one
        // thread cannot see — the daemon answers it before dispatch gets here.
        BridgeAction::SearchConversation { .. } => {
            Err("search_conversation is answered by the daemon, not one conversation".to_string())
        }
        // The topic lives on the agent's record, which one thread cannot see.
        BridgeAction::SetTopic { .. } => {
            Err("set_topic is answered by the daemon, not one conversation".to_string())
        }
        // Every other surface's tools are about work items, workspaces and
        // agents, so none of them is a thread operation. The socket refuses
        // them before this point; this arm is the type system agreeing, and it
        // names the surface the tool is actually on.
        elsewhere => Err(format!(
            "{} is a {} tool and reaches no conversation",
            elsewhere.tool_name(),
            elsewhere.surface_name()
        )),
    }
}

pub(in crate::app) fn parse_thread_inputs(
    params: &Value,
    artifact: crate::thread::ArtifactKind,
    legacy_field: &str,
) -> Result<Vec<ReviewerMessage>, String> {
    if let Some(messages) = params.get("messages") {
        let messages = messages
            .as_array()
            .ok_or_else(|| "messages must be an array".to_string())?;
        if messages.is_empty() {
            return Err("messages must not be empty".to_string());
        }
        if messages.len() > 100 {
            return Err("messages must contain at most 100 entries".to_string());
        }
        return messages
            .iter()
            .map(|message| {
                let body = message
                    .get("body")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|body| !body.is_empty())
                    .ok_or_else(|| "every message requires a non-empty body".to_string())?;
                if body.len() > 32_000 {
                    return Err("message body exceeds 32000 bytes".to_string());
                }
                let anchor =
                    parse_message_anchor(message.get("anchor").unwrap_or(&Value::Null), artifact)?;
                let viewing_context = parse_viewing_context(message.get("viewing_context"))?;
                Ok(ReviewerMessage {
                    body: body.to_string(),
                    anchor,
                    viewing_context,
                    from_agent: None,
                    from_task: None,
                })
            })
            .collect();
    }
    let body = params
        .get(legacy_field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .ok_or_else(|| format!("missing required param: {legacy_field} or messages"))?;
    if body.len() > 32_000 {
        return Err(format!("{legacy_field} exceeds 32000 bytes"));
    }
    Ok(vec![ReviewerMessage {
        body: body.to_string(),
        anchor: None,
        viewing_context: parse_viewing_context(params.get("viewing_context"))?,
        from_agent: None,
        from_task: None,
    }])
}

pub(in crate::app) fn parse_viewing_context(
    value: Option<&Value>,
) -> Result<Option<crate::thread::ViewingContext>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let context: crate::thread::ViewingContext = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid viewing_context: {error}"))?;
    context.normalize().map(Some)
}

pub(in crate::app) fn normalize_post_viewing_contexts(params: &Value) -> Result<Value, String> {
    let mut normalized = params.clone();
    if let Some(messages) = normalized.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            normalize_context_field(message)?;
        }
    } else {
        normalize_context_field(&mut normalized)?;
    }
    Ok(normalized)
}

fn normalize_context_field(container: &mut Value) -> Result<(), String> {
    let context = parse_viewing_context(container.get("viewing_context"))?;
    if let Some(context) = context {
        container["viewing_context"] =
            serde_json::to_value(context).expect("viewing context always serializes");
    }
    Ok(())
}

pub(in crate::app) fn optional_operation_id(params: &Value) -> Result<Option<String>, String> {
    optional_nonempty_string(params, "operation_id")?
        .map(|operation_id| {
            if operation_id.len() > MAX_OPERATION_ID_BYTES {
                return Err(format!(
                    "operation_id exceeds {MAX_OPERATION_ID_BYTES} bytes"
                ));
            }
            if !operation_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            }) {
                return Err("operation_id contains unsupported characters".to_string());
            }
            Ok(operation_id.to_string())
        })
        .transpose()
}

pub(in crate::app) fn required_operation_id(params: &Value) -> Result<String, String> {
    optional_operation_id(params)?.ok_or_else(|| "missing required param: operation_id".to_string())
}

pub(in crate::app) fn optional_choice_revision(params: &Value) -> Result<Option<u64>, String> {
    match params.get("choice_revision") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| "choice_revision must be a non-negative integer".to_string()),
    }
}

/// Parse `thread.post`'s single `body` (+ optional `anchor`) by funneling it
/// through [`parse_thread_inputs`]'s batch validator, so body limits and
/// anchor artifact-matching stay single-sourced.
///
/// `carries_attachments` waives the non-empty-body rule and nothing else: a
/// screenshot on its own IS the message, and demanding a caption for it would
/// only produce "see attached".
pub(in crate::app) fn parse_thread_post_input(
    params: &Value,
    artifact: crate::thread::ArtifactKind,
    carries_attachments: bool,
) -> Result<Vec<ReviewerMessage>, String> {
    let body = params.get("body").cloned().unwrap_or(Value::Null);
    let anchor = params.get("anchor").cloned().unwrap_or(Value::Null);
    let empty_body = body.as_str().map(str::trim).unwrap_or_default().is_empty();
    if carries_attachments && empty_body {
        let anchor = parse_message_anchor(&anchor, artifact)?;
        return Ok(vec![ReviewerMessage {
            body: String::new(),
            anchor,
            viewing_context: parse_viewing_context(params.get("viewing_context"))?,
            from_agent: None,
            from_task: None,
        }]);
    }
    parse_thread_inputs(
        &json!({
            "messages": [{
                "body": body,
                "anchor": anchor,
                "viewing_context": params.get("viewing_context"),
            }]
        }),
        artifact,
        "body",
    )
}

pub(in crate::app) fn parse_thread_post_messages(
    params: &Value,
    artifact: crate::thread::ArtifactKind,
    carries_attachments: bool,
) -> Result<Vec<ReviewerMessage>, String> {
    if params.get("messages").is_some() {
        return parse_thread_inputs(params, artifact, "body");
    }
    parse_thread_post_input(params, artifact, carries_attachments)
}

pub(in crate::app) fn parse_message_anchor(
    value: &Value,
    artifact: crate::thread::ArtifactKind,
) -> Result<Option<crate::thread::MessageAnchor>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let anchor: crate::thread::MessageAnchor = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid message anchor: {error}"))?;
    if anchor.artifact != artifact {
        return Err(format!(
            "message anchor artifact must be {}",
            artifact.as_str()
        ));
    }
    Ok(Some(anchor))
}

/// The choice the reviewer submitted from an agent's suggested actions, when
/// this post is one. `None` for a typed message, which is every other post.
pub(in crate::app) fn parse_option_choice(
    params: &Value,
) -> Result<Option<crate::thread::OptionChoice>, String> {
    let Some(reply) = params.get("option_reply").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let message_id = require_str(reply, "message_id")?;
    let option_ids = reply
        .get("option_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| "option_reply.option_ids must be an array".to_string())?
        .iter()
        .map(|id| {
            id.as_str()
                .map(str::to_string)
                .ok_or_else(|| "every option id must be a string".to_string())
        })
        .collect::<Result<Vec<String>, String>>()?;
    Ok(Some(crate::thread::OptionChoice {
        message_id,
        option_ids,
    }))
}

/// Append what the reviewer sent: the words they typed, or the options they
/// pressed.
///
/// A choice is recorded on the offer that made it and posted as an ordinary
/// reviewer message, so what the agent reads is the same shape either way. The
/// caller validated it against this very thread a moment earlier, under the
/// same lock — the fallback is there so that if the two ever came apart the
/// reviewer's words still land, unmarked, rather than vanishing.
pub(in crate::app) fn append_reviewer_messages(
    thread: &mut crate::thread::Thread,
    messages: Vec<ReviewerMessage>,
    attachments: Vec<crate::thread::MessageAttachment>,
    choice: Option<&crate::thread::OptionChoice>,
) -> Option<u64> {
    if let Some(choice) = choice {
        if thread.post_option_reply(choice, &now_rfc3339()).is_ok() {
            if let Some(crate::thread::ThreadItem::Message(reply)) = thread.items.last_mut() {
                reply.viewing_context = messages
                    .first()
                    .and_then(|message| message.viewing_context.clone())
                    .map(Box::new);
            }
            return last_appended_sequence(thread);
        }
    }
    append_user_thread_messages_with_attachments(thread, messages, attachments)
}

pub(in crate::app) fn append_operation_reviewer_messages(
    thread: &mut crate::thread::Thread,
    messages: Vec<ReviewerMessage>,
    attachments: Vec<crate::thread::MessageAttachment>,
    choice: Option<&crate::thread::OptionChoice>,
    operation_id: Option<&str>,
) -> (Option<u64>, Option<OperationPayload>) {
    let previous_sequence = thread.last_sequence();
    let prior_context = thread.operation_prior_context(crate::orchestrator::CATCH_UP_MESSAGES);
    let posted_sequence = append_reviewer_messages(thread, messages, attachments, choice);
    let payload = posted_sequence.map(|end_sequence| {
        let messages = if let Some(operation_id) = operation_id {
            thread.bind_operation_messages(operation_id, previous_sequence, end_sequence)
        } else {
            thread
                .items
                .iter()
                .filter_map(|item| match item {
                    crate::thread::ThreadItem::Message(message)
                        if message.role == crate::thread::MessageRole::User
                            && message.sequence > previous_sequence
                            && message.sequence <= end_sequence =>
                    {
                        Some(message.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        OperationPayload {
            start_sequence: messages
                .first()
                .map(|message| message.sequence)
                .unwrap_or(end_sequence),
            end_sequence,
            messages,
            prior_context,
            // Decided by whoever queues the delivery, which is where the
            // agent record can be read.
            ask_to_name: false,
            tells_sender_context: false,
        }
    });
    (posted_sequence, payload)
}

pub(in crate::app) fn append_user_thread_messages(
    thread: &mut crate::thread::Thread,
    messages: Vec<ReviewerMessage>,
) -> Option<u64> {
    append_user_thread_messages_with_attachments(thread, messages, Vec::new())
}

pub(in crate::app) fn last_appended_sequence(thread: &crate::thread::Thread) -> Option<u64> {
    thread.items.last().map(crate::thread::ThreadItem::sequence)
}

/// Append reviewer messages, hanging any attachments off the last of them.
///
/// Last rather than first because `thread.post` — the only caller that sends
/// files — posts exactly one message, and a batch sender's files would belong
/// with its closing note rather than its first line comment.
pub(in crate::app) fn append_user_thread_messages_with_attachments(
    thread: &mut crate::thread::Thread,
    messages: Vec<ReviewerMessage>,
    attachments: Vec<crate::thread::MessageAttachment>,
) -> Option<u64> {
    let now = now_rfc3339();
    let last = messages.len().saturating_sub(1);
    for (index, message) in messages.into_iter().enumerate() {
        let from_agent = message.from_agent;
        let from_task = message.from_task;
        if index == last && !attachments.is_empty() {
            thread.post_user_with_context_and_attachments(
                message.body,
                message.anchor,
                message.viewing_context,
                attachments.clone(),
                &now,
            );
        } else {
            thread.post_user_with_context(
                message.body,
                message.anchor,
                message.viewing_context,
                &now,
            );
        }
        // Set on the message just pushed, the way the attachments and the
        // viewing context are: every other caller would have to pass `None`
        // through the shared post arm otherwise.
        if let Some(from_agent) = from_agent {
            thread.wear_sender(from_agent);
        }
        if let Some(from_task) = from_task {
            thread.wear_task(from_task);
        }
    }
    last_appended_sequence(thread)
}

pub(in crate::app) fn with_posted_sequence(view: Value, sequence: Option<u64>) -> Value {
    let mut view = view;
    if let (Some(map), Some(sequence)) = (view.as_object_mut(), sequence) {
        map.insert("posted_sequence".to_string(), json!(sequence));
    }
    view
}

pub(in crate::app) fn with_post_receipt(
    view: Value,
    sequence: Option<u64>,
    receipt: Option<&OperationReceipt>,
) -> Value {
    let mut view = with_posted_sequence(view, sequence);
    let Some(receipt) = receipt else {
        return view;
    };
    let Some(object) = view.as_object_mut() else {
        return post_operation_value(receipt);
    };
    let receipt = post_operation_value(receipt);
    object.extend(
        receipt
            .as_object()
            .expect("an operation receipt serializes as an object")
            .clone(),
    );
    view
}
