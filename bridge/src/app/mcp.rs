use crate::app::{apply_thread_action, AppState, DeferredJob, DeliveryRunner, SessionRegistry};
use crate::mcp::{BridgeAction, DoneReport};
use crate::store::now_rfc3339;
use crate::timing::{FrameClock, FrameTimer};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Who is on the other end of an authenticated MCP control frame.
///
/// The kind decides the tool surface, and it is read off the identity the frame
/// authenticated with — never off the frame's own claim about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum AddressedSession {
    /// An agent working a plan or a run.
    Coding { entity_id: String, agent_id: String },
    /// A router deciding where one capture goes.
    Router {
        capture_id: String,
        agent_id: String,
    },
}

/// What a frame off the daemon's control socket — an agent's `done` report or
/// one of its MCP actions — is timed under. It is not a relay method, so it
/// gets a name of its own rather than borrowing one from the wire.
pub(in crate::app) const MCP_CONTROL_METHOD: &str = "mcp.control";

/// Resolve a control frame only when it carries the current per-session
/// capability, returning the AGENT that sent it. The token is never included in
/// errors or logs.
///
/// `task_id` is the legacy spelling of the identity baked into the harness's
/// argv; that identity is the agent now, and the entity behind it is a lookup
/// away.
pub(in crate::app) fn authenticated_mcp_owner<'a>(
    frame: &'a Value,
    sessions: &SessionRegistry,
) -> Option<&'a str> {
    let owner = frame.get("task_id")?.as_str()?;
    let supplied = frame.get("session_token")?.as_str()?;
    sessions.token_matches(owner, supplied).then_some(owner)
}

#[cfg(unix)]
/// Run the git a socket line handed back — with the guard released, on a
/// blocking thread so several harnesses at once park no runtime worker — and
/// write it down under the same timer. The socket's twin of the drain in
/// [`dispatch_frame`], for a router's tool and a coding agent's report alike.
pub(in crate::app) async fn apply_off_the_socket(
    state: &Arc<Mutex<AppState>>,
    timer: &FrameTimer,
    deferred: DeferredJob,
) -> Result<Value, String> {
    let done = tokio::task::spawn_blocking(move || deferred.run())
        .await
        .expect("the lifecycle job panicked");
    timer
        .lock(state)
        .apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done)
}

pub(in crate::app) fn bind_done_listener(
    path: &std::path::Path,
) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    let listener = tokio::net::UnixListener::bind(path)?;
    // bind(2) applies the process umask, but a permissive or changed umask must
    // never make the lifecycle control plane available to other local users.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(unix)]
pub(in crate::app) async fn serve_done_listener(
    state: Arc<Mutex<AppState>>,
    listener: tokio::net::UnixListener,
) {
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let mut accept_backoff =
        crate::backoff::Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                accept_backoff.reset();
                tokio::spawn(handle_done_stream(
                    Arc::clone(&state),
                    stream,
                    Arc::clone(&clock),
                ));
            }
            Err(error) => {
                let wait = accept_backoff.current();
                eprintln!("done socket: accept error: {error}; retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                accept_backoff.increase();
            }
        }
    }
}

#[cfg(unix)]
pub(in crate::app) async fn handle_done_stream(
    state: Arc<Mutex<AppState>>,
    stream: tokio::net::UnixStream,
    clock: Arc<FrameClock>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timer = clock.frame(MCP_CONTROL_METHOD);
        if let Some(response) = handle_authenticated_mcp_frame(&state, &frame, &timer).await {
            let _ = write_half.write_all(response.to_string().as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            let _ = write_half.flush().await;
        }
    }
}

pub(in crate::app) async fn handle_authenticated_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    timer: &FrameTimer,
) -> Option<Value> {
    let addressed = {
        let app = timer.lock(state);
        authenticated_mcp_owner(frame, &app.session_registry)
            .map(str::to_string)
            .and_then(|agent_id| app.addressed_session(agent_id))
    };
    match addressed {
        Some(AddressedSession::Router { capture_id, .. }) => {
            handle_router_mcp_frame(state, frame, &capture_id, timer).await
        }
        Some(AddressedSession::Coding {
            entity_id,
            agent_id,
        }) => handle_coding_mcp_frame(state, frame, &entity_id, &agent_id, timer).await,
        None => Some(json!({ "ok": false, "error": "unauthorized MCP session" })),
    }
}

pub(in crate::app) async fn handle_router_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    capture_id: &str,
    timer: &FrameTimer,
) -> Option<Value> {
    if let Ok(report) =
        serde_json::from_value::<DoneReport>(frame.get("report").cloned().unwrap_or(Value::Null))
    {
        timer.lock(state).on_router_done(capture_id, report);
        return None;
    }
    let action = serde_json::from_value::<BridgeAction>(
        frame.get("request").cloned().unwrap_or(Value::Null),
    )
    .ok()?;
    let (answered, deferred) = timer.lock(state).router_deferring(capture_id, action);
    let result = match deferred {
        Some(deferred) => apply_off_the_socket(state, timer, deferred).await,
        None => answered,
    };
    DeliveryRunner::drain(state, timer);
    Some(mcp_action_response(result))
}

