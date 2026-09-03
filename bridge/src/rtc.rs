//! The peer connection, as everything above it sees one: one answerer per E2EE
//! session, handed the browser's offer and its candidates, trickling its own
//! back.
//!
//! Signaling is the one thing pinned to the relay carrier (spec §Signaling).
//! `rtc.offer`, `rtc.ice` and `rtc.close` ride the carrier the client sent them
//! on and the bridge's candidates go back over that same carrier — never over
//! the channels they negotiate, which do not exist yet when they are needed.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::carrier::SessionSender;

/// What a peer connection could not do. Never fatal to the session: a refused
/// offer leaves the client working over the relay carrier, which the spec's
/// upgrade policy already treats as the failure case.
#[derive(Debug, thiserror::Error)]
pub enum RtcError {
    #[error("no peer connection for session {0}")]
    NoPeer(String),
    #[error("this bridge has no peer transport")]
    Unavailable,
}

/// One live peer connection — one per E2EE session, always the answerer.
#[async_trait]
pub trait SessionPeer: Send + Sync {
    /// Answer the browser's offer.
    ///
    /// The ICE servers ride with every offer and nothing else carries them: the
    /// first call configures the peer, a later one reconfigures it and restarts
    /// ICE, which is how fresh TURN credentials arrive.
    ///
    /// `signaling` is the caller's own sender, passed with every offer and never
    /// owned from construction — it is the carrier the offer arrived on, so a
    /// candidate trickled back goes to the relay socket generation that is live
    /// now rather than the one that was live when the peer was built.
    async fn answer(
        &self,
        offer_sdp: &str,
        ice_servers: &[Value],
        signaling: SessionSender,
    ) -> Result<String, RtcError>;

    /// Take one trickled browser candidate.
    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;

    /// Tear this peer connection down. Nothing to report: the session keeps
    /// working over the relay either way.
    async fn close(&self);
}

/// The only place a peer implementation is chosen and built.
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError>;
}

/// A bridge with no peer transport built in. Every offer is refused, so the
/// client logs the failed upgrade and stays on the relay carrier.
pub struct NoPeerFactory;

impl SessionPeerFactory for NoPeerFactory {
    fn open(&self, _session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
        Err(RtcError::Unavailable)
    }
}

/// Send one of the bridge's own candidates to the browser — the one place the
/// shape of that push is written. False once the carrier it was given is gone.
pub fn trickle_candidate(signaling: &SessionSender, candidate: Value) -> bool {
    signaling.push(json!({ "type": "rtc.ice", "candidate": candidate }))
}

#[cfg(test)]
pub mod recording {
    //! The peer the signaling stage ships against: it negotiates nothing and
    //! records everything it was given, so a test can say what reached the peer
    //! and what the peer was allowed to push back.

    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio::sync::{Notify, Semaphore};

    /// Everything one recording peer was handed.
    #[derive(Default)]
    struct PeerRecord {
        offers: Vec<(String, Vec<Value>)>,
        remote_candidates: Vec<Value>,
        closed: bool,
        /// The sender of the latest offer — the peer keeps only that one.
        signaling: Option<SessionSender>,
    }

    /// Holds every answer until a test lets it go, so "a close arrived while the
    /// offer was still being answered" is an order of events rather than a bet
    /// on wall-clock time.
    pub struct AnswerGate {
        /// One permit per answer that has reached the gate, so a test that asks
        /// after the fact still hears about it.
        answering: Semaphore,
        release: Notify,
    }

    impl AnswerGate {
        fn new() -> Arc<Self> {
            Arc::new(AnswerGate {
                answering: Semaphore::new(0),
                release: Notify::new(),
            })
        }

        /// Wait until a peer is inside `answer` and held there.
        pub async fn wait_until_answering(&self) {
            self.answering
                .acquire()
                .await
                .expect("a peer is answering")
                .forget();
        }

        pub fn release(&self) {
            self.release.notify_waiters();
        }

        async fn hold(&self) {
            let waiting = self.release.notified();
            self.answering.add_permits(1);
            waiting.await;
        }
    }

