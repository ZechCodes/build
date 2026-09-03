use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::limits::AppServerLimits;
use super::protocol::{
    ConnectionEvent, PendingOperation, RequestId, RpcError, ServerNotification, ServerRequest,
    ServerResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("app-server connection is closed")]
    Closed,
    #[error("app-server request id exhausted")]
    RequestIdExhausted,
    #[error("app-server pending request limit exceeded ({0})")]
    PendingLimit(usize),
    #[error("app-server frame exceeds {0} bytes")]
    FrameTooLarge(usize),
    #[error("app-server sent an unterminated frame")]
    UnterminatedFrame,
    #[error("app-server sent invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("app-server sent invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("app-server protocol violation: {0}")]
    Protocol(String),
    #[error("app-server I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub root: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub resume_id: Option<String>,
    pub thread_id: Option<String>,
}

impl Default for RequestContext {
    fn default() -> Self {
        RequestContext {
            root: PathBuf::from("/tmp"),
            model: None,
            effort: None,
            resume_id: None,
            thread_id: None,
        }
    }
}

pub struct AppServerConnection {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<BTreeMap<RequestId, PendingOperation>>,
    next_id: Mutex<RequestId>,
    context: Mutex<RequestContext>,
    limits: AppServerLimits,
}

impl AppServerConnection {
    pub fn new(
        writer: Box<dyn Write + Send>,
        context: RequestContext,
        limits: AppServerLimits,
    ) -> AppServerConnection {
        AppServerConnection {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            context: Mutex::new(context),
            limits,
        }
    }

    #[cfg(test)]
    pub fn memory(limits: AppServerLimits) -> AppServerConnection {
        AppServerConnection::new(
            Box::new(Vec::<u8>::new()),
            RequestContext::default(),
            limits,
        )
    }