pub(in crate::app) async fn handle_coding_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    entity_id: &str,
    agent_id: &str,
    timer: &FrameTimer,
) -> Option<Value> {
    if let Ok(report) =
        serde_json::from_value::<DoneReport>(frame.get("report").cloned().unwrap_or(Value::Null))
    {
        let deferred = timer
            .lock(state)
            .done_deferring_for_agent(entity_id, agent_id, report);
        if let Some(deferred) = deferred {
            if let Err(error) = apply_off_the_socket(state, timer, deferred).await {
                eprintln!("done report {entity_id}: {error}");
            }
        }
        DeliveryRunner::drain(state, timer);
        return None;
    }
    let action = serde_json::from_value::<BridgeAction>(
        frame.get("request").cloned().unwrap_or(Value::Null),
    )
    .ok()?;
    let (answered, deferred) = timer
        .lock(state)
        .agent_action_deferring(entity_id, agent_id, action);
    let result = match deferred {
        Some(deferred) => apply_off_the_socket(state, timer, deferred).await,
        None => answered,
    };
    DeliveryRunner::drain(state, timer);
    Some(mcp_action_response(result))
}

pub(in crate::app) fn mcp_action_response(result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

impl AppState {
    /// Listen on the daemon control socket for `done` reports forwarded by the
    /// per-task `build-bridge mcp` servers, and route each to its task's `on_done`.
    pub fn spawn_done_socket(state: Arc<Mutex<AppState>>, path: String) {
        tokio::spawn(async move {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // A socket file that ANSWERS belongs to a running daemon: its
            // harnesses dial this path for every `done`, and unlinking it out
            // from under them silently breaks each one's report. A second
            // bridge (a dev stack launched from inside an agent session that
            // inherited BRIDGE_MCP_SOCKET, say) must refuse loudly instead of
            // stealing the control plane. Only a DEAD file — one nothing
            // accepts on — is stale debris to clear.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                eprintln!(
                    "done socket: {path} is already served by a live daemon; refusing to \
                     replace it. Set BRIDGE_MCP_SOCKET to a private path for this instance."
                );
                return;
            }
            let _ = std::fs::remove_file(&path);
            let listener = match bind_done_listener(std::path::Path::new(&path)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("done socket: bind {path} failed: {e}");
                    return;
                }
            };
            eprintln!("done socket: listening on {path}");
            serve_done_listener(state, listener).await;
        });
    }

    /// Route an agent's `done` to its owner's lifecycle transition, by owner
    /// lookup (plans map, then runs map), without draining: a report that
    /// carries an Issue's scheduler on to its next stage hands that git back
    /// HERE, to the socket that can release the guard before running it. The
    /// `done` twin of [`AppState::dispatch_deferring`].
    #[cfg(test)]
    pub(in crate::app) fn done_deferring(
        &mut self,
        entity_id: &str,
        report: DoneReport,
    ) -> Option<DeferredJob> {
        let agent_id = self
            .entity_agents(entity_id)
            .ok()
            .and_then(|agents| agents.primary())
            .map(|agent| agent.id.clone());
        self.done_deferring_for_resolved_agent(entity_id, agent_id.as_deref(), report)
    }

    fn done_deferring_for_resolved_agent(
        &mut self,
        entity_id: &str,
        agent_id: Option<&str>,
        report: DoneReport,
    ) -> Option<DeferredJob> {
        if self.plans.contains_key(entity_id) {
            self.on_plan_agent_done(entity_id, agent_id, report);
        } else if self.runs.contains_key(entity_id) {
            self.on_run_agent_done(entity_id, agent_id, report);
        } else {
            eprintln!("on_agent_done: unknown entity {entity_id}");
        }
        self.take_deferred()
    }

    /// Route a report while retaining the authenticated actor long enough to
    /// stop only that agent's execution clock.
    pub(in crate::app) fn done_deferring_for_agent(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        report: DoneReport,
    ) -> Option<DeferredJob> {
        self.record_agent_working_since(entity_id, agent_id, None);
        self.done_deferring_for_resolved_agent(entity_id, Some(agent_id), report)
    }

    /// Execute an MCP thread request against the conversation owner resolved
    /// from the agent identity baked into that session's MCP command. The
    /// authenticated agent resolves to its current plan or run; planned runs
    /// resolve unread/reply actions to the owning Issue (legacy plan id), while
    /// planless adopted runs retain their independent worktree conversation.
    #[cfg(test)]
    pub(in crate::app) fn on_mcp_action(
        &mut self,
        entity_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        let agent_id = self.entity_agents(entity_id)?.resolve(None)?.id.clone();
        self.on_agent_mcp_action(entity_id, &agent_id, action)
    }

    /// One agent's tool, without draining: the coding and project surfaces'
    /// twin of [`AppState::router_deferring`]. A project agent's write tool can
    /// hand back git the way a verb does — cutting a workspace is the same
    /// `workspace.create` the browser calls — and the socket is the one place
    /// that can run it with the guard released.
    pub(in crate::app) fn agent_action_deferring(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: BridgeAction,
    ) -> (Result<Value, String>, Option<DeferredJob>) {
        let queued_before = self.delivery_queue.checkpoint();
        let answered = self.on_agent_mcp_action(entity_id, agent_id, action);
        if answered.is_err() {
            self.drop_turns_queued_since(queued_before);
        }
        (answered, self.take_deferred())
    }

    /// One agent's tool, drained — for the tests that own the state directly
    /// and have no mutex to release. Running it is what the MCP control socket
    /// does with the guard released.
    #[cfg(test)]
    pub(in crate::app) fn agent_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        let (answered, deferred) = self.agent_action_deferring(entity_id, agent_id, action);
        match deferred {
            Some(deferred) => {
                let done = deferred.run();
                self.apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done)
            }
            None => answered,
        }
    }

    /// The same, for a caller that knows WHICH agent is speaking — every real
    /// one, since the MCP control plane authenticates an agent.
    pub(in crate::app) fn on_agent_mcp_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        // The surface a session is on is a property of the SESSION — its agent
        // id says which one — and the gate is here as well as in the tool
        // inventory each surface is shown: a harness that writes its own frames
        // must not reach past the surface it was opened on.
        let surface = crate::mcp::McpSurface::for_owner(agent_id);
        if !action.allowed_on(surface) {
            return Err(format!(
                "{} is a {} tool; this session is on the {} surface",
                action.tool_name(),
                action.surface_name(),
                surface.as_str()
            ));
        }
        // The project reads answer about the project this agent belongs to, and
        // the daemon holds the binding that says which one that is.
        if matches!(action, BridgeAction::ListWorkspaces) {
            return self.project_agent_workspaces(entity_id);
        }
        if let BridgeAction::ListWorkspaceAgents { workspace_id } = &action {
            return self.project_agent_workspace_agents(entity_id, workspace_id);
        }
        if let BridgeAction::CreateWorkspace { name, isolation } = &action {
            return self.project_agent_create_workspace(entity_id, name, isolation.as_deref());
        }
        if let BridgeAction::ReadOperationMessages { operation_id } = &action {
            return self.read_operation_messages_for_agent(entity_id, agent_id, operation_id);
        }
        if let BridgeAction::SearchConversation { query } = &action {
            return self.search_agent_conversations(entity_id, agent_id, query);
        }
        // The topic is the AGENT's, not the conversation's: two agents sharing
        // an Issue's thread each name their own work, and the record is where
        // the bubble reads it from.
        if let BridgeAction::SetTopic { topic } = &action {
            return self.set_agent_topic(entity_id, agent_id, topic);
        }
        if let BridgeAction::PostThreadMessage { links, .. } = &action {
            self.validate_thread_links_for_owner(entity_id, links)?;
        }
        let posted_still_working = match &action {
            BridgeAction::PostThreadMessage { still_working, .. } => Some(*still_working),
            _ => None,
        };
        let reads_unread = matches!(action, BridgeAction::ReadUnreadMessages);
        // Where an agent speaks — its own conversation, or its Issue's when it
        // is the implementation's first — is one rule, and it is
        // `edit_agent_conversation`'s.
        let now = now_rfc3339();
        let result = self.edit_agent_conversation(entity_id, agent_id, |thread, artifact| {
            apply_thread_action(thread, artifact, action, &now)
        });
        if let Ok(value) = &result {
            if reads_unread && value["working"].is_string() {
                self.start_agent_working(entity_id, agent_id, &now);
            }
            match posted_still_working {
                Some(true) => self.start_agent_working(entity_id, agent_id, &now),
                Some(false) => self.record_agent_working_since(entity_id, agent_id, None),
                None => {}
            }
            self.observe_conversation_working(entity_id, &now);
        }
        result
    }

    /// One agent's `done`, drained — the synchronous twin of
    /// [`AppState::done_deferring`], for the tests that own the state directly
    /// and have no guard to release. Running it is what the MCP control socket
    /// does with the guard released.
    #[cfg(test)]
    pub(in crate::app) fn on_agent_done(&mut self, entity_id: &str, report: DoneReport) {
        let agent_id = self
            .entity_agents(entity_id)
            .ok()
            .and_then(|agents| agents.primary())
            .map(|agent| agent.id.clone());
        let deferred = match agent_id {
            Some(agent_id) => self.done_deferring_for_agent(entity_id, &agent_id, report),
            None => self.done_deferring(entity_id, report),
        };
        if let Some(deferred) = deferred {
            let done = deferred.run();
            if let Err(error) = self.apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done) {
                eprintln!("on_agent_done {entity_id}: {error}");
            }
        }
    }
}
