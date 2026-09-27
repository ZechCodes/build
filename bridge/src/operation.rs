//! Durable receipts for reviewer operations that cross a provider boundary.
//!
//! A receipt is committed with the transcript mutation it acknowledges.  Its
//! delivery intent is then claimed durably before any provider-facing write.
//! That ordering makes restart recovery conservative: queued work is safe to
//! replay, while a claim left behind by a dead process is explicitly uncertain.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::models::{AgentProvider, ModelChoice};
use crate::thread::ThreadMessage;

pub const THREAD_POST_METHOD: &str = "thread.post";
pub const NATIVE_REVIEWER_MESSAGES_HEADING: &str = "Exact accepted messages:";
pub(crate) const AGENT_NAME_NOTE: &str =
    "\nYou have no name yet. Before you answer, call set_name with one or two meaningful \
words for what you are working on — \"Tracker\", \"Rail scroll\", \"Transport\". It is \
what the user sees instead of \"Agent 1\" everywhere you are named, so pick something \
they would recognise from across a list. Then answer normally.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Queued,
    Claimed,
    Delivered,
    Uncertain,
}

impl OperationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryIntent {
    pub root: PathBuf,
    pub owner_id: String,
    pub agent_id: String,
    pub model_choice: ModelChoice,
    pub choice_revision: u64,
    pub interrupt: bool,
    /// Immutable conversation slice this provider turn is allowed to consume.
    /// `None` only decodes short-lived pre-fence development receipts; recovery
    /// treats those as ambiguous instead of issuing an unrestricted read.
    #[serde(default)]
    pub payload: Option<OperationPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPayload {
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub messages: Vec<ThreadMessage>,
    /// Bounded conversation context as it stood immediately before acceptance.
    pub prior_context: String,
    /// Whether this turn also asks the agent to name itself.
    ///
    /// Decided where the agent record is readable rather than here — this
    /// carries the answer to the prompt. Absent on every payload written
    /// before names existed, which is what `default` is for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_to_name: bool,
    /// Whether this turn tells its reader how full each sending agent's
    /// context was (#68). Only the project agent's are: it is the one that
    /// decides who takes the next task. Decided where the recipient is known,
    /// like [`ask_to_name`](Self::ask_to_name).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tells_sender_context: bool,
}

impl OperationPayload {
    pub fn delivery_prompt(
        &self,
        operation_id: &str,
        cold: bool,
        provider: AgentProvider,
    ) -> String {
        self.render_delivery_prompt(
            format!(
                "Process only reviewer operation `{operation_id}` (conversation sequences {} through {}).",
                self.start_sequence, self.end_sequence
            ),
            cold,
            provider,
        )
    }

    pub fn legacy_delivery_prompt(&self, cold: bool, provider: AgentProvider) -> String {
        self.render_delivery_prompt(
            format!(
                "Process only the newly delivered reviewer messages (conversation sequences {} through {}).",
                self.start_sequence, self.end_sequence
            ),
            cold,
            provider,
        )
    }

    fn render_delivery_prompt(&self, scope: String, cold: bool, provider: AgentProvider) -> String {
        if let Some(command) = self.unadorned_command(provider) {
            return command.to_string();
        }
        let messages = serde_json::to_string_pretty(&self.messages)
            .expect("operation messages always serialize");
        let context = if cold && !self.prior_context.is_empty() {
            format!(
                "\nConversation context before this operation:\n{}\n",
                self.prior_context
            )
        } else {
            String::new()
        };
        // Keep the reviewer's words first. Provider slash commands are parsed
        // only when the slash is the first token; putting Build's delivery
        // envelope ahead of it turns commands such as `/goal ...` into prose.
        let sender = self.sender_note();
        let sender_context = self.sender_context_note();
        let workspace = self.workspace_note();
        let task = self.task_note();
        let looking = self.viewing_task_note();
        let naming = self.name_note();
        let user_prompt = self
            .messages
            .first()
            .map(|message| message.body.trim())
            .filter(|body| !body.is_empty())
            .unwrap_or("Review the exact accepted messages below.");
        let prompt = format!("{user_prompt}\n\n{scope}{context}{sender}{sender_context}{workspace}{task}{looking}{naming}\nThis native payload replaces the former message-fetch protocol. Build tracks delivery; process these messages directly without fetching or acknowledging them through MCP.\n{NATIVE_REVIEWER_MESSAGES_HEADING}\n{messages}");
        if cold {
            crate::orchestrator::conversation_prompt(&prompt)
        } else {
            prompt
        }
    }