    #[cfg(test)]
    pub fn failing_writer(limits: AppServerLimits) -> AppServerConnection {
        struct Fails;
        impl Write for Fails {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        AppServerConnection::new(Box::new(Fails), RequestContext::default(), limits)
    }

    pub fn request(&self, operation: PendingOperation) -> Result<RequestId, ConnectionError> {
        let params = operation_params(&operation, &self.context.lock().unwrap())?;
        let request_id = self.reserve(operation.clone())?;
        let frame = json!({"id":request_id,"method":operation.method(),"params":params});
        if let Err(error) = self.write(frame) {
            self.pending.lock().unwrap().remove(&request_id);
            return Err(error);
        }
        Ok(request_id)
    }

    fn reserve(&self, operation: PendingOperation) -> Result<RequestId, ConnectionError> {
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= self.limits.pending_requests {
            return Err(ConnectionError::PendingLimit(self.limits.pending_requests));
        }
        let mut next_id = self.next_id.lock().unwrap();
        let request_id = *next_id;
        *next_id = next_id
            .checked_add(1)
            .ok_or(ConnectionError::RequestIdExhausted)?;
        pending.insert(request_id, operation);
        Ok(request_id)
    }

    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ConnectionError> {
        let frame = match params {
            Some(params) => json!({"method":method,"params":params}),
            None => json!({"method":method}),
        };
        self.write(frame)
    }

    pub fn respond(&self, response: ServerResponse) -> Result<(), ConnectionError> {
        self.write(response.to_value())
    }

    fn write(&self, value: Value) -> Result<(), ConnectionError> {
        let mut encoded = serde_json::to_vec(&value)?;
        if encoded.len() > self.limits.outbound_frame_bytes {
            return Err(ConnectionError::FrameTooLarge(
                self.limits.outbound_frame_bytes,
            ));
        }
        encoded.push(b'\n');
        let mut writer = self.writer.lock().unwrap();
        let writer = writer.as_mut().ok_or(ConnectionError::Closed)?;
        writer.write_all(&encoded)?;
        writer.flush()?;
        Ok(())
    }

    pub fn close(&self) -> Result<(), ConnectionError> {
        self.writer.lock().unwrap().take();
        Ok(())
    }

    pub fn set_thread_id(&self, thread_id: String) {
        self.context.lock().unwrap().thread_id = Some(thread_id);
    }

    pub fn decode(&self, value: Value) -> Result<ConnectionEvent, ConnectionError> {
        let object = value.as_object().ok_or_else(|| {
            ConnectionError::Protocol("top-level message is not an object".to_string())
        })?;
        match (object.get("id"), object.get("method")) {
            (Some(id), Some(method)) => decode_server_request(id, method, object.get("params")),
            (None, Some(method)) => decode_notification(method, object.get("params")),
            (Some(id), None) => self.decode_response(id, object.get("result"), object.get("error")),
            (None, None) => Err(ConnectionError::Protocol(
                "message has neither id nor method".to_string(),
            )),
        }
    }

    fn decode_response(
        &self,
        id: &Value,
        result: Option<&Value>,
        error: Option<&Value>,
    ) -> Result<ConnectionEvent, ConnectionError> {
        let request_id = id.as_u64().ok_or_else(|| {
            ConnectionError::Protocol("response id is not an unsigned integer".to_string())
        })?;
        if result.is_some() == error.is_some() {
            return Err(ConnectionError::Protocol(
                "response must carry exactly one of result or error".to_string(),
            ));
        }
        let operation = self
            .pending
            .lock()
            .unwrap()
            .remove(&request_id)
            .ok_or_else(|| {
                ConnectionError::Protocol(format!("unknown response id {request_id}"))
            })?;
        let body = match (result, error) {
            (Some(result), None) => {
                validate_result(&operation, result)?;
                Ok(result.clone())
            }
            (None, Some(error)) => {
                Err(RpcError::from_value(error).map_err(ConnectionError::Protocol)?)
            }
            _ => unreachable!(),
        };
        Ok(ConnectionEvent::Response {
            operation,
            result: body,
        })
    }

    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    #[cfg(test)]
    pub fn set_next_id(&self, next_id: RequestId) {
        *self.next_id.lock().unwrap() = next_id;
    }
}

fn decode_server_request(
    id: &Value,
    method: &Value,
    params: Option<&Value>,
) -> Result<ConnectionEvent, ConnectionError> {
    let method = method.as_str().ok_or_else(|| {
        ConnectionError::Protocol("server request method is not a string".to_string())
    })?;
    Ok(ConnectionEvent::Request(ServerRequest::new(
        id.clone(),
        method,
        params.cloned().unwrap_or_else(|| json!({})),
    )))
}

fn decode_notification(
    method: &Value,
    params: Option<&Value>,
) -> Result<ConnectionEvent, ConnectionError> {
    let method = method.as_str().ok_or_else(|| {
        ConnectionError::Protocol("notification method is not a string".to_string())
    })?;
    Ok(ConnectionEvent::Notification(ServerNotification {
        method: method.to_string(),
        params: params.cloned().unwrap_or_else(|| json!({})),
    }))
}

fn operation_params(
    operation: &PendingOperation,
    context: &RequestContext,
) -> Result<Value, ConnectionError> {
    let cwd = context.root.to_string_lossy();
    let thread_id = || {
        context.thread_id.clone().ok_or_else(|| {
            ConnectionError::Protocol("thread request made before thread id was known".to_string())
        })
    };
    Ok(match operation {
        PendingOperation::Initialize => json!({
            "clientInfo":{"name":"build_bridge","title":"Build","version":env!("CARGO_PKG_VERSION")},
            "capabilities":{}
        }),
        PendingOperation::StartThread => {
            thread_open_params(cwd.as_ref(), context.model.as_deref(), None)
        }
        PendingOperation::ResumeThread => thread_open_params(
            cwd.as_ref(),
            context.model.as_deref(),
            Some(context.resume_id.as_deref().ok_or_else(|| {
                ConnectionError::Protocol("resume operation has no exact thread id".to_string())
            })?),
        ),
        PendingOperation::StartTurn { input } => json!({
            "threadId":thread_id()?,
            "input":[{"type":"text","text":input}],
            "model":context.model,
            "effort":context.effort,
        }),
        PendingOperation::SteerTurn { turn_id, input } => json!({
            "threadId":thread_id()?,
            "expectedTurnId":turn_id,
            "input":[{"type":"text","text":input}],
        }),
        PendingOperation::InterruptTurn { turn_id } => json!({
            "threadId":thread_id()?,
            "turnId":turn_id,
        }),
    })
}

fn thread_open_params(cwd: &str, model: Option<&str>, resume_id: Option<&str>) -> Value {
    let mut params = json!({
        "cwd":cwd,
        "model":model,
        "approvalPolicy":"never",
        "sandbox":"danger-full-access",
    });
    if let Some(thread_id) = resume_id {
        params["threadId"] = json!(thread_id);
    }
    params
}

fn validate_result(operation: &PendingOperation, result: &Value) -> Result<(), ConnectionError> {
    let required = match operation {
        PendingOperation::Initialize => result.is_object(),
        PendingOperation::StartThread | PendingOperation::ResumeThread => {
            result
                .pointer("/thread/id")
                .and_then(Value::as_str)
                .is_some()
                && result["model"].as_str().is_some()
                && result["cwd"].as_str().is_some()
                && result["approvalPolicy"].as_str().is_some()
                && result["sandbox"].is_object()
        }
        PendingOperation::StartTurn { .. } => {
            result.pointer("/turn/id").and_then(Value::as_str).is_some()
        }
        PendingOperation::SteerTurn { .. } => result["turnId"].as_str().is_some(),
        PendingOperation::InterruptTurn { .. } => result.is_object(),
    };
    if required {
        Ok(())
    } else {
        Err(ConnectionError::Protocol(format!(
            "{} response has the wrong body",
            operation.method()
        )))
    }
}

pub fn read_jsonl_frame(
    reader: &mut dyn Read,
    limit: usize,
) -> Result<Option<Value>, ConnectionError> {
    let mut frame = Vec::with_capacity(limit.min(8192));
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 if frame.is_empty() => return Ok(None),
            0 => return Err(ConnectionError::UnterminatedFrame),
            _ if byte[0] == b'\n' => break,
            _ => {
                frame.push(byte[0]);
                if frame.len() > limit
                    && !(frame.len() == limit + 1 && frame.last() == Some(&b'\r'))
                {
                    return Err(ConnectionError::FrameTooLarge(limit));
                }
            }
        }
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    if frame.len() > limit {
        return Err(ConnectionError::FrameTooLarge(limit));
    }
    if frame.is_empty() {
        return Err(ConnectionError::Protocol("blank JSONL frame".to_string()));
    }
    let text = std::str::from_utf8(&frame)?;
    Ok(Some(serde_json::from_str(text)?))
}

pub type SharedConnection = Arc<AppServerConnection>;
