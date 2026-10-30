//! Bounded wire parsing/encoding. No automatic content decompression.
use std::io::{self, Write};
use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;

use super::limits::{BufferReservation, BODY_IDLE_TIMEOUT};

const AUTHORITY_RECHECK: std::time::Duration = std::time::Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    TooLarge,
    InvalidJson,
    Timeout,
    Disconnected,
    Encoding,
    Unauthorized,
}

pub async fn read_json(body: Body, limit: usize) -> Result<Value, BodyError> {
    let mut stream = body.into_data_stream();
    let mut buffer = Vec::with_capacity(limit.min(8192));
    while let Some(chunk) = tokio::time::timeout(BODY_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| BodyError::Timeout)?
    {
        let chunk = chunk.map_err(|_| BodyError::Disconnected)?;
        if chunk.len() > limit.saturating_sub(buffer.len()) {
            return Err(BodyError::TooLarge);
        }
        buffer.extend_from_slice(&chunk);
    }
    if buffer.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(&buffer).map_err(|_| BodyError::InvalidJson)
}

/// Read a body only while its already-verified authority remains live. The
/// hard deadline does not reset when a peer supplies another chunk.
pub async fn read_json_authorized(
    body: Body,
    limit: usize,
    deadline: tokio::time::Instant,
    check: impl Fn() -> bool,
) -> Result<Value, BodyError> {
    let mut stream = body.into_data_stream();
    let mut buffer = Vec::with_capacity(limit.min(8192));
    let mut recheck = tokio::time::interval(AUTHORITY_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if !check() || tokio::time::Instant::now() >= deadline {
            return Err(BodyError::Unauthorized);
        }
        let next = stream.next();
        tokio::pin!(next);
        let idle = tokio::time::sleep(BODY_IDLE_TIMEOUT);
        tokio::pin!(idle);
        let chunk = loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Err(BodyError::Unauthorized),
                _ = recheck.tick() => {
                    if !check() { return Err(BodyError::Unauthorized); }
                }
                _ = &mut idle => return Err(BodyError::Timeout),
                chunk = &mut next => break chunk,
            }
        };
        let Some(chunk) = chunk else { break };
        if !check() {
            return Err(BodyError::Unauthorized);
        }
        let chunk = chunk.map_err(|_| BodyError::Disconnected)?;
        if chunk.len() > limit.saturating_sub(buffer.len()) {
            return Err(BodyError::TooLarge);
        }
        buffer.extend_from_slice(&chunk);
    }
    if buffer.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(&buffer).map_err(|_| BodyError::InvalidJson)
}

pub fn encode_json(value: &Value, limit: usize) -> Result<Vec<u8>, BodyError> {
    let mut writer = CappedWriter {
        bytes: Vec::with_capacity(limit.min(8192)),
        limit,
        full: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.full {
            BodyError::TooLarge
        } else {
            BodyError::Encoding
        });
    }
    Ok(writer.bytes)
}

struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
    full: bool,
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.full = true;
            return Err(io::Error::other("HTTP response limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Bytes retains the guard while Hyper buffers or writes any clone/slice of the
/// response, rather than releasing capacity when the body produces its last item.
pub fn reserved_bytes(bytes: Vec<u8>, permit: Arc<BufferReservation>) -> Bytes {
    Bytes::from_owner(ReservedBytes {
        bytes,
        _permit: permit,
    })
}

pub fn authority_bytes(bytes: Vec<u8>, ticket: super::listener::OutputTicket) -> Bytes {
    Bytes::from_owner(AuthorityBytes {
        bytes,
        ticket: Some(ticket),
    })
}

struct AuthorityBytes {
    bytes: Vec<u8>,
    ticket: Option<super::listener::OutputTicket>,
}
impl AsRef<[u8]> for AuthorityBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for AuthorityBytes {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.release();
        }
    }
}

struct ReservedBytes {
    bytes: Vec<u8>,
    _permit: Arc<BufferReservation>,
}
impl AsRef<[u8]> for ReservedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