    /// One line naming the agent these words came from, when an agent sent
    /// them rather than the human, and how to write back to it. Empty for
    /// everything the human said.
    ///
    /// The payload carries `from_agent` on the message itself, but the message
    /// is JSON inside a prompt written in the user's voice, and the sentence
    /// around it is what an agent actually reads. So the envelope says it too:
    /// this is work being handed over, the person to answer is not here, and
    /// the reply handle is the id spelled out rather than something to go and
    /// look up.
    fn sender_note(&self) -> String {
        let mut senders: Vec<&str> = self
            .messages
            .iter()
            .filter_map(|message| message.from_agent.as_deref())
            .map(|sender| sender.id.as_str())
            .collect();
        senders.dedup();
        if senders.is_empty() {
            return String::new();
        }
        let named: Vec<String> = senders
            .iter()
            .map(|sender| format!("agent `{sender}`"))
            .collect();
        let handles: Vec<String> = senders.iter().map(|sender| format!("`{sender}`")).collect();
        format!(
            "\nThese messages came from {}, not from the user. Reply with message_agent to {}.\n",
            named.join(" and "),
            handles.join(" and ")
        )
    }

    /// One line per sending agent saying how full its context was when it
    /// wrote — "Rail scroll is at 190k of 200k (95%, compacts at 200k)." — for the project agent,
    /// which is choosing who takes the next task. Empty for every other
    /// reader, for a notice Build wrote, and for a sender with no reading.
    fn sender_context_note(&self) -> String {
        if !self.tells_sender_context {
            return String::new();
        }
        let mut lines: Vec<String> = self
            .messages
            .iter()
            .filter(|message| !message.from_build)
            .filter_map(|message| message.from_agent.as_deref())
            .filter_map(|sender| {
                let author = sender
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Agent `{}`", sender.id));
                sender
                    .context
                    .as_ref()
                    .map(|reading| reading.sentence(&author))
            })
            .collect();
        lines.dedup();
        lines.iter().map(|line| format!("\n{line}\n")).collect()
    }

    /// What to do about the task this turn was handed, when being HANDED one
    /// is what sent it. Empty for every other message.
    ///
    /// Only for an assignment, and the check is `from_build`: a tracking
    /// notice wears a task too, and it is Build telling a watcher that
    /// somebody else changed something. Telling that watcher to read the task
    /// before it starts, comment its progress and move the card to In review
    /// is telling it to take over work nobody gave it — which is what a
    /// tracker was being told until #35.
    ///
    /// The body already says who assigned what — it is the notice — so this
    /// does not say it again. What it adds is the id, which the notice has no
    /// room for and `get_task` needs, and the two tools that keep the task
    /// honest about where the work got to. The payload is JSON inside a prompt
    /// written in the user's voice, and the sentence around it is what an agent
    /// actually acts on.
    fn task_note(&self) -> String {
        self.messages
            .iter()
            .filter(|message| !message.from_build)
            .filter_map(|message| message.from_task.as_deref())
            .next()
            .map_or_else(String::new, |task| {
                format!(
                    "\nThe task is `{}` — read it with get_task before you start. Comment your \
                     progress on it with comment_task, and move it to In review with move_task \
                     when you report Complete.\n",
                    task.task_id
                )
            })
    }

