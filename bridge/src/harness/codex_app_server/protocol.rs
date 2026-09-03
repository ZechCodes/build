use std::io::Write;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOperation {
    Initialize,
    StartThread {
        cwd: String,
        model: Option<String>,
    },
    ResumeThread {
        thread_id: String,
        cwd: String,
        model: Option<String>,
    },
    StartTurn {
        thread_id: String,
        input: String,
        model: Option<String>,
        effort: Option<String>,
    },
    SteerTurn {
        thread_id: String,
        turn_id: String,
        input: String,
    },
    InterruptTurn {
        thread_id: String,
        turn_id: String,
    },
}

impl PendingOperation {
    pub fn method(&self) -> &'static str {
        match self {
            PendingOperation::Initialize => "initialize",
            PendingOperation::StartThread { .. } => "thread/start",
            PendingOperation::ResumeThread { .. } => "thread/resume",
            PendingOperation::StartTurn { .. } => "turn/start",
            PendingOperation::SteerTurn { .. } => "turn/steer",
            PendingOperation::InterruptTurn { .. } => "turn/interrupt",
        }
    }

    pub fn serialize_request(
        &self,
        id: RequestId,
        writer: &mut dyn Write,
    ) -> Result<(), serde_json::Error> {
        match self {
            PendingOperation::Initialize => serialize_request(
                writer,
                id,
                self.method(),
                &InitializeParams {
                    client_info: ClientInfo {
                        name: "build_bridge",
                        title: "Build",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    capabilities: EmptyObject {},
                },
            ),
            PendingOperation::StartThread { cwd, model } => serialize_request(
                writer,
                id,
                self.method(),
                &ThreadOpenParams {
                    thread_id: None,
                    cwd,
                    model: model.as_deref(),
                    approval_policy: "never",
                    sandbox: "danger-full-access",
                },
            ),
            PendingOperation::ResumeThread {
                thread_id,
                cwd,
                model,
            } => serialize_request(
                writer,
                id,
                self.method(),
                &ThreadOpenParams {
                    thread_id: Some(thread_id),
                    cwd,
                    model: model.as_deref(),
                    approval_policy: "never",
                    sandbox: "danger-full-access",
                },
            ),
            PendingOperation::StartTurn {
                thread_id,
                input,
                model,
                effort,
            } => serialize_request(
                writer,
                id,
                self.method(),
                &TurnStartParams {
                    thread_id,
                    input: [TextInput {
                        kind: "text",
                        text: input,
                    }],
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                },
            ),
            PendingOperation::SteerTurn {
                thread_id,
                turn_id,
                input,
            } => serialize_request(
                writer,
                id,
                self.method(),
                &TurnSteerParams {
                    thread_id,
                    expected_turn_id: turn_id,
                    input: [TextInput {
                        kind: "text",
                        text: input,
                    }],
                },
            ),
            PendingOperation::InterruptTurn { thread_id, turn_id } => serialize_request(
                writer,
                id,
                self.method(),
                &TurnInterruptParams { thread_id, turn_id },
            ),
        }
    }

    pub fn decode_result(&self, value: &Value) -> Result<OperationResult, String> {
        match self {
            PendingOperation::Initialize => decode(value).map(OperationResult::Initialize),
            PendingOperation::StartThread { .. } | PendingOperation::ResumeThread { .. } => {
                decode(value).map(OperationResult::ThreadOpened)
            }
            PendingOperation::StartTurn { .. } => decode(value).map(OperationResult::TurnStarted),
            PendingOperation::SteerTurn { .. } => decode(value).map(OperationResult::TurnSteered),
            PendingOperation::InterruptTurn { .. } => {
                if value.is_object() {
                    Ok(OperationResult::TurnInterrupted)
                } else {
                    Err("turn/interrupt response has the wrong body".to_string())
                }
            }
        }
        .map_err(|error| format!("{} response has the wrong body: {error}", self.method()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationResult {
    Initialize(InitializeResult),
    ThreadOpened(ThreadOpenResult),
    TurnStarted(TurnStartResult),
    TurnSteered(TurnSteerResult),
    TurnInterrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOpenResult {
    pub thread: ThreadIdentity,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub cwd: String,
    pub approval_policy: String,
    pub sandbox: SandboxResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ThreadIdentity {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxResult {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TurnStartResult {
    pub turn: TurnIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TurnIdentity {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResult {
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    #[cfg(test)]
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[cfg(test)]
    pub fn with_data(code: i64, message: impl Into<String>, data: Value) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn from_value(value: &Value) -> Result<RpcError, String> {
        let code = value["code"]
            .as_i64()
            .ok_or_else(|| "JSON-RPC error is missing integer code".to_string())?;
        let message = value["message"]
            .as_str()
            .ok_or_else(|| "JSON-RPC error is missing message".to_string())?;
        Ok(RpcError {
            code,
            message: message.to_string(),
            data: value.get("data").cloned(),
        })
    }

    pub fn is_no_active_turn(&self) -> bool {
        self.code == -32600
            && matches!(
                self.message.to_ascii_lowercase().as_str(),
                "no active turn" | "no active turn to steer" | "no active turn to interrupt"
            )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionEvent {
    Response {
        operation: PendingOperation,
        result: Result<OperationResult, RpcError>,
    },
    Notification(ServerNotification),
    Request(ServerRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCompletion {
    pub turn_id: String,
    pub status: String,
    pub error: Option<String>,
}

impl TurnCompletion {
    #[cfg(test)]
    pub fn new(turn_id: String, error: Option<String>) -> TurnCompletion {
        let status = if error.is_some() {
            "failed".to_string()
        } else {
            "completed".to_string()
        };
        TurnCompletion::observed(turn_id, status, error)
    }

    pub fn observed(turn_id: String, status: String, error: Option<String>) -> TurnCompletion {
        TurnCompletion {
            turn_id,
            status: normalize_text(&status).to_ascii_lowercase(),
            error: error
                .map(|message| normalize_text(&message))
                .filter(|message| !message.is_empty()),
        }
    }

    fn from_params(params: &Value) -> Result<TurnCompletion, String> {
        let turn_id = required_string(params, "/turn/id", "turn/completed turn id")?;
        let status = required_string(params, "/turn/status", "turn/completed status")?;
        let error = params
            .pointer("/turn/error/message")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(TurnCompletion::observed(turn_id, status, error))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemLifecycle {
    Started,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemType {
    Reasoning,
    AgentMessage,
    CommandExecution,
    FileChange,
    BuildMcpToolCall,
    McpToolCall,
    WebSearch,
    ImageView,
    Sleep,
    ImageGeneration,
    CollabAgentToolCall,
    SubAgentActivity,
    ContextCompaction,
    DynamicToolCall,
    Unknown,
}

impl ItemType {
    fn decode(value: &Value) -> ItemType {
        match value["type"].as_str() {
            Some("reasoning") => ItemType::Reasoning,
            Some("agentMessage") => ItemType::AgentMessage,
            Some("commandExecution") => ItemType::CommandExecution,
            Some("fileChange") => ItemType::FileChange,
            Some("mcpToolCall") if value["server"].as_str() == Some("build") => {
                ItemType::BuildMcpToolCall
            }
            Some("mcpToolCall") => ItemType::McpToolCall,
            Some("webSearch") => ItemType::WebSearch,
            Some("imageView") => ItemType::ImageView,
            Some("sleep") => ItemType::Sleep,
            Some("imageGeneration") => ItemType::ImageGeneration,
            Some("collabAgentToolCall") => ItemType::CollabAgentToolCall,
            Some("subAgentActivity") => ItemType::SubAgentActivity,
            Some("contextCompaction") => ItemType::ContextCompaction,
            Some("dynamicToolCall") => ItemType::DynamicToolCall,
            _ => ItemType::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemNotification {
    pub lifecycle: ItemLifecycle,
    pub thread_id: String,
    pub turn_id: String,
    pub item_type: ItemType,
    pub item: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerNotification {
    ThreadStarted {
        thread_id: String,
        parent_thread_id: Option<String>,
    },
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TurnCompleted {
        thread_id: String,
        completion: TurnCompletion,
    },
    Item(ItemNotification),
    Error(Value),
    Delta,
    Unknown,
}

impl ServerNotification {
    pub fn decode(method: &str, params: Value) -> Result<ServerNotification, String> {
        match method {
            "thread/started" => Ok(ServerNotification::ThreadStarted {
                thread_id: required_string(&params, "/thread/id", "thread/started thread id")?,
                parent_thread_id: params
                    .pointer("/thread/parentThreadId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            "turn/started" => Ok(ServerNotification::TurnStarted {
                thread_id: required_string(&params, "/threadId", "turn/started thread id")?,
                turn_id: required_string(&params, "/turn/id", "turn/started turn id")?,
            }),
            "turn/completed" => Ok(ServerNotification::TurnCompleted {
                thread_id: required_string(&params, "/threadId", "turn/completed thread id")?,
                completion: TurnCompletion::from_params(&params)?,
            }),
            "item/started" | "item/completed" => {
                let item = params
                    .get("item")
                    .filter(|item| item.is_object())
                    .cloned()
                    .ok_or_else(|| format!("{method} item is missing"))?;
                Ok(ServerNotification::Item(ItemNotification {
                    lifecycle: if method == "item/started" {
                        ItemLifecycle::Started
                    } else {
                        ItemLifecycle::Completed
                    },
                    thread_id: required_string(&params, "/threadId", "item thread id")?,
                    turn_id: required_string(&params, "/turnId", "item turn id")?,
                    item_type: ItemType::decode(&item),
                    item,
                }))
            }
            "error" => Ok(ServerNotification::Error(params)),
            method if method.contains("delta") => Ok(ServerNotification::Delta),
            _ => Ok(ServerNotification::Unknown),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRequestKind {
    CommandApproval,
    FileApproval,
    LegacyCommandApproval,
    LegacyPatchApproval,
    Elicitation,
    UserInput,
    Permissions,
    DynamicTool,
    RefreshAuth,
    Attestation,
    CurrentTime,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerRequest {
    pub id: Value,
    pub kind: ServerRequestKind,
    pub method: String,
    pub params: Value,
}

impl ServerRequest {
    pub fn new(id: Value, method: impl Into<String>, params: Value) -> ServerRequest {
        let method = method.into();
        let kind = match method.as_str() {
            "item/commandExecution/requestApproval" => ServerRequestKind::CommandApproval,
            "item/fileChange/requestApproval" => ServerRequestKind::FileApproval,
            "execCommandApproval" => ServerRequestKind::LegacyCommandApproval,
            "applyPatchApproval" => ServerRequestKind::LegacyPatchApproval,
            "mcpServer/elicitation/request" => ServerRequestKind::Elicitation,
            "item/tool/requestUserInput" => ServerRequestKind::UserInput,
            "item/permissions/requestApproval" => ServerRequestKind::Permissions,
            "item/tool/call" => ServerRequestKind::DynamicTool,
            "account/chatgptAuthTokens/refresh" => ServerRequestKind::RefreshAuth,
            "attestation/generate" => ServerRequestKind::Attestation,
            "currentTime/read" => ServerRequestKind::CurrentTime,
            _ => ServerRequestKind::Unknown,
        };
        ServerRequest {
            id,
            kind,
            method,
            params,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "method", content = "params")]
pub enum ClientNotification {
    #[serde(rename = "initialized")]
    Initialized,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServerResponse {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl ServerResponse {
    pub fn result(id: Value, result: Value) -> ServerResponse {
        ServerResponse {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> ServerResponse {
        ServerResponse {
            id,
            result: None,
            error: Some(RpcErrorBody {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    #[cfg(test)]
    pub fn error_code(&self) -> Option<i64> {
        self.error.as_ref().map(|error| error.code)
    }

    #[cfg(test)]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a typed server response serializes")
    }
}

#[derive(Serialize)]
struct RpcRequest<'a, Params> {
    id: RequestId,
    method: &'a str,
    params: Params,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams<'a> {
    client_info: ClientInfo<'a>,
    capabilities: EmptyObject,
}

#[derive(Serialize)]
struct ClientInfo<'a> {
    name: &'a str,
    title: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct EmptyObject {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadOpenParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<&'a str>,
    cwd: &'a str,
    model: Option<&'a str>,
    approval_policy: &'a str,
    sandbox: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartParams<'a> {
    thread_id: &'a str,
    input: [TextInput<'a>; 1],
    model: Option<&'a str>,
    effort: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnSteerParams<'a> {
    thread_id: &'a str,
    expected_turn_id: &'a str,
    input: [TextInput<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnInterruptParams<'a> {
    thread_id: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct TextInput<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    text: &'a str,
}

fn serialize_request<Params: Serialize>(
    writer: &mut dyn Write,
    id: RequestId,
    method: &str,
    params: Params,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer(writer, &RpcRequest { id, method, params })
}

fn decode<ResultType: for<'de> Deserialize<'de>>(value: &Value) -> Result<ResultType, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn required_string(value: &Value, pointer: &str, label: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{label} is missing"))
}

fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
