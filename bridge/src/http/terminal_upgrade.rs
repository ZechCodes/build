//! HTTP/1 upgrade on the supervised HTTPS listener. The mounted route owns
//! authentication and binding admission. The listener owns the socket task.

use std::sync::{Arc, Mutex};

use axum::{
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version},
    response::{IntoResponse, Response},
    Json,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use futures_util::future::BoxFuture;
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio::sync::{oneshot, watch};
use tokio_tungstenite::{
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig},
    },
    WebSocketStream,
};

use crate::http::limits::{self, TerminalSocketBudget, TerminalSocketReservation};

pub const PATH: &str = "/v1/terminal";
pub const PROTOCOL: &str = "build.term.v1";
/// Buffered wire bytes, including frame headers. Keep half the allocation
/// budget free for geometric growth of Tungstenite's retained write buffer.
pub const MAX_BUFFERED_FRAME_BYTES: usize = limits::TERMINAL_CODEC_BYTES / 2;

pub type TerminalSocket = WebSocketStream<TokioIo<Upgraded>>;
type SocketHandler = Box<dyn for<'a> FnOnce(&'a mut TerminalSocket) -> BoxFuture<'a, ()> + Send>;

/// Validated upgrade syntax. The caller checks the HTTP policy and redeems
/// the ticket against the request's exact Origin before accepting the socket.
pub struct Handshake {
    accept_key: HeaderValue,
    ticket: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeRefusal {
    BadRequest,
    AuthorityMismatch,
    InvalidCredential,
    Overloaded,
    Unavailable,
}

impl IntoResponse for UpgradeRefusal {
    fn into_response(self) -> Response {
        let (status, code, retryable) = match self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "invalid_request", false),
            Self::AuthorityMismatch => {
                return (
                    StatusCode::MISDIRECTED_REQUEST,
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(serde_json::json!({
                        "error": "HTTP request policy refused",
                        "error_code": "authority_mismatch",
                        "retryable": false,
                        "request_id": uuid::Uuid::new_v4().to_string(),
                    })),
                )
                    .into_response();
            }
            Self::InvalidCredential => (StatusCode::UNAUTHORIZED, "invalid_credential", false),
            Self::Overloaded => (StatusCode::TOO_MANY_REQUESTS, "overloaded", true),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable", true),
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"error": code, "error_code": code, "retryable": retryable})),
        )
            .into_response()
    }
}

impl Handshake {
    pub fn parse(request: &Request) -> Result<Self, UpgradeRefusal> {
        use UpgradeRefusal::{AuthorityMismatch, BadRequest};

        if request.method() != Method::GET
            || request.version() != Version::HTTP_11
            || request.uri().path_and_query().map(|path| path.as_str()) != Some(PATH)
        {
            return Err(BadRequest);
        }
        let headers = request.headers();
        singleton(headers, header::HOST).ok_or(AuthorityMismatch)?;
        let upgrade = singleton(headers, header::UPGRADE).ok_or(BadRequest)?;
        let connection_upgrade = headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|token| trim_ows(token).eq_ignore_ascii_case("upgrade"));
        if !upgrade.eq_ignore_ascii_case("websocket")
            || !connection_upgrade
            || singleton(headers, header::SEC_WEBSOCKET_VERSION) != Some("13")
        {
            return Err(BadRequest);
        }
        let key = singleton(headers, header::SEC_WEBSOCKET_KEY).ok_or(BadRequest)?;
        if STANDARD.decode_slice(key, &mut [0; 16]) != Ok(16) {
            return Err(BadRequest);
        }
        let offer = singleton(headers, header::SEC_WEBSOCKET_PROTOCOL).ok_or(BadRequest)?;
        let ticket = parse_offer(offer)?;
        let accept_key = HeaderValue::from_str(&derive_accept_key(key.as_bytes()))
            .expect("WebSocket accept is base64");
        Ok(Self { accept_key, ticket })
    }

    pub fn ticket(&self) -> &[u8; 32] {
        &self.ticket
    }

    fn response(self) -> Response {
        (
            StatusCode::SWITCHING_PROTOCOLS,
            [
                (header::CONNECTION, HeaderValue::from_static("Upgrade")),
                (header::UPGRADE, HeaderValue::from_static("websocket")),
                (header::SEC_WEBSOCKET_ACCEPT, self.accept_key),
                (
                    header::SEC_WEBSOCKET_PROTOCOL,
                    HeaderValue::from_static(PROTOCOL),
                ),
            ],
        )
            .into_response()
    }
}