    /// One line naming the workspace the user was standing in, when the message
    /// says which. Empty for everything sent from nowhere in particular.
    ///
    /// The workspace rides the message's viewing context, but that context is
    /// JSON inside a prompt written in the user's voice — and the project's
    /// agent is reachable from every workspace in the project, so "this
    /// workspace" is a question the envelope has to answer in prose.
    fn workspace_note(&self) -> String {
        self.messages
            .iter()
            .filter_map(|message| message.viewing_context.as_deref())
            .find_map(crate::thread::ViewingContext::workspace)
            .map_or_else(String::new, |(workspace_id, name)| {
                format!("\nThe user sent this from workspace \"{name}\" ({workspace_id}).\n")
            })
    }

    /// Ask the agent to name itself, on the one turn that asks.
    ///
    /// The first turn of an agent that has no name, and only then: an agent
    /// that was asked and did not do it has decided, and asking
    /// again on every message would be nagging in the user's voice.
    fn name_note(&self) -> String {
        if !self.ask_to_name {
            return String::new();
        }
        AGENT_NAME_NOTE.to_string()
    }

    /// One line naming the task the user was LOOKING at, when the message
    /// says which. Empty for everything sent from anywhere else.
    ///
    /// Not the same sentence as [`Self::task_note`], and deliberately not:
    /// that one hands the agent work and names the two tools that keep the
    /// task honest about it. This one answers "this task" for a user who is
    /// standing on the board and asking about what is on screen — the agent
    /// has not been given the task, only pointed at it.
    ///
    /// It carries no body, because the item does not: a task moves on after
    /// the message is sent, so the agent is told to go and read it rather than
    /// handed a copy that reads as current and is not.
    fn viewing_task_note(&self) -> String {
        self.messages
            .iter()
            .filter_map(|message| message.viewing_context.as_deref())
            .find_map(crate::thread::ViewingContext::task)
            .map_or_else(String::new, |(number, title, task_id)| {
                format!(
                    "\nThe user is looking at task #{number} \"{title}\" (`{task_id}`); read it \
                     with get_task before answering about it.\n"
                )
            })
    }

    /// Commands that must reach the provider's command parser byte-for-byte.
    /// Claude and the Codex app server own `/compact`; both provider families
    /// own `/clear`. A payload
    /// containing anything else is a reviewer turn and keeps its delivery
    /// envelope, after the user's leading prompt.
    fn unadorned_command(&self, provider: AgentProvider) -> Option<&str> {
        let [message] = self.messages.as_slice() else {
            return None;
        };
        let body = message.body.trim();
        crate::harness::harness_for(provider)
            .requires_unadorned_command(body)
            .then_some(body)
    }

    pub(crate) fn requires_unadorned_delivery(&self, provider: AgentProvider) -> bool {
        self.unadorned_command(provider).is_some()
    }
}

/// Who asked for an operation, when an agent did rather than the human.
///
/// The message itself already wears the sender — `from_agent` on the
/// `ThreadMessage` says who wrote the words. This says where the ANSWER is
/// owed: the agent's own conversation, which the identity on a message does not
/// carry and which nothing else on the receipt names, because every other
/// address on it is the recipient's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequester {
    /// The agent that asked. Its id is what the message wears.
    pub agent_id: String,
    /// The conversation owner that agent belongs to.
    pub entity_id: String,
    /// The agent's own conversation, which an answer is posted back into.
    pub conversation_id: String,
}

