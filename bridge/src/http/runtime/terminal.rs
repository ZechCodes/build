//! Terminal WebSocket v1 over the listener's supervised upgrade.

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    error::CapacityError, protocol::frame::coding::CloseCode, Error, Message,
};

use crate::{
    api::{self, ApiError},
    execution::{
        session::{
            RequestContext, SessionError, TerminalInterest, TerminalSnapshotLane, VerifiedCaller,
        },
        Call, ExecutionService, Invocation,
    },
    http::{
        listener::{
            terminal_upgrade::{Handshake, TerminalSocket, TerminalUpgrade, UpgradeRefusal},
            OutputAuthority,
        },
        policy,
    },
};

use super::{
    terminal_tickets::{self, TerminalAdmission},
    HttpResult, HttpState,
};
use sink::{CallSink, Outbound, SocketSink};
use wire::{Control, Incoming, Method};
use writer::WriterFence;

mod interests;
mod sink;
mod wire;
mod writer;

pub(super) use crate::http::listener::terminal_upgrade::PATH;

const MAX_CALLS: usize = 8;

pub(super) async fn upgrade(State(state): State<HttpState>, request: Request) -> Response {
    admit(state, request).unwrap_or_else(|error| *error)
}

fn admit(state: HttpState, mut request: Request) -> HttpResult<Response> {
    // Host is guarded upstream. Strict Origin precedes handshake syntax and the
    // ticket transaction; only this checked origin enters the redemption.
    let origin = policy::terminal_origin(&request)?;
    let handshake = Handshake::parse(&request).map_err(refusal)?;
    let upgrade = extension::<TerminalUpgrade>(&request)?;
    let output = extension::<OutputAuthority>(&request)?;
    let (sink, receive) = SocketSink::new();
    let admission = Arc::new(terminal_tickets::redeem(
        &state,
        handshake.ticket(),
        origin,
        sink.clone(),
        |family| upgrade.reserve(family).map_err(refusal),
    )?);
    let (stop_snapshots, stopped) = watch::channel(false);
    let snapshots = TerminalSnapshotLane {
        opening: admission.session().opening().terminal_snapshot_slot(),
        bridge: state.execution.sessions().terminal_snapshot_pool(),
        runtime: tokio::runtime::Handle::current(),
        stopped,
    };
    let writer = WriterFence::default();
    let connection = Connection {
        admission: admission.clone(),
        execution: state.execution.clone(),
        sink,
        writer: writer.clone(),
        receive,
        calls: Calls::default(),
        snapshots,
        stop_snapshots,
    };
    let response = upgrade
        .accept(
            &mut request,
            handshake,
            admission.reservation(),
            move |socket| Box::pin(connection.run(socket)),
        )
        .map_err(refusal)?;
    super::observe_output(&state, &output);
    writer.install(output, admission);
    Ok(response)
}

fn extension<T: Clone + Send + Sync + 'static>(request: &Request) -> HttpResult<T> {
    request
        .extensions()
        .get::<T>()
        .cloned()
        .ok_or_else(|| refusal(UpgradeRefusal::Unavailable))
}

fn refusal(refusal: UpgradeRefusal) -> Box<Response> {
    Box::new(refusal.into_response())
}

struct Connection {
    admission: Arc<TerminalAdmission>,
    execution: Arc<ExecutionService>,
    sink: Arc<SocketSink>,
    writer: WriterFence,
    receive: mpsc::Receiver<Outbound>,
    calls: Calls,
    snapshots: TerminalSnapshotLane,
    stop_snapshots: watch::Sender<bool>,
}

enum End {
    Drop,
    Close(CloseCode),
}

enum Io {
    Write(Outbound),
    Read(Option<Result<Message, Error>>),
}

async fn next_io(socket: &mut TerminalSocket, receive: &mut mpsc::Receiver<Outbound>) -> Io {
    // A continuous producer cannot prevent the peer from sending input or
    // releasing an attachment. Each complete outbound message still flushes
    // before the writer accepts another.
    tokio::select! {
        Some(outbound) = receive.recv() => Io::Write(outbound),
        message = socket.next() => Io::Read(message),
    }
}

