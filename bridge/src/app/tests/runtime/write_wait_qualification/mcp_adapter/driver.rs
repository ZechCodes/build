//! Actual execute/ownership and bounded shutdown orchestration.
use super::*;

impl McpHarness {
    /// Caller already has an immutable schedule and actual offer before any
    /// writer/socket credit wait. No schedule/offer/reset occurs in this adapter.
    pub async fn execute(
        &self,
        endpoint: usize,
        original: &str,
        action: BridgeAction,
        read: bool,
    ) -> Result<ReplyProof, String> {
        let endpoint_ref = self
            .inner
            .endpoints
            .get(endpoint)
            .ok_or("unknown MCP endpoint")?;
        let frame = json!({"id":original,"task_id":endpoint_ref.identity.agent,
            "session_token":endpoint_ref.identity.token,"request":action});
        validate_call(&self.inner, endpoint_ref, original, &frame, read)?;
        let method = format!("mcp.control:{}", action.tool_name());
        let bytes = format!("{frame}\n").into_bytes();
        let (send, receive) = oneshot::channel();
        {
            let mut jobs = self.inner.jobs.lock().unwrap();
            if !jobs.accepting {
                return Err("MCP adapter is closing; no new work".into());
            }
            if jobs.evidence.calls.contains_key(original) {
                drop(jobs);
                self.inner
                    .identity_error(original, "duplicate original; retries are forbidden".into());
                return Err("duplicate MCP original".into());
            }
            jobs.evidence.calls.insert(
                original.into(),
                McpCallRow {
                    original_id: original.into(),
                    endpoint,
                    method: method.clone(),
                    read,
                    queued_at: Instant::now(),
                    socket_wait_started_at: None,
                    socket_acquired_at: None,
                    sent_at: None,
                    final_reply_at: None,
                    response_ok: None,
                    response_bytes: None,
                    response_sha256: None,
                    actual_wire_id: None,
                    client_closed_at: None,
                    server_joined_at: None,
                    worker_finished_at: None,
                    worker_reaped_at: None,
                    abandoned: false,
                    error: None,
                },
            );
            let worker = socket::RequestWorker {
                inner: Arc::clone(&self.inner),
                endpoint: Arc::clone(endpoint_ref),
                original: original.into(),
                method,
                bytes,
                send,
            };
            let mut active = (**self.inner.active.load()).clone();
            active.insert(original.to_owned(), ());
            self.inner.active.store(Arc::new(active));
            let handle = tokio::spawn(worker.run());
            jobs.owned.insert(original.into(), handle);
        }
        let result = match tokio::time::timeout(INFRASTRUCTURE_BOUND, receive).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("owned MCP worker ended without returning response evidence".into()),
            Err(_) => Err(
                "MCP caller infrastructure deadline; owned worker/server retained, no retry".into(),
            ),
        };
        if let Err(error) = &result {
            self.inner.error(original, error.clone());
        }
        self.reap_finished().await;
        result
    }

    pub fn stop_offers(&self) {
        self.inner.jobs.lock().unwrap().accepting = false;
    }

    pub fn observation(&self) -> McpObservation {
        McpObservation {
            active: Arc::clone(&self.inner.active),
            failed: Arc::clone(&self.inner.failed),
        }
    }

    pub fn evidence(&self) -> McpEvidence {
        let mut evidence = self.inner.jobs.lock().unwrap().evidence.clone();
        evidence.sockets = self
            .inner
            .endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| SocketEvidence {
                endpoint: index,
                agent: endpoint.identity.agent.clone(),
                passive_router: endpoint.identity.read_only,
                active: endpoint.active.load(Ordering::Acquire),
                maximum_active: endpoint.maximum_active.load(Ordering::Acquire),
                limit: SOCKETS_PER_IDENTITY,
            })
            .collect();
        evidence
    }

    /// No pending handle is removed/awaited by value. Finished handles alone
    /// are reaped, so cancellation of this observer cannot detach accepted work.
    pub async fn reap_finished(&self) {
        let finished = {
            let mut jobs = self.inner.jobs.lock().unwrap();
            let ids: Vec<_> = jobs
                .owned
                .iter()
                .filter(|(_, handle)| handle.is_finished())
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .map(|id| {
                    let handle = jobs.owned.remove(&id).unwrap();
                    (id, handle)
                })
                .collect::<Vec<_>>()
        };
        for (id, handle) in finished {
            if let Err(error) = handle.await {
                self.inner
                    .identity_error(&id, format!("owned worker panicked: {error}"));
            }
            self.inner
                .update(&id, |row| row.worker_reaped_at = Some(Instant::now()));
        }
    }

    pub fn is_quiet(&self) -> bool {
        self.inner.jobs.lock().unwrap().owned.is_empty()
            && self
                .inner
                .endpoints
                .iter()
                .all(|endpoint| endpoint.active.load(Ordering::Acquire) == 0)
    }

    /// Called only after caller tasks and accepted finite service obligations
    /// drain. If deadline expires, retain this Arc and report unfinished work;
    /// never abort/detach an accepted server or release its socket permit early.
    pub async fn shutdown(&self, deadline: tokio::time::Instant) -> McpShutdownReport {
        self.stop_offers();
        loop {
            self.reap_finished().await;
            if self.is_quiet() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + Duration::from_millis(1)).min(deadline),
            )
            .await;
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.archive_registry_receipts()
        }))
        .is_err()
        {
            self.inner.error(
                "<registry-export>",
                "actual MCP registry export panicked after owned cleanup observation".into(),
            );
        }
        let unfinished = self
            .inner
            .jobs
            .lock()
            .unwrap()
            .owned
            .keys()
            .cloned()
            .collect();
        McpShutdownReport {
            quiet: self.is_quiet(),
            unfinished,
            evidence: self.evidence(),
        }
    }
}