impl OperationRequester {
    /// The sender as a message wears it.
    pub fn identity(&self) -> crate::thread::AgentIdentity {
        crate::thread::AgentIdentity::new(self.agent_id.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub method: String,
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub choice_revision: u64,
    pub posted_sequence: u64,
    pub message_start_sequence: u64,
    pub status: OperationStatus,
    /// A known terminal delivery failure. Absence means either delivery is
    /// still pending/ambiguous or the provider handoff completed normally.
    #[serde(default)]
    pub execution_error: Option<String>,
    pub request_hash: String,
    pub delivery: Option<DeliveryIntent>,
    /// The agent that asked for this operation, when one did. Absent is the
    /// human, whose answer comes back to the screen they are looking at.
    #[serde(default)]
    pub requested_by: Option<OperationRequester>,
}

impl OperationReceipt {
    pub fn wire_value(&self) -> Value {
        serde_json::json!({
            "operation_id": self.operation_id,
            "method": self.method,
            "entity_id": self.entity_id,
            "agent_id": self.agent_id,
            "conversation_id": self.conversation_id,
            "choice_revision": self.choice_revision,
            "posted_sequence": self.posted_sequence,
            "message_start_sequence": self.message_start_sequence,
            "status": self.status,
            "operation_error": self.execution_error,
        })
    }

    pub fn matches_request(&self, method: &str, request_hash: &str) -> bool {
        self.method == method && self.request_hash == request_hash
    }
}

/// Hash a request independently of JSON object insertion order. The operation
/// id itself is excluded: it names the receipt and is not part of its payload.
pub fn request_hash(method: &str, params: &Value) -> String {
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("operation_id");
        object.remove("thread_limit");
        object.remove("thread_after_sequence");
        object.remove("before_sequence");
    }
    let normalized = normalize_json(params);
    let envelope = serde_json::json!({ "method": method, "params": normalized });
    format!("{:x}", Sha256::digest(envelope.to_string().as_bytes()))
}

/// Bind a post's semantic payload to the address the bridge validated. Read
/// projection fields are deliberately absent, so changing the returned page on
/// retry does not turn the same send into an operation-id conflict.
pub fn thread_post_request_hash(
    params: &Value,
    entity_id: &str,
    agent_id: &str,
    conversation_id: &str,
) -> String {
    let payload_hash = request_hash(THREAD_POST_METHOD, params);
    let envelope = serde_json::json!({
        "payload_hash": payload_hash,
        "entity_id": entity_id,
        "agent_id": agent_id,
        "conversation_id": conversation_id,
    });
    format!("{:x}", Sha256::digest(envelope.to_string().as_bytes()))
}

fn normalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut normalized = Map::new();
            for key in keys {
                let value = values
                    .get(&key)
                    .expect("the key came from this object")
                    .clone();
                normalized.insert(key, normalize_json(value));
            }
            Value::Object(normalized)
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> OperationPayload {
        OperationPayload {
            start_sequence: 7,
            end_sequence: 7,
            messages: vec![serde_json::from_value(serde_json::json!({
                "id": "message-7",
                "sequence": 7,
                "updated_sequence": 7,
                "role": "user",
                "body": "inspect the screenshot",
                "created_at": "2026-09-15T12:00:00Z",
                "attachments": [{
                    "name": "failure.png",
                    "path": ".build/attachments/failure.png",
                    "mime": "image/png",
                    "size": 123
                }]
            }))
            .unwrap()],
            prior_context: "The previous revision changed the parser.".into(),
            ask_to_name: false,
            tells_sender_context: false,
        }
    }

    /// A message from an agent at 190k of a chat that compacts at 200k, on a
    /// known 1M window, named Rail scroll.
    fn from_rail_scroll(from_build: bool) -> ThreadMessage {
        let mut sender = crate::thread::AgentIdentity::new("agent-9");
        sender.name = Some("Rail scroll".to_string());
        sender.context = Some(crate::thread::ContextReading {
            tokens: 190_000,
            window: Some(1_000_000),
            compact_at: Some(200_000),
            at: "2026-09-21T12:00:00Z".to_string(),
        });
        ThreadMessage {
            from_agent: Some(Box::new(sender)),
            from_build,
            ..payload().messages[0].clone()
        }
    }

    /// The project agent is told how full a sender's context is, in one line;
    /// nobody else is, and a notice Build wrote carries no such line.
    #[test]
    fn the_project_agent_is_told_how_full_a_senders_context_is() {
        let told = OperationPayload {
            messages: vec![from_rail_scroll(false)],
            tells_sender_context: true,
            ..payload()
        };
        let prompt = told.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(
            prompt.contains("\nRail scroll is at 190k of 200k (95%, compacts at 200k).\n"),
            "{prompt}"
        );

        let not_the_project_agent = OperationPayload {
            tells_sender_context: false,
            ..told.clone()
        };
        let prompt = not_the_project_agent.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(!prompt.contains("Rail scroll is at"), "{prompt}");

        let a_notice = OperationPayload {
            messages: vec![from_rail_scroll(true)],
            ..told
        };
        let prompt = a_notice.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(!prompt.contains("Rail scroll is at"), "{prompt}");
    }