impl Connection {
    async fn run(mut self, socket: &mut TerminalSocket) {
        let admission = self.admission.clone();
        tokio::select! {
            biased;
            _ = admission.ended() => {},
            _ = self.drive(socket) => {},
        }
    }

    async fn drive(&mut self, socket: &mut TerminalSocket) {
        let end = self.service(socket).await;
        self.calls = Calls::default();
        if let End::Close(code) = end {
            if self.admission.is_live() {
                writer::close(socket, code).await;
            }
        }
    }

    async fn service(&mut self, socket: &mut TerminalSocket) -> End {
        loop {
            tokio::select! {
                biased;
                code = self.sink.failed() => return End::Close(code),
                Some(ready) = self.calls.attaching.next(), if !self.calls.attaching.is_empty() => {
                    if let Err(end) = self.dispatch_attach(socket, ready).await {
                        return end;
                    }
                }
                Some(settled) = self.calls.settling.next(), if !self.calls.settling.is_empty() => {
                    self.settle(settled);
                }
                io = next_io(socket, &mut self.receive) => match io {
                    Io::Write(outbound) => {
                        if let Err(end) = self.deliver(socket, outbound).await {
                            return end;
                        }
                    }
                    Io::Read(message) => {
                        if let Err(end) = self.receive_message(socket, message).await {
                            return end;
                        }
                    }
                }
            }
        }
    }

    async fn deliver(
        &mut self,
        socket: &mut TerminalSocket,
        outbound: Outbound,
    ) -> Result<(), End> {
        let id = outbound.id.clone();
        if let Some(cursor) = self
            .writer
            .write(socket, outbound)
            .await
            .map_err(|_| End::Drop)?
        {
            self.sink.record_delivery(cursor);
        }
        if let Some(id) = id {
            self.calls.flushed(&id);
        }
        Ok(())
    }

    async fn receive_message(
        &mut self,
        socket: &mut TerminalSocket,
        message: Option<Result<Message, Error>>,
    ) -> Result<(), End> {
        match message {
            Some(Ok(Message::Binary(bytes))) => {
                match Incoming::parse(&bytes).map_err(End::Close)? {
                    Incoming::Call(request) => self.request(socket, request).await,
                    Incoming::Control(Control::Detach { attach_id }) => self.detach(&attach_id),
                }
            }
            Some(Ok(Message::Text(_))) => Err(End::Close(CloseCode::Unsupported)),
            Some(Ok(Message::Ping(_))) => socket.flush().await.map_err(|_| End::Drop),
            Some(Ok(Message::Pong(_))) => Ok(()),
            Some(Ok(Message::Close(_))) => {
                let _ = socket.flush().await;
                Err(End::Drop)
            }
            Some(Err(Error::Capacity(CapacityError::MessageTooLong { .. }))) => {
                Err(End::Close(CloseCode::Size))
            }
            Some(Err(Error::Protocol(_)) | Ok(Message::Frame(_))) => {
                Err(End::Close(CloseCode::Protocol))
            }
            Some(Err(Error::Utf8)) => Err(End::Close(CloseCode::Invalid)),
            _ => Err(End::Drop),
        }
    }

    async fn request(
        &mut self,
        socket: &mut TerminalSocket,
        request: wire::Request,
    ) -> Result<(), End> {
        let id: Arc<str> = request.id.as_str().into();
        let held = self.sink.interests.lock().unwrap().contains(&id);
        if held || self.calls.entries.contains_key(&id) {
            return Err(End::Close(CloseCode::Protocol));
        }
        if self.calls.entries.len() == MAX_CALLS {
            return reply_error(socket, &id, busy()).await;
        }
        let (method, invocation) = match request.decode() {
            Ok(call) => call,
            Err(error) => return reply_error(socket, &id, error).await,
        };
        let (request, reply, interest) = match self.prepare(&id, method, &invocation) {
            Ok(prepared) => prepared,
            Err(error) => return reply_error(socket, &id, error).await,
        };
        if let Some(interest) = interest.filter(|_| method.is_attach()) {
            let lane = self.snapshots.clone();
            self.calls.entries.insert(id.clone(), CallState::default());
            self.calls.attaching.push(Box::pin(async move {
                let outcome = tokio::select! {
                    biased;
                    slot = lane.reserve(&interest) => match slot {
                        Some(reservation) if request.is_live() => {
                            reply.reserve_snapshot(reservation);
                            AttachOutcome::Ready
                        }
                        Some(_) => AttachOutcome::Expired,
                        None => AttachOutcome::Cancelled,
                    },
                    _ = request.cancelled() => {
                        if interest.is_live() { AttachOutcome::Expired } else { AttachOutcome::Cancelled }
                    }
                };
                ReadyAttach {
                    id,
                    method,
                    outcome,
                    request,
                    invocation,
                }
            }));
            return Ok(());
        }
        self.submit(socket, id, method, request, invocation).await
    }