fn singleton(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn parse_offer(offer: &str) -> Result<[u8; 32], UpgradeRefusal> {
    use UpgradeRefusal::{BadRequest, InvalidCredential};

    let mut offered = offer.split(',').map(trim_ows);
    let ticket = match (offered.next(), offered.next(), offered.next()) {
        (Some(PROTOCOL), None, None) => return Err(InvalidCredential),
        (Some(PROTOCOL), Some(ticket), None) | (Some(ticket), Some(PROTOCOL), None) => {
            ticket.strip_prefix("ticket.").ok_or(BadRequest)?
        }
        _ => return Err(BadRequest),
    };
    let mut bytes = [0; 32];
    if URL_SAFE_NO_PAD.decode_slice(ticket, &mut bytes) != Ok(bytes.len()) {
        return Err(InvalidCredential);
    }
    Ok(bytes)
}

/// A request extension for ticket capacity checks and upgrade registration.
/// The listener drives the single registered job in its physical connection task.
#[derive(Clone)]
pub struct TerminalUpgrade {
    stop: watch::Receiver<bool>,
    sockets: TerminalSocketBudget,
    job: Arc<Mutex<Option<oneshot::Sender<UpgradeJob>>>>,
}

impl TerminalUpgrade {
    pub(super) fn new(
        sockets: TerminalSocketBudget,
        stop: watch::Receiver<bool>,
    ) -> (Self, oneshot::Receiver<UpgradeJob>) {
        let (send, receive) = oneshot::channel();
        (
            Self {
                stop,
                sockets,
                job: Arc::new(Mutex::new(Some(send))),
            },
            receive,
        )
    }

    /// Check at ticket issuance. Redemption must still reserve under the
    /// ticket/binding transaction's own gate.
    pub fn check_capacity(&self, family: &str) -> Result<(), UpgradeRefusal> {
        let stopped = self.stop.borrow();
        if *stopped {
            return Err(UpgradeRefusal::Unavailable);
        }
        self.sockets
            .has_capacity(family)
            .then_some(())
            .ok_or(UpgradeRefusal::Overloaded)
    }

    pub fn reserve(&self, family: &str) -> Result<TerminalSocketReservation, UpgradeRefusal> {
        let stopped = self.stop.borrow();
        if *stopped {
            return Err(UpgradeRefusal::Unavailable);
        }
        self.sockets
            .reserve(family)
            .map_err(|_| UpgradeRefusal::Overloaded)
    }

    /// Register a successfully admitted socket before replying 101. Cleanup
    /// captured by the handler drops on both failed and completed handoffs.
    pub fn accept(
        &self,
        request: &mut Request,
        handshake: Handshake,
        reservation: TerminalSocketReservation,
        handler: impl for<'a> FnOnce(&'a mut TerminalSocket) -> BoxFuture<'a, ()> + Send + 'static,
    ) -> Result<Response, UpgradeRefusal> {
        let stopped = self.stop.borrow();
        if *stopped {
            return Err(UpgradeRefusal::Unavailable);
        }
        let on_upgrade = request
            .extensions_mut()
            .remove::<OnUpgrade>()
            .ok_or(UpgradeRefusal::BadRequest)?;
        let mut slot = self.job.lock().unwrap();
        let sender = slot.take().ok_or(UpgradeRefusal::Unavailable)?;
        sender
            .send(UpgradeJob {
                on_upgrade,
                handler: Box::new(handler),
                reservation,
            })
            .map_err(|_| UpgradeRefusal::Unavailable)?;
        Ok(handshake.response())
    }

    #[cfg(test)]
    async fn stopped(&self) {
        let mut stop = self.stop.clone();
        wait_for_stop(&mut stop).await;
    }
}

pub(super) struct UpgradeJob {
    // If the job never runs, drop Hyper's receiver (which can own the I/O)
    // before returning terminal capacity.
    on_upgrade: OnUpgrade,
    handler: SocketHandler,
    reservation: TerminalSocketReservation,
}

impl UpgradeJob {
    pub(super) async fn run(self) {
        let _reservation = self.reservation;
        let Ok(io) = self.on_upgrade.await else {
            return;
        };
        let config = WebSocketConfig {
            // Send eagerly. Tungstenite keeps its staging allocation after
            // flush, so large messages must use bounded fragments.
            write_buffer_size: 0,
            max_write_buffer_size: MAX_BUFFERED_FRAME_BYTES,
            max_message_size: Some(limits::MAX_TERMINAL_CLIENT_BYTES),
            max_frame_size: Some(limits::MAX_TERMINAL_CLIENT_BYTES),
            accept_unmasked_frames: false,
            ..WebSocketConfig::default()
        };
        let mut socket =
            WebSocketStream::from_raw_socket(TokioIo::new(io), Role::Server, Some(config)).await;
        (self.handler)(&mut socket).await;
    }
}

pub(super) async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

#[cfg(test)]
#[path = "terminal_upgrade/tests.rs"]
mod tests;
