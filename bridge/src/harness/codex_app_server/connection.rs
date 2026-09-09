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
    #[error(
        "app-server frame exceeds {limit} bytes (observed at least {observed_at_least} bytes)"
    )]
    FrameTooLarge {
        limit: usize,
        observed_at_least: usize,
    },
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
    frame_reader: Mutex<JsonlFrameReader>,
}

impl AppServerConnection {
    pub fn new(writer: Box<dyn Write + Send>, limits: ConnectionLimits) -> AppServerConnection {
        AppServerConnection {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            limits,
            frame_reader: Mutex::new(JsonlFrameReader::new()),
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
            return Err(ConnectionError::FrameTooLarge {
                limit: self.limits.outbound_frame_bytes,
                observed_at_least: encoded.observed,
            });
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

    /// Reads one JSONL frame from the app-server's stdout under the inbound bound
    /// this connection owns and decodes it into a typed event, returning `None`
    /// at end of stream. This is the only reader of app-server stdout.
    pub fn read_event(
        &self,
        reader: &mut dyn Read,
    ) -> Result<Option<ConnectionEvent>, ConnectionError> {
        match self
            .frame_reader
            .lock()
            .unwrap()
            .read_jsonl_frame(reader, self.limits.inbound_frame_bytes)?
        {
            Some(value) => self.decode(value).map(Some),
            None => Ok(None),
        }
    }

    fn decode(&self, value: Value) -> Result<ConnectionEvent, ConnectionError> {
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
    observed: usize,
}

impl CappedBuffer {
    fn new(limit: usize) -> CappedBuffer {
        CappedBuffer {
            bytes: Vec::with_capacity(limit.min(8192)),
            limit,
            exceeded: false,
            observed: 0,
        }
    }
}

impl Write for CappedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.observed = self.observed.saturating_add(bytes.len());
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

const READ_CHUNK_BYTES: usize = 8 * 1024;

struct JsonlFrameReader {
    frame: Vec<u8>,
    buffered: [u8; READ_CHUNK_BYTES],
    buffered_start: usize,
    buffered_end: usize,
    failed: bool,
}

impl JsonlFrameReader {
    fn new() -> Self {
        Self {
            frame: Vec::with_capacity(8 * 1024),
            buffered: [0; READ_CHUNK_BYTES],
            buffered_start: 0,
            buffered_end: 0,
            failed: false,
        }
    }

    fn read_jsonl_frame(
        &mut self,
        reader: &mut dyn Read,
        limit: usize,
    ) -> Result<Option<Value>, ConnectionError> {
        if self.failed {
            return Err(ConnectionError::Closed);
        }
        self.frame.clear();
        let mut observed = 0_usize;
        let mut terminal_carriage_return = false;

        loop {
            if self.buffered_start == self.buffered_end {
                self.buffered_end = reader.read(&mut self.buffered)?;
                self.buffered_start = 0;
                if self.buffered_end == 0 {
                    return if observed == 0 {
                        Ok(None)
                    } else {
                        Err(ConnectionError::UnterminatedFrame)
                    };
                }
            }

            let available = &self.buffered[self.buffered_start..self.buffered_end];
            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |newline| newline + 1);
            let segment = &available[..consumed];
            self.buffered_start += consumed;

            let content = segment.strip_suffix(b"\n").unwrap_or(segment);
            observed = observed.saturating_add(content.len());
            let remaining = limit - self.frame.len();
            if terminal_carriage_return {
                if !content.is_empty() {
                    self.failed = true;
                    return Err(ConnectionError::FrameTooLarge {
                        limit,
                        observed_at_least: observed,
                    });
                }
            } else if content.len() > remaining {
                if content.len() == remaining + 1 && content.last() == Some(&b'\r') {
                    self.frame.extend_from_slice(&content[..remaining]);
                    terminal_carriage_return = true;
                } else {
                    self.failed = true;
                    return Err(ConnectionError::FrameTooLarge {
                        limit,
                        observed_at_least: observed,
                    });
                }
            } else {
                self.frame.extend_from_slice(content);
            }

            if segment.ends_with(b"\n") {
                if !terminal_carriage_return && self.frame.last() == Some(&b'\r') {
                    self.frame.pop();
                }
                if self.frame.is_empty() {
                    return Err(ConnectionError::Protocol("blank JSONL frame".to_string()));
                }
                return decode_json_frame(&self.frame).map(Some);
            }
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

    use super::super::limits::{AppServerLimits, DEFAULT_INBOUND_FRAME_BYTES};
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
            .respond(ServerResponse::method_not_found(json!(1), "unsupported"))
            .unwrap_err()
            .to_string()
            .contains("exact flush error"));
        assert_eq!(flushes.load(Ordering::SeqCst), 3);
    }

    fn frame_reader(inbound_frame_bytes: usize) -> AppServerConnection {
        AppServerConnection::memory(
            AppServerLimits {
                inbound_frame_bytes,
                ..AppServerLimits::default()
            }
            .connection(),
        )
    }

    fn notification_method(event: Option<ConnectionEvent>) -> String {
        match event {
            Some(ConnectionEvent::Notification(inbound)) => inbound.method,
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    #[test]
    fn end_of_stream_reads_as_no_event() {
        let mut empty = Cursor::new(b"".as_slice());
        assert_eq!(frame_reader(64).read_event(&mut empty).unwrap(), None);
    }

    #[test]
    fn crlf_frames_decode_up_to_the_exact_inbound_limit() {
        let crlf = b"{\"method\":\"initialized\"}\r\n";
        let mut reader = Cursor::new(crlf);
        assert_eq!(
            notification_method(frame_reader(crlf.len()).read_event(&mut reader).unwrap()),
            "initialized"
        );

        let exact = b"{\"method\":\"exact\"}\r\n";
        let mut reader = Cursor::new(exact);
        let payload_length = exact.len() - b"\r\n".len();
        assert_eq!(
            notification_method(
                frame_reader(payload_length)
                    .read_event(&mut reader)
                    .unwrap()
            ),
            "exact"
        );
    }

    #[test]
    fn a_carriage_return_at_the_limit_does_not_lift_the_inbound_bound() {
        let connection = frame_reader(20);
        let overrun = [
            b"12345678901234567890\r".as_slice(),
            b"x".repeat(64).as_slice(),
            b"\n{\"method\":\"next\"}\n",
        ]
        .concat();
        let mut reader = Cursor::new(overrun);
        assert!(matches!(
            connection.read_event(&mut reader),
            Err(ConnectionError::FrameTooLarge {
                limit: 20,
                observed_at_least: 85
            })
        ));
        assert!(matches!(
            connection.read_event(&mut reader),
            Err(ConnectionError::Closed)
        ));
    }

    fn read_frame(bytes: Vec<u8>) -> Result<Option<ConnectionEvent>, ConnectionError> {
        frame_reader(bytes.len() + 2).read_event(&mut Cursor::new(bytes))
    }

    #[test]
    fn malformed_frames_fail_at_the_stage_that_owns_them() {
        let protocol_violations = [
            (b"\n".to_vec(), "blank JSONL frame"),
            (b"{}\n".to_vec(), "message has neither id nor method"),
            (b"[]\n".to_vec(), "top-level message is not an object"),
        ];
        for (bad, violation) in protocol_violations {
            match read_frame(bad.clone()) {
                Err(ConnectionError::Protocol(message)) => assert_eq!(message, violation),
                other => panic!("{bad:?} yielded {other:?}"),
            }
        }
        assert!(matches!(
            read_frame(b"{bad}\n".to_vec()),
            Err(ConnectionError::InvalidJson(_))
        ));
        assert!(matches!(
            read_frame(b"{} trailing\n".to_vec()),
            Err(ConnectionError::InvalidJson(_))
        ));
        assert!(matches!(
            read_frame(vec![0xff, b'\n']),
            Err(ConnectionError::InvalidUtf8(_))
        ));
        assert!(matches!(
            read_frame(b"{}".to_vec()),
            Err(ConnectionError::UnterminatedFrame)
        ));
    }

    #[test]
    fn oversized_frame_fails_fast_and_makes_the_reader_terminal() {
        let connection = frame_reader(20);
        let mut reader = Cursor::new(b"123456789-not-another-frame\n{\"method\":\"next\"}\n");
        assert!(matches!(
            connection.read_event(&mut reader),
            Err(ConnectionError::FrameTooLarge {
                limit: 20,
                observed_at_least: 27
            })
        ));
        assert!(matches!(
            connection.read_event(&mut reader),
            Err(ConnectionError::Closed)
        ));
    }

    #[test]
    fn oversized_frame_returns_without_waiting_for_a_newline() {
        struct NoMoreReads(bool);
        impl Read for NoMoreReads {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(
                    !self.0,
                    "reader was polled after the size violation was known"
                );
                self.0 = true;
                buffer[..9].copy_from_slice(b"123456789");
                Ok(9)
            }
        }

        let mut reader = NoMoreReads(false);
        assert!(matches!(
            frame_reader(8).read_event(&mut reader),
            Err(ConnectionError::FrameTooLarge {
                limit: 8,
                observed_at_least: 9
            })
        ));
    }

    #[test]
    fn chunk_read_ahead_is_reused_for_following_frames() {
        struct CountReads {
            bytes: Cursor<Vec<u8>>,
            reads: Arc<AtomicUsize>,
        }

        impl Read for CountReads {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.reads.fetch_add(1, Ordering::SeqCst);
                self.bytes.read(buffer)
            }
        }

        let reads = Arc::new(AtomicUsize::new(0));
        let mut reader = CountReads {
            bytes: Cursor::new(b"{\"method\":\"one\"}\n{\"method\":\"two\"}\n".to_vec()),
            reads: Arc::clone(&reads),
        };
        let connection = frame_reader(64);

        assert_eq!(
            notification_method(connection.read_event(&mut reader).unwrap()),
            "one"
        );
        assert_eq!(
            notification_method(connection.read_event(&mut reader).unwrap()),
            "two"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn large_frames_grow_storage_on_demand_and_decode_across_chunks() {
        let method = "m".repeat(READ_CHUNK_BYTES * 2);
        let encoded = serde_json::to_vec(&json!({ "method": method })).unwrap();
        let mut framed = encoded.clone();
        framed.push(b'\n');
        let connection = frame_reader(encoded.len());

        assert_eq!(
            notification_method(connection.read_event(&mut Cursor::new(framed)).unwrap()),
            method
        );
        let capacity = connection.frame_reader.lock().unwrap().frame.capacity();
        assert!(capacity >= encoded.len());
        assert!(capacity < encoded.len() + READ_CHUNK_BYTES * 2);
    }

    #[test]
    fn oversized_error_reports_limit_and_observed_bytes_without_payload() {
        let connection = frame_reader(8);
        let payload = b"sensitive-payload-value\n";
        let error = connection
            .read_event(&mut Cursor::new(payload))
            .unwrap_err();

        assert!(matches!(
            error,
            ConnectionError::FrameTooLarge {
                limit: 8,
                observed_at_least: 23
            }
        ));
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("8"), "{diagnostic}");
        assert!(diagnostic.contains("23"), "{diagnostic}");
        assert!(!diagnostic.contains("sensitive"), "{diagnostic}");
    }

    #[test]
    fn exact_limit_crlf_remains_valid_when_every_byte_is_a_separate_read() {
        struct OneByte(Cursor<Vec<u8>>);
        impl Read for OneByte {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.0.read(&mut buffer[..1])
            }
        }

        let encoded = b"{\"method\":\"exact\"}";
        let mut framed = encoded.to_vec();
        framed.extend_from_slice(b"\r\n");
        let connection = frame_reader(encoded.len());
        let mut reader = OneByte(Cursor::new(framed));

        assert_eq!(
            notification_method(connection.read_event(&mut reader).unwrap()),
            "exact"
        );
        assert_eq!(
            connection.frame_reader.lock().unwrap().frame.capacity(),
            8192
        );
    }

    #[test]
    fn default_limit_crlf_never_grows_storage_past_the_inbound_bound() {
        let prefix = b"{\"method\":\"";
        let suffix = b"\"}";
        let method_length = DEFAULT_INBOUND_FRAME_BYTES - prefix.len() - suffix.len();
        let mut framed = Vec::with_capacity(DEFAULT_INBOUND_FRAME_BYTES + 2);
        framed.extend_from_slice(prefix);
        framed.extend(std::iter::repeat_n(b'm', method_length));
        framed.extend_from_slice(suffix);
        framed.extend_from_slice(b"\r\n");
        let connection = frame_reader(DEFAULT_INBOUND_FRAME_BYTES);

        assert!(connection
            .read_event(&mut Cursor::new(framed))
            .unwrap()
            .is_some());
        assert!(
            connection.frame_reader.lock().unwrap().frame.capacity() <= DEFAULT_INBOUND_FRAME_BYTES
        );
    }

    #[test]
    fn a_second_carriage_return_after_the_limit_is_rejected_when_fragmented() {
        struct OneByte(Cursor<Vec<u8>>);
        impl Read for OneByte {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.0.read(&mut buffer[..1])
            }
        }

        let limit = 8;
        let mut reader = OneByte(Cursor::new(b"12345678\r\r\n".to_vec()));
        assert!(matches!(
            frame_reader(limit).read_event(&mut reader),
            Err(ConnectionError::FrameTooLarge {
                limit: 8,
                observed_at_least: 10
            })
        ));
    }
}
