//! The carrier boundary: what the app pushes to a client session, and who owns
//! the session keys.
//!
//! A carrier is a wire, not a protocol. The relay socket is one; a WebRTC
//! DataChannel is another. Everything above this line — the session key, the
//! encrypted frames, dispatch and teardown — is the same on either, so nothing
//! above it learns which wire is carrying.

use tokio::sync::mpsc;

use crate::transport::{self, Envelope, OuterFields};
use serde_json::Value;

/// One encrypted frame bound for one client session, before any wire wrapper
/// exists. The carrier that takes it decides how to frame it: the relay writer
/// wraps it as `{"type":"e2ee_envelope",…}`, a DataChannel sends the envelope
/// JSON directly.
#[derive(Debug, Clone)]
pub struct OutboundEnvelope {
    pub session_id: String,
    pub envelope: Envelope,
}

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl SessionSender {
    /// The session this sender targets.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn keyed(
        session_id: &str,
        session_key: String,
        out: mpsc::UnboundedSender<OutboundEnvelope>,
    ) -> Self {
        SessionSender {
            session_id: session_id.to_string(),
            session_key,
            out,
        }
    }

    /// A sender not bound to a live connection — for tests and request/response
    /// callers that never push. `push` succeeds-into-the-void.
    pub fn detached(session_id: impl Into<String>) -> Self {
        let (out, _rx) = mpsc::unbounded_channel();
        SessionSender {
            session_id: session_id.into(),
            session_key: String::new(),
            out,
        }
    }

    /// Test-only: a sender with a real session key and a captured channel, so
    /// tests can decrypt every pushed frame (`term.output`, `term.closed`, …)
    /// with [`decrypt_push`](Self::decrypt_push).
    #[cfg(test)]
    pub fn observable(
        session_id: impl Into<String>,
    ) -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>, String) {
        let (out, rx) = mpsc::unbounded_channel();
        let session_key = transport::generate_session_key();
        (
            SessionSender {
                session_id: session_id.into(),
                session_key: session_key.clone(),
                out,
            },
            rx,
            session_key,
        )
    }

    /// Test-only: decode one captured [`Self::observable`] envelope back to the
    /// pushed inner payload.
    #[cfg(test)]
    pub fn decrypt_push(session_key: &str, outbound: &OutboundEnvelope) -> Value {
        transport::decrypt_envelope(session_key, &outbound.envelope)
            .expect("push decrypts with the session key")
            .payload
    }

    /// Encrypt `payload` as an inner frame and hand it to the carrier. Returns
    /// false once the carrier is gone (so the app can drop the stale sender).
    pub fn push(&self, payload: Value) -> bool {
        let envelope = match transport::encrypt_frame(
            &self.session_key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: format!("session:{}", self.session_id),
            },
            &transport::FrameFields {
                frame_type: "data".into(),
                sender: "device".into(),
                payload,
                message_id: None,
                created_at: None,
            },
            None,
        ) {
            Ok(env) => env,
            Err(_) => return false,
        };
        self.out
            .send(OutboundEnvelope {
                session_id: self.session_id.clone(),
                envelope,
            })
            .is_ok()
    }
}

#[cfg(test)]
mod sender_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_push_hands_the_carrier_an_envelope_and_no_wire_wrapper() {
        let (sender, mut pushes, key) = SessionSender::observable("s-1");

        assert!(sender.push(json!({ "id": 7, "ok": true })));

        let outbound = pushes.try_recv().expect("the push arrived");
        assert_eq!(outbound.session_id, "s-1");
        assert_eq!(outbound.envelope.route_to, "session:s-1");
        assert_eq!(
            SessionSender::decrypt_push(&key, &outbound),
            json!({ "id": 7, "ok": true })
        );
    }

    #[test]
    fn a_push_to_a_gone_carrier_reports_failure() {
        let (sender, pushes, _key) = SessionSender::observable("s-gone");
        drop(pushes);

        assert!(!sender.push(json!({ "ok": true })));
    }
}