    async fn dispatch_attach(
        &mut self,
        socket: &mut TerminalSocket,
        ready: ReadyAttach,
    ) -> Result<(), End> {
        match ready.outcome {
            AttachOutcome::Ready => {
                self.submit(
                    socket,
                    ready.id,
                    ready.method,
                    ready.request,
                    ready.invocation,
                )
                .await
            }
            AttachOutcome::Cancelled => {
                self.calls.entries.remove(&ready.id);
                Ok(())
            }
            AttachOutcome::Expired => {
                self.calls.entries.remove(&ready.id);
                self.release(&ready.id);
                reply_error(socket, &ready.id, ended()).await
            }
        }
    }

    async fn submit(
        &mut self,
        socket: &mut TerminalSocket,
        id: Arc<str>,
        method: Method,
        request: RequestContext,
        invocation: Invocation,
    ) -> Result<(), End> {
        match self
            .execution
            .submit(request, invocation)
            .await
            .map_err(session_error)
        {
            Ok(call) => {
                self.calls.track(id, method, call);
                Ok(())
            }
            Err(error) => {
                self.calls.entries.remove(&id);
                if method.is_attach() {
                    self.release(&id);
                }
                reply_error(socket, &id, error).await
            }
        }
    }

    fn prepare(
        &self,
        id: &Arc<str>,
        method: Method,
        invocation: &Invocation,
    ) -> Result<PreparedCall, ApiError> {
        let (caller, interest) = self
            .admission
            .session()
            .opening()
            .while_open(|| self.capture(id, method, invocation))
            .ok_or_else(ended)??;
        let reply = self.sink.reply(id.clone(), method);
        let request = self.execution.sessions().request_terminal(
            self.admission.binding(),
            caller,
            reply.clone(),
            interest.clone(),
        );
        match request {
            Ok(request) => Ok((request, reply, interest)),
            Err(error) => {
                if method.is_attach() {
                    self.release(id);
                }
                Err(session_error(error))
            }
        }
    }

    fn capture(
        &self,
        id: &str,
        method: Method,
        invocation: &Invocation,
    ) -> Result<(VerifiedCaller, Option<Arc<TerminalInterest>>), ApiError> {
        let mut caller = self.admission.capture_caller().ok_or_else(ended)?;
        let mut interests = self.sink.interests.lock().unwrap();
        let term_id = invocation.params()["term_id"].as_str();
        let interest = if method.is_attach() {
            Some(
                interests
                    .begin(id.into(), method, Some(self.snapshots.clone()))
                    .ok_or_else(|| ApiError::invalid_params("Terminal attachment limit reached"))?,
            )
        } else if method.requires_attachment() {
            let interest = term_id
                .and_then(|term_id| interests.for_effect(term_id))
                .ok_or_else(|| ApiError::invalid_params("Terminal is not attached"))?;
            if method == Method::Ack {
                let cursor = invocation.params()["cursor"]
                    .as_u64()
                    .expect("validated cursor");
                if !interests.permits_ack(&interest, cursor) {
                    return Err(ApiError::invalid_params(
                        "Terminal cursor exceeds delivered output",
                    ));
                }
            }
            caller = caller.for_terminal_effect(&interest);
            Some(interest)
        } else {
            None
        };
        Ok((caller, interest))
    }

