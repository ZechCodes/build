//! The browser side of the carrier boundary, as a test builds it.
//!
//! One home for the frames a client sends the device and for the handler that
//! reports what arrived, so the crate's own tests and the integration tests
//! that drive a real relay socket state each shape once. Compiled for tests
//! only, never into the daemon.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{
    CarrierError, CarrierHandle, FrameHandler, FrameIntake, OutboundEnvelope, SessionSender,
};
use crate::transport::{self, Envelope, Frame, FrameFields, OuterFields, SessionInit};

/// The device a test's browser is talking to. Nothing checks it — the session
/// key proves who holds what — so one value serves every test.
pub const DEVICE_ID: &str = "dev-1";

/// What a browser sends to open a session: its fresh session key, wrapped to
/// the device's transport public key.
pub fn session_init(
    session_id: &str,
    transport_public_key: &str,
    session_key: &str,
) -> SessionInit {
    SessionInit {
        session_id: session_id.to_string(),
        device_id: DEVICE_ID.into(),
        wrapped_session_key: transport::wrap_session_key(transport_public_key, session_key)
            .expect("a browser can wrap to the device's transport key"),
    }
}

/// One encrypted frame from the browser, addressed to the device as every
/// client frame is.
pub fn client_request(
    session_key: &str,
    session_id: &str,
    frame_type: &str,
    payload: Value,
) -> Envelope {
    transport::encrypt_frame(
        session_key,
        &OuterFields {
            session_id: session_id.to_string(),
            route_to: format!("device:{DEVICE_ID}"),
        },
        &FrameFields {
            frame_type: frame_type.into(),
            sender: transport::SENDER_CLIENT.into(),
            payload,
            message_id: None,
            created_at: None,
        },
        None,
    )
    .expect("the client can encrypt to its own session key")
}

/// `handler`, with every frame it is given reported as
/// `<frame_type>:<session_id>` — the synthetic `close` a session gets when it
/// ends among them. Read the reports with [`within_patience`].
///
/// The one home of that shape: a test that only needs to see what arrived uses
/// [`reporting_handler`], and one that needs the device to answer too wraps its
/// own handler here.
pub fn reporting(handler: FrameHandler) -> (FrameHandler, mpsc::UnboundedReceiver<String>) {
    let (reported, reports) = mpsc::unbounded_channel();
    let watched = FrameHandler::new(
        Arc::clone(&handler.clock),
        move |sender: SessionSender, frame: Frame, timer| {
            let _ = reported.send(format!("{}:{}", frame.frame_type, frame.session_id));
            (handler.dispatch)(sender, frame, timer)
        },
    );
    (watched, reports)
}

/// [`reporting`] over a handler that answers and does nothing else.
pub fn reporting_handler() -> (FrameHandler, mpsc::UnboundedReceiver<String>) {
    reporting(FrameHandler::new(
        crate::timing::FrameClock::new(),
        |_sender, _frame, _timer| json!({ "ok": true }),
    ))
}

/// How long a test waits on the code under test before the run has hung rather
/// than failed. One home for that number, whatever channel is being read.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// The next value off a channel, or a failed test.
pub async fn within_patience<T>(next: impl Future<Output = Option<T>>) -> T {
    settled(next).await.expect("the channel is open")
}

/// Whether the channel being read closed rather than answering — the shape of
/// "the device took this wire down", which is a value a test waits for too.
pub async fn closed_within_patience<T>(next: impl Future<Output = Option<T>>) -> bool {
    settled(next).await.is_none()
}

async fn settled<T>(next: impl Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(PATIENCE, next)
        .await
        .expect("the code under test answered in time")
}

/// A DataChannel wire as a test drives it: the carrier the peer transport
/// opens for a negotiated channel (`rtc::DataChannelCarrier`), with the test
/// at the browser's end. The relay is not a data plane — a request over it is
/// refused before the dispatcher (`FrameIntake::accept`) — so a test that
/// needs the device's handler pool to be doing something puts the work in
/// through one of these, the way a browser's requests actually arrive.
pub struct ChannelWire {
    carrier: CarrierHandle,
    /// What the device pushed to this wire, as the channel's writer would
    /// take it: every reply and push for the sessions riding the wire.
    pub outbound: mpsc::UnboundedReceiver<OutboundEnvelope>,
}

impl ChannelWire {
    pub fn open() -> Self {
        let (carrier, outbound) = CarrierHandle::open_channel();
        ChannelWire { carrier, outbound }
    }

    /// A session opened over this wire, as a `session_init` arriving on a
    /// channel is.
    pub fn open_session(
        &self,
        intake: &FrameIntake,
        session_id: &str,
        init: &SessionInit,
    ) -> Result<(), CarrierError> {
        intake.open(session_id, init, &self.carrier)
    }

    /// One envelope arrived on this wire.
    pub async fn accept(
        &self,
        intake: &FrameIntake,
        envelope: Envelope,
    ) -> Result<(), CarrierError> {
        intake.accept(envelope, &self.carrier).await
    }

    /// The wire closed under whatever it was carrying.
    pub fn close(self, intake: &FrameIntake) {
        intake.close_carrier(&self.carrier);
    }
}