    #[test]
    fn operation_delivery_is_native_and_cold_reestablishes_protocol() {
        let payload = payload();
        let warm = payload.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(warm.contains("reviewer operation `post-1`"), "{warm}");
        assert!(warm.contains("inspect the screenshot"), "{warm}");
        assert!(warm.contains(".build/attachments/failure.png"), "{warm}");
        assert!(!warm.contains("read_unread_messages"), "{warm}");
        assert!(!warm.contains("Build conversation protocol"), "{warm}");

        let cold = payload.delivery_prompt("post-1", true, AgentProvider::Claude);
        assert!(
            cold.contains("Conversation context before this operation"),
            "{cold}"
        );
        assert!(cold.contains("Build conversation protocol"), "{cold}");
        assert!(!cold.contains("read_unread_messages"), "{cold}");
    }

    /// A notice gets no envelope paragraph at all (#61).
    ///
    /// The line IS the notice — "New comment tc-… on #53 from the user" — and
    /// a paragraph under it explaining what a notice is would be more words
    /// than the thing it explains. The assignment brief is for the one message
    /// that hands work over.
    #[test]
    fn a_notice_gets_the_line_and_no_envelope_paragraph() {
        let envelope = crate::thread::TaskEnvelope {
            task_id: "task-01K5Z".into(),
            number: 13,
            title: "Kanban drag".into(),
            links: crate::tracker::TaskLinks::default(),
        };
        let notice = crate::thread::TaskNotice {
            actor: crate::thread::NoticeActor {
                who: crate::tracker::Actor::User,
                name: None,
                identity: None,
            },
            action: "moved".into(),
            comment_id: None,
            from: Some("in_progress".into()),
            to: Some("in_review".into()),
            assignee: None,
            assignee_identity: None,
        };
        let watching = OperationPayload {
            messages: vec![ThreadMessage {
                from_build: true,
                body: "#13 moved to In review by the user.".into(),
                from_task: Some(Box::new(envelope.clone())),
                task_notice: Some(Box::new(notice)),
                ..payload().messages[0].clone()
            }],
            ..payload()
        };

        let warm = watching.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(
            warm.contains("#13 moved to In review by the user."),
            "{warm}"
        );
        for absent in [
            "which you are tracking",
            "nobody is waiting",
            "read it with get_task before you start",
            "move it to In review with move_task",
        ] {
            assert!(!warm.contains(absent), "{absent:?} is still said: {warm}");
        }

        // The assignee's own hand-off still says all of it: that message hands
        // work over, and this one does not.
        let handed = OperationPayload {
            messages: vec![ThreadMessage {
                from_task: Some(Box::new(envelope)),
                ..payload().messages[0].clone()
            }],
            ..payload()
        };
        let brief = handed.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(
            brief.contains("read it with get_task before you start"),
            "{brief}"
        );
        assert!(
            brief.contains("move it to In review with move_task"),
            "{brief}"
        );
    }

    /// A task the user is LOOKING at is not one they handed over, and the
    /// envelope says which it is: the agent is being asked about the task on
    /// screen, and has to go and read it before it can answer.
    #[test]
    fn the_task_the_user_is_looking_at_is_named_and_not_mistaken_for_a_hand_off() {
        let looking = OperationPayload {
            messages: vec![ThreadMessage {
                viewing_context: Some(Box::new(
                    serde_json::from_value(serde_json::json!({
                        "version": 1,
                        "items": [{
                            "kind": "task",
                            "task_id": "task-01K5Z",
                            "number": 9,
                            "title": "Kanban drag does not persist"
                        }]
                    }))
                    .unwrap(),
                )),
                ..payload().messages[0].clone()
            }],
            ..payload()
        };

        let warm = looking.delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(
            warm.contains(
                "The user is looking at task #9 \"Kanban drag does not persist\" \
                 (`task-01K5Z`)"
            ),
            "{warm}"
        );
        assert!(
            warm.contains("read it with get_task"),
            "the agent is told how to read it: {warm}"
        );
        assert!(
            !warm.contains("hands you task"),
            "looking at a task is not being handed one: {warm}"
        );

        // And a message sent from nowhere near the board says nothing about
        // one, which is what keeps the sentence worth reading.
        let ordinary = payload().delivery_prompt("post-1", false, AgentProvider::Claude);
        assert!(!ordinary.contains("is looking at task"), "{ordinary}");
    }

