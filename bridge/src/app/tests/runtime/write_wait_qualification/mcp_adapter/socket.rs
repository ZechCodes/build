//! One owned request worker retains its socket permit through actual server join.
use super::*;
use futures_util::FutureExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::OwnedSemaphorePermit;

const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct RequestWorker {
    pub inner: Arc<Inner>,
    pub endpoint: Arc<Endpoint>,
    pub original: String,
    pub method: String,
    pub bytes: Vec<u8>,
    pub send: oneshot::Sender<Result<ReplyProof, String>>,
}

struct SocketLease {
    endpoint: Arc<Endpoint>,
    _permit: OwnedSemaphorePermit,
}

impl SocketLease {
    fn new(endpoint: Arc<Endpoint>, permit: OwnedSemaphorePermit) -> Self {
        let active = endpoint.active.fetch_add(1, Ordering::AcqRel) + 1;
        endpoint.maximum_active.fetch_max(active, Ordering::AcqRel);
        Self {
            endpoint,
            _permit: permit,
        }
    }
}
impl Drop for SocketLease {
    fn drop(&mut self) {
        self.endpoint.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl RequestWorker {
    pub(super) async fn run(self) {
        let result = self.exchange_and_join().await;
        if let Err(error) = &result {
            self.inner.error(&self.original, error.clone());
        }
        if self.send.send(result).is_err() {
            self.inner
                .update(&self.original, |row| row.abandoned = true);
            self.inner.identity_error(
                &self.original,
                "caller abandoned its owned MCP worker".into(),
            );
        }
        self.inner.finish_worker(&self.original);
    }

    async fn exchange_and_join(&self) -> Result<ReplyProof, String> {
        self.inner.update(&self.original, |row| {
            row.socket_wait_started_at = Some(Instant::now())
        });
        let permit = Arc::clone(&self.endpoint.sockets)
            .acquire_owned()
            .await
            .map_err(|_| "shared identity socket budget closed".to_owned())?;
        self.inner.update(&self.original, |row| {
            row.socket_acquired_at = Some(Instant::now())
        });
        let (client, daemon) =
            UnixStream::pair().map_err(|error| format!("owned socket pair: {error}"))?;
        let lease = SocketLease::new(Arc::clone(&self.endpoint), permit);
        let server = tokio::spawn(handle_done_stream(
            Arc::clone(&self.inner.state),
            daemon,
            Arc::clone(&self.inner.plane),
        ));
        let mut client = BufReader::new(client);
        // A panic in caller instrumentation/decoding still closes and JOINS the
        // accepted server. The server handle/permit live outside this boundary.
        let exchange = std::panic::AssertUnwindSafe(self.exchange(&mut client))
            .catch_unwind()
            .await;
        let mut result = match exchange {
            Ok(result) => result,
            Err(_) => Err("MCP client exchange panicked; owned server still drains".into()),
        };
        drop(client);
        self.inner.update(&self.original, |row| {
            row.client_closed_at = Some(Instant::now())
        });
        // Deliberately not timeout-by-value: timeout must not detach an accepted
        // server or release its identity permit. The worker remains owned in
        // McpHarness until this join completes; bounded shutdown reports pending.
        match server.await {
            Ok(()) => self.inner.update(&self.original, |row| {
                row.server_joined_at = Some(Instant::now())
            }),
            Err(error) => {
                let message = format!("owned MCP server join failed: {error}");
                self.inner.identity_error(&self.original, message.clone());
                result = Err(message);
            }
        }
        drop(lease);
        result
    }

    async fn exchange(&self, client: &mut BufReader<UnixStream>) -> Result<ReplyProof, String> {
        let deadline = tokio::time::Instant::now() + INFRASTRUCTURE_BOUND;
        tokio::time::timeout_at(deadline, self.exchange_inner(client))
            .await
            .map_err(|_| "MCP socket infrastructure deadline; no final is fabricated".to_owned())?
    }

    async fn exchange_inner(
        &self,
        client: &mut BufReader<UnixStream>,
    ) -> Result<ReplyProof, String> {
        let sent_at = Instant::now();
        self.inner
            .update(&self.original, |row| row.sent_at = Some(sent_at));
        self.inner.watch.sent(&self.original);
        client
            .get_mut()
            .write_all(&self.bytes)
            .await
            .map_err(|error| format!("actual MCP write: {error}"))?;
        let line = read_bounded_line(client).await?;
        let reply: Value = serde_json::from_str(&line)
            .map_err(|error| format!("MCP final JSON parse failed: {error}"))?;
        let final_reply_at = Instant::now();
        self.validate_final(&reply)?;
        let outcome = if reply["ok"] == true {
            FinalOutcome::Reply
        } else {
            FinalOutcome::ProtocolError
        };
        self.inner
            .watch
            .decoded_final_at(&self.original, outcome, final_reply_at);
        self.inner.update(&self.original, |row| {
            row.final_reply_at = Some(final_reply_at);
            row.actual_wire_id = reply["id"].as_str().map(str::to_owned);
            row.response_ok = reply["ok"].as_bool();
            row.response_bytes = Some(line.len());
            row.response_sha256 = Some(format!("{:x}", Sha256::digest(line.as_bytes())));
        });
        // Half-close the only request direction, then require actual EOF after
        // its one final response. This is not a generic pipelined MCP client.
        client
            .get_mut()
            .shutdown()
            .await
            .map_err(|error| format!("MCP write half close: {error}"))?;
        let mut extra = String::new();
        let extra_bytes = read_optional_bounded_line(client, &mut extra).await?;
        if extra_bytes != 0 {
            let extra_id = serde_json::from_str::<Value>(&extra)
                .ok()
                .and_then(|body| body["id"].as_str().map(str::to_owned));
            self.inner.identity_error(
                extra_id.as_deref().unwrap_or(&self.original),
                "unexpected duplicate/extra MCP response on one-request socket".into(),
            );
            return Err("MCP socket returned more than one final response".into());
        }
        Ok(ReplyProof {
            original_id: self.original.clone(),
            method: self.method.clone(),
            reply,
            sent_at,
            final_reply_at,
        })
    }

    fn validate_final(&self, reply: &Value) -> Result<(), String> {
        if !reply.is_object()
            || !reply["ok"].is_boolean()
            || reply.get("accepted").is_some()
            || reply.get("type").is_some()
        {
            return Err("MCP response is not an actual final action reply".into());
        }
        if let Some(id) = reply.get("id") {
            if id.as_str() != Some(self.original.as_str()) {
                let received = id.as_str().unwrap_or("<non-string-MCP-id>");
                self.inner.identity_error(
                    received,
                    format!(
                        "MCP final ID differs from dedicated original {}",
                        self.original
                    ),
                );
                return Err("MCP final identity mismatch".into());
            }
        }
        // Native current protocol has no reply id. A dedicated fresh socket
        // containing exactly this one authenticated request is its correlation.
        Ok(())
    }
}

async fn read_bounded_line(client: &mut BufReader<UnixStream>) -> Result<String, String> {
    let mut line = String::new();
    let length = read_optional_bounded_line(client, &mut line).await?;
    if length == 0 {
        return Err("MCP server closed before actual final response".into());
    }
    if !line.ends_with('\n') {
        return Err("MCP final frame lacks its line terminator".into());
    }
    Ok(line)
}

async fn read_optional_bounded_line(
    client: &mut BufReader<UnixStream>,
    line: &mut String,
) -> Result<usize, String> {
    let mut bounded = client.take((MAX_REPLY_BYTES + 1) as u64);
    let length = bounded
        .read_line(line)
        .await
        .map_err(|error| format!("MCP actual response read: {error}"))?;
    if length > MAX_REPLY_BYTES {
        return Err("MCP response exceeded declared harness byte bound".into());
    }
    Ok(length)
}
