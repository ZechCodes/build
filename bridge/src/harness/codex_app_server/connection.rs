use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use super::limits::ConnectionLimits;
use super::protocol::{
    ClientNotification, ConnectionEvent, InboundNotification, InboundServerRequest,
    PendingOperation, RequestId, RpcError, ServerResponse,
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

pub struct AppServerConnection {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<BTreeMap<RequestId, PendingOperation>>,
    next_id: Mutex<RequestId>,
    limits: ConnectionLimits,
}

impl AppServerConnection {
    pub fn new(writer: Box<dyn Write + Send>, limits: ConnectionLimits) -> AppServerConnection {
        AppServerConnection {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            limits,
        }
    }

    #[cfg(test)]
    pub fn memory(limits: ConnectionLimits) -> AppServerConnection {
        AppServerConnection::new(Box::new(Vec::<u8>::new()), limits)
    }

    #[cfg(test)]
    pub fn failing_writer(limits: ConnectionLimits) -> AppServerConnection {
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
        AppServerConnection::new(Box::new(Fails), limits)
    }

    pub fn request(&self, operation: PendingOperation) -> Result<(), ConnectionError> {
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= self.limits.pending_requests {
            return Err(ConnectionError::PendingLimit(self.limits.pending_requests));
        }
        let request_id = self.allocate_request_id()?;
        pending.insert(request_id, operation.clone());
        if let Err(error) = self.write_operation(request_id, &operation) {
            pending.remove(&request_id);
            return Err(error);
        }
        Ok(())
    }

    fn allocate_request_id(&self) -> Result<RequestId, ConnectionError> {
        let mut next_id = self.next_id.lock().unwrap();
        let request_id = *next_id;
        *next_id = next_id
            .checked_add(1)
            .ok_or(ConnectionError::RequestIdExhausted)?;
        Ok(request_id)
    }

    pub fn notify(&self, notification: ClientNotification) -> Result<(), ConnectionError> {
        self.write_serialized(&notification)
    }

    pub fn respond(&self, response: ServerResponse) -> Result<(), ConnectionError> {
        self.write_serialized(&response)
    }

    fn write_operation(
        &self,
        request_id: RequestId,
        operation: &PendingOperation,
    ) -> Result<(), ConnectionError> {
        let mut encoded = CappedBuffer::new(self.limits.outbound_frame_bytes);
        let result = operation.serialize_request(request_id, &mut encoded);
        self.finish_serialization(encoded, result)
    }

    fn write_serialized(&self, value: &impl Serialize) -> Result<(), ConnectionError> {
        let mut encoded = CappedBuffer::new(self.limits.outbound_frame_bytes);
        let result = serde_json::to_writer(&mut encoded, value);
        self.finish_serialization(encoded, result)
    }

    fn finish_serialization(
        &self,
        mut encoded: CappedBuffer,
        result: Result<(), serde_json::Error>,
    ) -> Result<(), ConnectionError> {
        if encoded.exceeded {
            return Err(ConnectionError::FrameTooLarge(
                self.limits.outbound_frame_bytes,
            ));
        }
        result?;
        encoded.bytes.push(b'\n');
        let mut writer = self.writer.lock().unwrap();
        let writer = writer.as_mut().ok_or(ConnectionError::Closed)?;
        writer.write_all(&encoded.bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn close(&self) -> Result<(), ConnectionError> {
        let Some(mut writer) = self.writer.lock().unwrap().take() else {
            return Ok(());
        };
        writer.flush()?;
        Ok(())
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
            (Some(result), None) => operation
                .decode_result(result)
                .map_err(ConnectionError::Protocol)
                .map(Ok)?,
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
    pub fn pending_ids(&self) -> Vec<RequestId> {
        self.pending.lock().unwrap().keys().copied().collect()
    }

    #[cfg(test)]
    pub fn set_next_id(&self, next_id: RequestId) {
        *self.next_id.lock().unwrap() = next_id;
    }
}

struct CappedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl CappedBuffer {
    fn new(limit: usize) -> CappedBuffer {
        CappedBuffer {
            bytes: Vec::with_capacity(limit.min(8192)),
            limit,
            exceeded: false,
        }
    }
}

impl Write for CappedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "outbound frame limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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
    Ok(ConnectionEvent::Request(InboundServerRequest {
        id: id.clone(),
        method: method.to_string(),
        params: params.cloned().unwrap_or(Value::Null),
    }))
}

fn decode_notification(
    method: &Value,
    params: Option<&Value>,
) -> Result<ConnectionEvent, ConnectionError> {
    let method = method.as_str().ok_or_else(|| {
        ConnectionError::Protocol("notification method is not a string".to_string())
    })?;
    Ok(ConnectionEvent::Notification(InboundNotification {
        method: method.to_string(),
        params: params.cloned().unwrap_or(Value::Null),
    }))
}

pub fn read_jsonl_frame(
    reader: &mut dyn Read,
    limit: usize,
) -> Result<Option<Value>, ConnectionError> {
    let Some(frame) = read_jsonl_bytes(reader, limit)? else {
        return Ok(None);
    };
    decode_json_frame(&frame).map(Some)
}

fn read_jsonl_bytes(
    reader: &mut dyn Read,
    limit: usize,
) -> Result<Option<Vec<u8>>, ConnectionError> {
    let mut frame = Vec::with_capacity(limit.saturating_add(1));
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 if frame.is_empty() => return Ok(None),
            0 => return Err(ConnectionError::UnterminatedFrame),
            _ if byte[0] == b'\n' => break,
            _ if frame.len() == limit && byte[0] != b'\r' => {
                return discard_oversized_frame(reader, limit)
            }
            _ => frame.push(byte[0]),
        }
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    if frame.is_empty() {
        return Err(ConnectionError::Protocol("blank JSONL frame".to_string()));
    }
    Ok(Some(frame))
}

fn discard_oversized_frame(
    reader: &mut dyn Read,
    limit: usize,
) -> Result<Option<Vec<u8>>, ConnectionError> {
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 => return Err(ConnectionError::FrameTooLarge(limit)),
            _ if byte[0] == b'\n' => return Err(ConnectionError::FrameTooLarge(limit)),
            _ => continue,
        }
    }
}