    /// Words another agent sent are named as such in the envelope, not only in
    /// the payload's JSON: a harness that answers them as if the user had
    /// asked is answering the wrong person.
    #[test]
    fn words_another_agent_sent_say_whose_they_are() {
        let handed_over = OperationPayload {
            messages: vec![ThreadMessage {
                from_agent: Some(Box::new(crate::thread::AgentIdentity::new("router-7"))),
                ..payload().messages[0].clone()
            }],
            ..payload()
        };

        let prompt = handed_over.legacy_delivery_prompt(false, AgentProvider::Claude);
        assert!(
            prompt.contains(
                "These messages came from agent `router-7`, not from the user. \
                 Reply with message_agent to `router-7`."
            ),
            "{prompt}"
        );
        assert!(
            !payload()
                .legacy_delivery_prompt(false, AgentProvider::Claude)
                .contains("came from agent"),
            "the user's own words claim no sender"
        );
    }

    /// The workspace the user was standing in is named in the envelope, not
    /// only in the payload's JSON: the project agent is reachable from every
    /// workspace, and "this workspace" has to mean the one they were in.
    #[test]
    fn a_message_sent_from_a_workspace_names_it() {
        let from_workspace = OperationPayload {
            messages: vec![ThreadMessage {
                viewing_context: Some(Box::new(crate::thread::ViewingContext {
                    version: 1,
                    items: vec![crate::thread::ViewingContextItem::Workspace {
                        workspace_id: "ws-3f2a91c4".into(),
                        name: "wire-facade".into(),
                    }],
                })),
                ..payload().messages[0].clone()
            }],
            ..payload()
        };

        let prompt = from_workspace.legacy_delivery_prompt(false, AgentProvider::Claude);
        assert!(
            prompt.contains("The user sent this from workspace \"wire-facade\" (ws-3f2a91c4)."),
            "{prompt}"
        );
        assert!(
            !payload()
                .legacy_delivery_prompt(false, AgentProvider::Claude)
                .contains("from workspace"),
            "a message sent from nowhere in particular claims no workspace"
        );
    }

    #[test]
    fn legacy_delivery_has_exact_scope_without_inventing_an_operation() {
        let warm = payload().legacy_delivery_prompt(false, AgentProvider::Claude);
        assert!(
            warm.contains("conversation sequences 7 through 7"),
            "{warm}"
        );
        assert!(warm.contains(NATIVE_REVIEWER_MESSAGES_HEADING), "{warm}");
        assert!(!warm.contains("operation `"), "{warm}");
        assert!(!warm.contains("read_unread_messages"), "{warm}");
    }

    #[test]
    fn reviewer_text_precedes_the_injected_delivery_envelope() {
        let prompt = payload().delivery_prompt("post-1", true, AgentProvider::Codex);
        assert!(
            prompt.starts_with("inspect the screenshot\n\nProcess only reviewer operation"),
            "{prompt}"
        );
        let messages = prompt.find(NATIVE_REVIEWER_MESSAGES_HEADING).unwrap();
        let protocol = prompt.find("Build conversation protocol:").unwrap();
        assert!(
            messages < protocol,
            "the protocol is injected after the user prompt: {prompt}"
        );
    }

