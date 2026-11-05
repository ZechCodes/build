//! Bounded one-request private secret-release transport. Never part of MCP/HTTP.
use crate::secrets::{Result, SecretError, SecretValue};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_FRAME_BYTES: usize = 2048;
const MAX_CONNECTIONS: usize = 4;
const GLOBAL_PER_SECOND: u32 = 64;
const PER_CAPABILITY_PER_SECOND: u32 = 16;
const IO_BUDGET: Duration = Duration::from_secs(5);

// No Debug/Serialize: capability material never enters diagnostics.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) agent_id: String,
    pub(super) capability: String,
    pub(super) secret_id: String,
    pub(super) request_id: String,
}
impl Drop for Request {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.capability);
    }
}

pub(super) trait Backend: Send + Sync + 'static {
    /// Returns only after audit is committed and every metadata lock released.
    fn release(&self, request: &Request) -> Result<SecretValue>;
}

pub(super) struct Plane {
    backend: Arc<dyn Backend>,
    connections: Arc<Semaphore>,
    limits: Mutex<RateLimits>,
}
impl Plane {
    pub(super) fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            limits: Mutex::new(RateLimits::default()),
        }
    }
}

struct RateLimits {
    start: Instant,
    total: u32,
    buckets: HashMap<[u8; 32], u32>,
}
impl Default for RateLimits {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            total: 0,
            buckets: HashMap::new(),
        }
    }
}
impl RateLimits {
    fn admit(&mut self, capability: &str, now: Instant) -> bool {
        if now.saturating_duration_since(self.start) >= Duration::from_secs(1) {
            self.start = now;
            self.total = 0;
            self.buckets.clear();
        }
        if self.total >= GLOBAL_PER_SECOND {
            return false;
        }
        self.total += 1;
        let key = Sha256::digest(capability.as_bytes()).into();
        let count = self.buckets.entry(key).or_default();
        if *count >= PER_CAPABILITY_PER_SECOND {
            return false;
        }
        *count += 1;
        true
    }
}

pub(super) async fn serve(listener: UnixListener, backend: Arc<dyn Backend>) {
    let plane = Arc::new(Plane::new(backend));
    let mut backoff =
        crate::backoff::Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                backoff.reset();
                // Admission occurs before spawning, so queued clients cannot
                // create an unbounded task or blocking-pool backlog.
                if let Ok(permit) = plane.connections.clone().try_acquire_owned() {
                    tokio::spawn(handle_admitted(stream, plane.clone(), permit));
                }
            }
            Err(_) => {
                tokio::time::sleep(backoff.current()).await;
                backoff.increase();
            }
        }
    }
}

#[cfg(test)]
async fn handle(stream: UnixStream, plane: Arc<Plane>) {
    if let Ok(permit) = plane.connections.clone().try_acquire_owned() {
        handle_admitted(stream, plane, permit).await;
    }
}

async fn handle_admitted(mut stream: UnixStream, plane: Arc<Plane>, permit: OwnedSemaphorePermit) {
    let request = match read_request(&mut stream).await {
        Some(request) => request,
        None => {
            write_error(&mut stream, "unauthorized").await;
            return;
        }
    };
    let allowed = plane
        .limits
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .admit(&request.capability, Instant::now());
    if !allowed {
        write_error(&mut stream, "secret_limit").await;
        return;
    }
    let backend = plane.backend.clone();
    // The permit follows the blocking job if the connection task is cancelled.
    // No app/writer guard survives this callback into socket I/O.
    let Ok((result, _permit)) =
        tokio::task::spawn_blocking(move || (backend.release(&request), permit)).await
    else {
        return;
    };
    match result {
        Ok(value) => {
            let header = format!("{{\"ok\":true,\"length\":{}}}\n", value.as_bytes().len());
            let _ = tokio::time::timeout(IO_BUDGET, async {
                stream.write_all(header.as_bytes()).await?;
                stream.write_all(value.as_bytes()).await?;
                stream.shutdown().await
            })
            .await;
        }
        Err(error) => write_error(&mut stream, error_code(&error)).await,
    }
}

async fn read_request(stream: &mut UnixStream) -> Option<Request> {
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(MAX_FRAME_BYTES));
    let mut reader = tokio::io::BufReader::new(stream.take((MAX_FRAME_BYTES + 1) as u64));
    let count = tokio::time::timeout(IO_BUDGET, reader.read_until(b'\n', &mut bytes))
        .await
        .ok()?
        .ok()?;
    if count > MAX_FRAME_BYTES || bytes.last() != Some(&b'\n') {
        return None;
    }
    let request: Request = serde_json::from_slice(&bytes).ok()?;
    let fields = [
        &request.agent_id,
        &request.capability,
        &request.secret_id,
        &request.request_id,
    ];
    if fields
        .iter()
        .any(|s| s.is_empty() || s.len() > 128 || s.contains('\0'))
    {
        return None;
    }
    let uuid = uuid::Uuid::parse_str(&request.request_id).ok()?;
    (uuid.hyphenated().to_string() == request.request_id).then_some(request)
}

async fn write_error(stream: &mut UnixStream, code: &'static str) {
    let line = format!("{{\"ok\":false,\"error\":\"{code}\"}}\n");
    let _ = tokio::time::timeout(IO_BUDGET, stream.write_all(line.as_bytes())).await;
}

fn error_code(error: &SecretError) -> &'static str {
    match error {
        SecretError::Unauthorized => "unauthorized",
        SecretError::Expired => "secret_expired",
        SecretError::Revoked => "secret_revoked",
        SecretError::Limit => "secret_limit",
        _ => "secret_unavailable",
    }
}

#[cfg(test)]
mod tests;
