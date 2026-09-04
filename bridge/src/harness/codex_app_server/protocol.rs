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

    pub fn is_active_turn_not_steerable(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|data| data.pointer("/codexErrorInfo/activeTurnNotSteerable"))
            .is_some_and(Value::is_object)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionEvent {
    Response {
        operation: PendingOperation,
        result: Result<OperationResult, RpcError>,
    },
    Notification(InboundNotification),
    Request(InboundServerRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InboundNotification {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InboundServerRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ItemNotification {
    pub lifecycle: ItemLifecycle,
    pub thread_id: String,
    pub turn_id: String,
    pub item: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub error: NotificationError,
    pub will_retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NotificationError {
    pub message: String,
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
    Error(ErrorNotification),
    Delta,
    Unknown,
}

impl ServerNotification {
    pub fn decode(method: &str, params: Value) -> Result<ServerNotification, String> {
        match tag_for(NOTIFICATION_METHODS, method).unwrap_or(NotificationMethod::Unknown) {
            NotificationMethod::ThreadStarted => Ok(ServerNotification::ThreadStarted {
                thread_id: required_string(&params, "/thread/id", "thread/started thread id")?,
                parent_thread_id: params
                    .pointer("/thread/parentThreadId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            NotificationMethod::TurnStarted => Ok(ServerNotification::TurnStarted {
                thread_id: required_string(&params, "/threadId", "turn/started thread id")?,
                turn_id: required_string(&params, "/turn/id", "turn/started turn id")?,
            }),
            NotificationMethod::TurnCompleted => Ok(ServerNotification::TurnCompleted {
                thread_id: required_string(&params, "/threadId", "turn/completed thread id")?,
                completion: TurnCompletion::from_params(&params)?,
            }),
            NotificationMethod::ItemStarted => {
                item_notification(ItemLifecycle::Started, method, params)
            }
            NotificationMethod::ItemCompleted => {
                item_notification(ItemLifecycle::Completed, method, params)
            }
            NotificationMethod::Error => decode(&params).map(ServerNotification::Error),
            NotificationMethod::Delta => Ok(ServerNotification::Delta),
            NotificationMethod::Unknown => Ok(ServerNotification::Unknown),
        }
    }
}

fn item_notification(
    lifecycle: ItemLifecycle,
    method: &str,
    params: Value,
) -> Result<ServerNotification, String> {
    let item = params
        .get("item")
        .filter(|item| item.is_object())
        .cloned()
        .ok_or_else(|| format!("{method} item is missing"))?;
    Ok(ServerNotification::Item(ItemNotification {
        lifecycle,
        thread_id: required_string(&params, "/threadId", "item thread id")?,
        turn_id: required_string(&params, "/turnId", "item turn id")?,
        item,
    }))
}

#[derive(Debug, Clone, Copy)]
enum NotificationMethod {
    ThreadStarted,
    TurnStarted,
    TurnCompleted,
    ItemStarted,
    ItemCompleted,
    Error,
    Delta,
    Unknown,
}

const NOTIFICATION_METHODS: &[(&str, NotificationMethod)] = &[
    ("thread/started", NotificationMethod::ThreadStarted),
    ("turn/started", NotificationMethod::TurnStarted),
    ("turn/completed", NotificationMethod::TurnCompleted),
    ("item/started", NotificationMethod::ItemStarted),
    ("item/completed", NotificationMethod::ItemCompleted),
    ("error", NotificationMethod::Error),
    ("item/agentMessage/delta", NotificationMethod::Delta),
    (
        "item/commandExecution/outputDelta",
        NotificationMethod::Delta,
    ),
    ("item/fileChange/outputDelta", NotificationMethod::Delta),
    ("item/reasoning/summaryTextDelta", NotificationMethod::Delta),
    ("item/reasoning/textDelta", NotificationMethod::Delta),
];

#[derive(Debug, Clone, Copy)]
struct ServerRequestMethod {
    routing_pointer: Option<&'static str>,
    build: fn(Value) -> ServerRequest,
}

const fn thread_scoped(build: fn(Value) -> ServerRequest) -> ServerRequestMethod {
    ServerRequestMethod {
        routing_pointer: Some("/threadId"),
        build,
    }
}

const fn conversation_scoped(build: fn(Value) -> ServerRequest) -> ServerRequestMethod {
    ServerRequestMethod {
        routing_pointer: Some("/conversationId"),
        build,
    }
}

const fn unscoped(build: fn(Value) -> ServerRequest) -> ServerRequestMethod {
    ServerRequestMethod {
        routing_pointer: None,
        build,
    }
}

const SERVER_REQUEST_METHODS: &[(&str, ServerRequestMethod)] = &[
    (
        "item/commandExecution/requestApproval",
        thread_scoped(|id| ServerRequest::CommandApproval { id }),
    ),
    (
        "item/fileChange/requestApproval",
        thread_scoped(|id| ServerRequest::FileApproval { id }),
    ),
    (
        "execCommandApproval",
        conversation_scoped(|id| ServerRequest::LegacyCommandApproval { id }),
    ),
    (
        "applyPatchApproval",
        conversation_scoped(|id| ServerRequest::LegacyPatchApproval { id }),
    ),
    (
        "mcpServer/elicitation/request",
        thread_scoped(|id| ServerRequest::Elicitation { id }),
    ),
    (
        "item/tool/requestUserInput",
        thread_scoped(|id| ServerRequest::UserInput { id }),
    ),
    (
        "item/permissions/requestApproval",
        thread_scoped(|id| ServerRequest::Permissions { id }),
    ),
    (
        "item/tool/call",
        thread_scoped(|id| ServerRequest::DynamicTool { id }),
    ),
    (
        "account/chatgptAuthTokens/refresh",
        unscoped(|id| ServerRequest::RefreshAuth { id }),
    ),
    (
        "attestation/generate",
        unscoped(|id| ServerRequest::Attestation { id }),
    ),
    (
        "currentTime/read",
        thread_scoped(|id| ServerRequest::CurrentTime { id }),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentThreadRoute {
    Parent,
    Child,
    Unscoped,
}

pub struct ParentThreadFilter;

impl ParentThreadFilter {
    pub fn notification(
        method: &str,
        params: &Value,
        expected_parent: Option<&str>,
    ) -> ParentThreadRoute {
        if method == "thread/started" {
            if params
                .pointer("/thread/parentThreadId")
                .is_some_and(|parent| !parent.is_null())
            {
                return ParentThreadRoute::Child;
            }
            return route_id(
                params.pointer("/thread/id").and_then(Value::as_str),
                expected_parent,
            );
        }
        route_id(
            params.pointer("/threadId").and_then(Value::as_str),
            expected_parent,
        )
    }

    fn server_request(
        routing_pointer: Option<&str>,
        params: &Value,
        expected_parent: Option<&str>,
    ) -> ParentThreadRoute {
        let Some(pointer) = routing_pointer else {
            return ParentThreadRoute::Unscoped;
        };
        route_id(
            params.pointer(pointer).and_then(Value::as_str),
            expected_parent,
        )
    }
}

fn route_id(candidate: Option<&str>, expected_parent: Option<&str>) -> ParentThreadRoute {
    match (candidate, expected_parent) {
        (Some(candidate), Some(expected)) if candidate != expected => ParentThreadRoute::Child,
        (Some(_), _) => ParentThreadRoute::Parent,
        (None, _) => ParentThreadRoute::Unscoped,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerRequest {
    CommandApproval { id: Value },
    FileApproval { id: Value },
    LegacyCommandApproval { id: Value },
    LegacyPatchApproval { id: Value },
    Elicitation { id: Value },
    UserInput { id: Value },
    Permissions { id: Value },
    DynamicTool { id: Value },
    RefreshAuth { id: Value },
    Attestation { id: Value },
    CurrentTime { id: Value },
    Unknown { id: Value, method: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedServerRequest {
    pub request: ServerRequest,
    pub route: ParentThreadRoute,
}

impl RoutedServerRequest {
    pub fn decode(
        inbound: &InboundServerRequest,
        expected_parent: Option<&str>,
    ) -> Result<RoutedServerRequest, String> {
        let Some(method) = tag_for(SERVER_REQUEST_METHODS, &inbound.method) else {
            return Ok(RoutedServerRequest {
                request: ServerRequest::Unknown {
                    id: inbound.id.clone(),
                    method: inbound.method.clone(),
                },
                route: ParentThreadRoute::Unscoped,
            });
        };
        let route = ParentThreadFilter::server_request(
            method.routing_pointer,
            &inbound.params,
            expected_parent,
        );
        if route != ParentThreadRoute::Child {
            if let Some(pointer) = method.routing_pointer {
                required_string(
                    &inbound.params,
                    pointer,
                    &format!("{} routing id", inbound.method),
                )?;
            }
        }
        Ok(RoutedServerRequest {
            request: (method.build)(inbound.id.clone()),
            route,
        })
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
    result: Option<ServerResult>,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
enum ServerResult {
    Approval(ApprovalResult),
    LegacyApproval(LegacyApprovalResult),
    Elicitation(ElicitationResult),
    CurrentTime(CurrentTimeResult),
    #[cfg(test)]
    Test(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ApprovalResult {
    decision: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct LegacyApprovalResult {
    decision: DeniedDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DeniedDecision {
    denied: DeniedReason,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DeniedReason {
    rejection: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ElicitationResult {
    action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentTimeResult {
    current_time_at: i64,
}

impl ServerResponse {
    pub fn approval_declined(id: Value) -> ServerResponse {
        ServerResponse::success(
            id,
            ServerResult::Approval(ApprovalResult {
                decision: "decline",
            }),
        )
    }

    pub fn legacy_command_declined(id: Value) -> ServerResponse {
        ServerResponse::legacy_declined(id, "Build does not approve commands")
    }

    pub fn legacy_patch_declined(id: Value) -> ServerResponse {
        ServerResponse::legacy_declined(id, "Build does not approve file changes")
    }

    fn legacy_declined(id: Value, rejection: &'static str) -> ServerResponse {
        ServerResponse::success(
            id,
            ServerResult::LegacyApproval(LegacyApprovalResult {
                decision: DeniedDecision {
                    denied: DeniedReason { rejection },
                },
            }),
        )
    }

    pub fn elicitation_declined(id: Value) -> ServerResponse {
        ServerResponse::success(
            id,
            ServerResult::Elicitation(ElicitationResult { action: "decline" }),
        )
    }

    pub fn current_time(id: Value, current_unix_seconds: i64) -> ServerResponse {
        ServerResponse::success(
            id,
            ServerResult::CurrentTime(CurrentTimeResult {
                current_time_at: current_unix_seconds,
            }),
        )
    }

    fn success(id: Value, result: ServerResult) -> ServerResponse {
        ServerResponse {
            id,
            result: Some(result),
            error: None,
        }
    }

    #[cfg(test)]
    pub fn result(id: Value, result: Value) -> ServerResponse {
        ServerResponse::success(id, ServerResult::Test(result))
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

pub fn tag_for<Tag: Copy>(table: &[(&str, Tag)], name: &str) -> Option<Tag> {
    table
        .iter()
        .find_map(|(known, tag)| (*known == name).then_some(*tag))
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