    fn release(&self, id: &str) -> bool {
        let removed = self
            .admission
            .session()
            .opening()
            .while_open(|| {
                let mut interests = self.sink.interests.lock().unwrap();
                interests.remove(id).then(|| interests.is_empty())
            })
            .flatten();
        if removed.is_some() {
            self.execution.reap_expired();
        }
        removed.unwrap_or(false)
    }

    fn detach(&self, id: &str) -> Result<(), End> {
        if self.release(id) {
            return Err(End::Close(CloseCode::Normal));
        }
        Ok(())
    }

    fn settle(&mut self, settled: Settled) {
        if settled.replied {
            self.calls.settled(&settled.id, settled.call);
        } else {
            self.calls.entries.remove(&settled.id);
            if settled.method.is_attach() {
                self.release(&settled.id);
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop_snapshots.send_replace(true);
        self.admission.retire();
        self.execution.reap_expired();
    }
}

async fn reply_error(socket: &mut TerminalSocket, id: &str, error: ApiError) -> Result<(), End> {
    let bytes = refusal_bytes(id, error);
    if bytes.len() > sink::MAX_ORDINARY {
        return Err(End::Close(CloseCode::Size));
    }
    writer::write_message(socket, &bytes)
        .await
        .map_err(|_| End::Drop)
}

fn refusal_bytes(id: &str, error: ApiError) -> Vec<u8> {
    let mut reply = api::reply(Value::String(id.into()), Err(error));
    reply["bridge_epoch"] = Value::String(crate::bridge_epoch::current().into());
    serde_json::to_vec(&reply).expect("terminal refusal serializes")
}

fn session_error(error: SessionError) -> ApiError {
    match error {
        SessionError::Full => busy(),
        _ => ended(),
    }
}

fn busy() -> ApiError {
    ApiError::busy("Terminal request limit reached")
}

fn ended() -> ApiError {
    ApiError::unavailable("Terminal authorization has ended")
}

type PreparedCall = (RequestContext, Arc<CallSink>, Option<Arc<TerminalInterest>>);

struct ReadyAttach {
    id: Arc<str>,
    method: Method,
    outcome: AttachOutcome,
    request: RequestContext,
    invocation: Invocation,
}

enum AttachOutcome {
    Ready,
    Cancelled,
    Expired,
}

// A completion and its queued response can arrive in either order. Retain the
// Call (and its exact authorization and shared request permit) through both.
#[derive(Default)]
struct Calls {
    entries: HashMap<Arc<str>, CallState>,
    attaching: FuturesUnordered<BoxFuture<'static, ReadyAttach>>,
    settling: FuturesUnordered<BoxFuture<'static, Settled>>,
}

#[derive(Default)]
struct CallState {
    flushed: bool,
    settled: Option<Call>,
}

struct Settled {
    id: Arc<str>,
    method: Method,
    replied: bool,
    call: Call,
}

impl Calls {
    fn track(&mut self, id: Arc<str>, method: Method, mut call: Call) {
        self.entries.insert(id.clone(), CallState::default());
        self.settling.push(Box::pin(async move {
            let replied = call.reply().await.is_ok();
            Settled {
                id,
                method,
                replied,
                call,
            }
        }));
    }

    fn flushed(&mut self, id: &Arc<str>) {
        // A canceled reply may still be queued when the peer reuses its ID.
        // Only the response carrying this allocation can settle the new call.
        if !self
            .entries
            .get_key_value(id)
            .is_some_and(|(key, _)| Arc::ptr_eq(key, id))
        {
            return;
        }
        let Some(state) = self.entries.get_mut(id) else {
            return;
        };
        if state.settled.is_some() {
            self.entries.remove(id);
        } else {
            state.flushed = true;
        }
    }

    fn settled(&mut self, id: &str, call: Call) {
        let Some(state) = self.entries.get_mut(id) else {
            return;
        };
        if state.flushed {
            self.entries.remove(id);
        } else {
            state.settled = Some(call);
        }
    }
}

#[cfg(test)]
mod tests;