    pub struct RecordingPeer {
        record: Mutex<PeerRecord>,
        gate: Option<Arc<AnswerGate>>,
    }

    impl RecordingPeer {
        /// The offers this peer answered, as `(sdp, ice_servers)`.
        pub fn offers(&self) -> Vec<(String, Vec<Value>)> {
            self.record.lock().unwrap().offers.clone()
        }

        pub fn remote_candidates(&self) -> Vec<Value> {
            self.record.lock().unwrap().remote_candidates.clone()
        }

        pub fn is_closed(&self) -> bool {
            self.record.lock().unwrap().closed
        }

        /// Trickle one of the bridge's candidates back over the carrier the
        /// latest offer arrived on.
        pub fn trickle(&self, candidate: Value) -> bool {
            let signaling = self.record.lock().unwrap().signaling.clone();
            match signaling {
                Some(signaling) => trickle_candidate(&signaling, candidate),
                None => false,
            }
        }
    }

    #[async_trait]
    impl SessionPeer for RecordingPeer {
        async fn answer(
            &self,
            offer_sdp: &str,
            ice_servers: &[Value],
            signaling: SessionSender,
        ) -> Result<String, RtcError> {
            {
                let mut record = self.record.lock().unwrap();
                record
                    .offers
                    .push((offer_sdp.to_string(), ice_servers.to_vec()));
                record.signaling = Some(signaling);
            }
            if let Some(gate) = &self.gate {
                gate.hold().await;
            }
            Ok(format!("answer-to:{offer_sdp}"))
        }

        async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError> {
            self.record
                .lock()
                .unwrap()
                .remote_candidates
                .push(candidate);
            Ok(())
        }

        async fn close(&self) {
            self.record.lock().unwrap().closed = true;
        }
    }

    #[derive(Default)]
    pub struct RecordingPeerFactory {
        opened: Mutex<HashMap<String, Arc<RecordingPeer>>>,
        gate: Mutex<Option<Arc<AnswerGate>>>,
    }

    impl RecordingPeerFactory {
        pub fn new() -> Arc<Self> {
            Arc::new(RecordingPeerFactory::default())
        }

        /// Make every answer from here on wait for the returned gate.
        pub fn hold_answers(&self) -> Arc<AnswerGate> {
            let gate = AnswerGate::new();
            *self.gate.lock().unwrap() = Some(gate.clone());
            gate
        }

        /// The peer this session was given, if it ever asked for one.
        pub fn peer_of(&self, session_id: &str) -> Option<Arc<RecordingPeer>> {
            self.opened.lock().unwrap().get(session_id).cloned()
        }

        pub fn opened_count(&self) -> usize {
            self.opened.lock().unwrap().len()
        }
    }

    impl SessionPeerFactory for RecordingPeerFactory {
        fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
            let peer = Arc::new(RecordingPeer {
                record: Mutex::new(PeerRecord::default()),
                gate: self.gate.lock().unwrap().clone(),
            });
            self.opened
                .lock()
                .unwrap()
                .insert(session_id.to_string(), peer.clone());
            Ok(peer)
        }
    }
}

#[cfg(test)]
mod trickle_tests {
    use super::*;

    #[test]
    fn a_bridge_candidate_reaches_the_client_as_an_rtc_ice_push() {
        let (signaling, mut pushes, key) = SessionSender::observable("s-trickle");

        assert!(trickle_candidate(
            &signaling,
            json!({ "candidate": "candidate:1 1 udp", "sdpMid": "0" })
        ));

        let pushed = SessionSender::decrypt_push(
            &key,
            &pushes.try_recv().expect("the candidate was pushed"),
        );
        assert_eq!(pushed["type"], "rtc.ice");
        assert_eq!(pushed["candidate"]["candidate"], "candidate:1 1 udp");
    }

    #[test]
    fn a_bridge_with_no_peer_transport_opens_nothing() {
        assert!(matches!(
            NoPeerFactory.open("s-1"),
            Err(RtcError::Unavailable)
        ));
    }
}