fn decode_json_frame(frame: &[u8]) -> Result<Value, ConnectionError> {
    let text = std::str::from_utf8(frame)?;
    Ok(serde_json::from_str(text)?)
}

pub type SharedConnection = Arc<AppServerConnection>;

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Error, ErrorKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::super::limits::AppServerLimits;
    use super::*;

    #[test]
    fn capped_outbound_serializer_never_retains_more_than_the_limit() {
        let mut buffer = CappedBuffer::new(8);
        assert!(serde_json::to_writer(&mut buffer, &"x".repeat(1024)).is_err());
        assert!(buffer.exceeded);
        assert!(buffer.bytes.len() <= 8);
    }

    #[test]
    fn close_propagates_the_writer_flush_error() {
        struct FlushFails;
        impl Write for FlushFails {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(Error::new(ErrorKind::BrokenPipe, "exact close error"))
            }
        }

        let connection = AppServerConnection::new(
            Box::new(FlushFails),
            AppServerLimits::default().connection(),
        );
        let error = connection.close().unwrap_err().to_string();
        assert!(error.contains("exact close error"), "{error}");
        assert!(connection.close().is_ok());
    }

    #[test]
    fn every_write_path_propagates_flush_failure() {
        struct FlushFails(Arc<AtomicUsize>);
        impl Write for FlushFails {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(Error::new(ErrorKind::BrokenPipe, "exact flush error"))
            }
        }

        let flushes = Arc::new(AtomicUsize::new(0));
        let connection = AppServerConnection::new(
            Box::new(FlushFails(Arc::clone(&flushes))),
            AppServerLimits::default().connection(),
        );
        assert!(connection
            .request(PendingOperation::Initialize)
            .unwrap_err()
            .to_string()
            .contains("exact flush error"));
        assert_eq!(connection.pending_count(), 0);
        assert!(connection
            .notify(ClientNotification::Initialized)
            .unwrap_err()
            .to_string()
            .contains("exact flush error"));
        assert!(connection
            .respond(ServerResponse::error(json!(1), -32601, "unsupported"))
            .unwrap_err()
            .to_string()
            .contains("exact flush error"));
        assert_eq!(flushes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn oversized_frame_is_discarded_through_newline_before_the_error_returns() {
        let mut reader = Cursor::new(b"12345-not-another-frame\n{}\n");
        assert!(matches!(
            read_jsonl_frame(&mut reader, 4),
            Err(ConnectionError::FrameTooLarge(4))
        ));
        assert_eq!(read_jsonl_frame(&mut reader, 4).unwrap(), Some(json!({})));
    }
}