    #[test]
    fn provider_commands_that_require_a_bare_turn_skip_injection() {
        let command = |body: &str| OperationPayload {
            messages: vec![ThreadMessage {
                body: body.into(),
                attachments: Vec::new(),
                viewing_context: None,
                ..payload().messages[0].clone()
            }],
            ..payload()
        };

        for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
            assert_eq!(
                command("/clear").delivery_prompt("op", true, provider),
                "/clear"
            );
            assert_eq!(
                command("/compact focus on API changes").delivery_prompt("op", true, provider),
                "/compact focus on API changes"
            );
        }
        for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
            assert_eq!(
                command("/clear").delivery_prompt("op", true, provider),
                "/clear"
            );
        }
        assert!(command("/compact")
            .delivery_prompt("op", true, AgentProvider::Codex)
            .contains("Build conversation protocol:"));
        assert_eq!(
            command("/compact").delivery_prompt("op", true, AgentProvider::CodexAppServer),
            "/compact"
        );
        assert!(command("/goal ship it")
            .delivery_prompt("op", true, AgentProvider::Codex)
            .starts_with("/goal ship it\n\nProcess only reviewer operation"));

        let mut contextual = command("/clear");
        contextual.messages[0].viewing_context = Some(Box::new(
            serde_json::from_value(serde_json::json!({ "version": 1, "items": [] })).unwrap(),
        ));
        assert_eq!(
            contextual.delivery_prompt("op", true, AgentProvider::Claude),
            "/clear"
        );
    }

    #[test]
    fn request_hash_ignores_object_order_and_operation_id() {
        let left = serde_json::json!({
            "operation_id": "first",
            "entity_id": "task-1",
            "body": "hello",
            "anchor": { "line": 3, "path": "src/lib.rs" }
        });
        let right = serde_json::json!({
            "anchor": { "path": "src/lib.rs", "line": 3 },
            "body": "hello",
            "entity_id": "task-1",
            "operation_id": "second"
        });
        assert_eq!(
            request_hash(THREAD_POST_METHOD, &left),
            request_hash(THREAD_POST_METHOD, &right)
        );
    }

    #[test]
    fn thread_post_hash_ignores_read_projection_but_binds_resolved_address() {
        let first = serde_json::json!({
            "entity_id": "task-1",
            "body": "hello",
            "thread_limit": 20
        });
        let retry = serde_json::json!({
            "entity_id": "task-1",
            "body": "hello",
            "thread_after_sequence": 9
        });
        let hash = thread_post_request_hash(&first, "task-1", "agent-1", "conversation-1");
        assert_eq!(
            hash,
            thread_post_request_hash(&retry, "task-1", "agent-1", "conversation-1",)
        );
        assert_ne!(
            hash,
            thread_post_request_hash(&retry, "task-1", "agent-2", "conversation-1",)
        );
    }

    #[test]
    fn thread_post_hash_binds_viewing_context_and_keeps_legacy_hash_stable() {
        let legacy = serde_json::json!({ "entity_id": "task-1", "body": "hello" });
        let expected = thread_post_request_hash(&legacy, "task-1", "agent-1", "conversation-1");
        assert_eq!(
            expected,
            thread_post_request_hash(&legacy, "task-1", "agent-1", "conversation-1")
        );

        let contextual = serde_json::json!({
            "entity_id": "task-1",
            "body": "hello",
            "viewing_context": { "version": 1, "items": [{ "kind": "file", "path": "src/lib.rs" }] }
        });
        assert_ne!(
            expected,
            thread_post_request_hash(&contextual, "task-1", "agent-1", "conversation-1")
        );
    }

    #[test]
    fn shared_browser_contract_matches_receipt_vocabulary() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../fixtures/api/v1/thread.post.json")).unwrap();
        let contract = &fixture["operations"];
        assert_eq!(contract["post_method"], THREAD_POST_METHOD);
        assert_eq!(contract["status_method"], "thread.operation");
        assert_eq!(contract["operation_id"]["max_bytes"], 128);
        assert_eq!(
            contract["statuses"],
            serde_json::json!(["queued", "claimed", "delivered", "uncertain"])
        );
    }
}
