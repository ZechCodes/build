use serde_json::{json, Value};

pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOperation {
    Initialize,
    StartThread,
    ResumeThread,
    StartTurn { input: String },
    SteerTurn { turn_id: String, input: String },
    InterruptTurn { turn_id: String },
}

impl PendingOperation {
    pub fn method(&self) -> &'static str {
        match self {
            PendingOperation::Initialize => "initialize",
            PendingOperation::StartThread => "thread/start",
            PendingOperation::ResumeThread => "thread/resume",
            PendingOperation::StartTurn { .. } => "turn/start",
            PendingOperation::SteerTurn { .. } => "turn/steer",
            PendingOperation::InterruptTurn { .. } => "turn/interrupt",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
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
            .is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionEvent {
    Response {
        operation: PendingOperation,
        result: Result<Value, RpcError>,
    },
    Notification(ServerNotification),
    Request(ServerRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerNotification {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

impl ServerRequest {
    pub fn new(id: Value, method: impl Into<String>, params: Value) -> ServerRequest {
        ServerRequest {
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerResponse {
    id: Value,
    result: Option<Value>,
    error: Option<RpcError>,
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
            error: Some(RpcError::new(code, message)),
        }
    }

    #[cfg(test)]
    pub fn error_code(&self) -> Option<i64> {
        self.error.as_ref().map(|error| error.code)
    }

    pub fn to_value(&self) -> Value {
        match (&self.result, &self.error) {
            (Some(result), None) => json!({"id":self.id,"result":result}),
            (None, Some(error)) => {
                let mut encoded = json!({"code":error.code,"message":error.message});
                if let Some(data) = &error.data {
                    encoded["data"] = data.clone();
                }
                json!({"id":self.id,"error":encoded})
            }
            _ => unreachable!("server responses have exactly one body"),
        }
    }
}
