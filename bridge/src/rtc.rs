//! The peer connection, as everything above it sees one: one answerer per E2EE
//! session, handed the browser's offer and its candidates, trickling its own
//! back.
//!
//! Signaling is the one thing pinned to the relay carrier (spec §Signaling).
//! `rtc.offer`, `rtc.ice` and `rtc.close` ride the carrier the client sent them
//! on and the bridge's candidates go back over that same carrier — never over
//! the channels they negotiate, which do not exist yet when they are needed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    #[error("the peer connection refused the offer: {0}")]
    Refused(String),
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

    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;

    /// Tear this peer connection down. Nothing to report: the session keeps
    /// working over the relay either way.
    async fn close(&self);
}

/// The only place a peer implementation is chosen and built.
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError>;
}

/// Which peer connection each E2EE session has, and when it stops being its.
///
/// **Boundary** the whole lifecycle behind three verbs the signaling arms call
/// and one a session end calls: a session's first offer opens its peer, every
/// later offer reconfigures that same one, and a peer stops being the
/// session's exactly once. **Hides** the map and its lock, the factory, the
/// open-once race, and that a peer's work is async at all — a handler runs on
/// a blocking thread and gets its answer back before it replies.
pub struct SessionPeers {
    factory: Arc<dyn SessionPeerFactory>,
    peers: Mutex<HashMap<String, Arc<dyn SessionPeer>>>,
}

impl SessionPeers {
    pub fn with_factory(factory: Arc<dyn SessionPeerFactory>) -> Arc<Self> {
        Arc::new(SessionPeers {
            factory,
            peers: Mutex::new(HashMap::new()),
        })
    }

    /// Answer this session's offer, opening its peer if this is the first one.
    ///
    /// A first offer the peer cannot answer leaves the session with no peer, so
    /// the browser's retry builds a fresh one rather than reaching the
    /// half-open peer that just failed; a failed ICE restart keeps the peer
    /// that is already carrying.
    pub fn offer(
        &self,
        session_id: &str,
        offer_sdp: &str,
        ice_servers: &[Value],
        signaling: SessionSender,
    ) -> Result<String, RtcError> {
        let (peer, opened_by_this_offer) = self.riding_or_opened(session_id)?;
        match awaited(peer.answer(offer_sdp, ice_servers, signaling)) {
            Ok(answer) => Ok(answer),
            Err(refused) => {
                if opened_by_this_offer {
                    if let Some(unusable) = self.take(session_id) {
                        awaited(unusable.close());
                    }
                }
                Err(refused)
            }
        }
    }

    /// Trickle one of the browser's candidates to the peer this session is
    /// negotiating over. A candidate for a session that never offered is
    /// refused, not answered by opening a peer nobody negotiated.
    pub fn candidate(&self, session_id: &str, candidate: Value) -> Result<(), RtcError> {
        let peer = self
            .peers
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RtcError::NoPeer(session_id.to_string()))?;
        awaited(peer.add_remote_candidate(candidate))
    }

    /// The browser gave up on the peer carrier: tear this session's peer down
    /// and leave the session working over the relay.
    pub fn close(&self, session_id: &str) -> Result<(), RtcError> {
        let peer = self
            .take(session_id)
            .ok_or_else(|| RtcError::NoPeer(session_id.to_string()))?;
        awaited(peer.close());
        Ok(())
    }

    /// The session ended, so its peer does: an ICE negotiation belongs to the
    /// session that offered it. The teardown is spawned, because this runs
    /// where the app releases everything else the session held and nothing
    /// there waits on a socket.
    pub fn end_session(&self, session_id: &str) {
        if let Some(peer) = self.take(session_id) {
            tokio::spawn(async move { peer.close().await });
        }
    }

    /// This session's peer and whether this call is the one that opened it.
    ///
    /// Building a peer is foreign work — a real one allocates an ICE agent and
    /// a DTLS transport — so it runs with no lock held, and two offers racing
    /// on one session still leave one peer: the one that reached the map first,
    /// the loser closed rather than left negotiating.
    fn riding_or_opened(&self, session_id: &str) -> Result<(Arc<dyn SessionPeer>, bool), RtcError> {
        if let Some(peer) = self.peers.lock().unwrap().get(session_id) {
            return Ok((peer.clone(), false));
        }
        let opened = self.factory.open(session_id)?;
        let won_the_race = {
            let mut peers = self.peers.lock().unwrap();
            match peers.get(session_id) {
                Some(peer) => Some(peer.clone()),
                None => {
                    peers.insert(session_id.to_string(), opened.clone());
                    None
                }
            }
        };
        match won_the_race {
            Some(peer) => {
                awaited(opened.close());
                Ok((peer, false))
            }
            None => Ok((opened, true)),
        }
    }

    /// Take this session's peer out — the one place a peer stops being the
    /// session's, so an answer still in flight cannot put a closed one back.
    fn take(&self, session_id: &str) -> Option<Arc<dyn SessionPeer>> {
        self.peers.lock().unwrap().remove(session_id)
    }

    /// Test-only: how many sessions hold a peer.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
}

/// Finish one peer-connection call where a frame handler can wait for it.
/// Handlers run on the runtime's blocking pool (`carrier::dispatch`), so the
/// peer's async work is finished here rather than outliving the reply the
/// client is waiting for — the one place that happens.
fn awaited<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Handle::current().block_on(work)
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        refuses_offers: bool,
        closing: Notify,
    }

    impl RecordingPeer {
        pub fn offers(&self) -> Vec<(String, Vec<Value>)> {
            self.record.lock().unwrap().offers.clone()
        }

        pub fn remote_candidates(&self) -> Vec<Value> {
            self.record.lock().unwrap().remote_candidates.clone()
        }

        pub fn is_closed(&self) -> bool {
            self.record.lock().unwrap().closed
        }

        /// Wait for this peer to be torn down, however far away the teardown
        /// was spawned — an order of events rather than a wall-clock bet.
        pub async fn closed(&self) {
            while !self.is_closed() {
                self.closing.notified().await;
            }
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
            if self.refuses_offers {
                return Err(RtcError::Refused(offer_sdp.to_string()));
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
            self.closing.notify_one();
        }
    }

    #[derive(Default)]
    pub struct RecordingPeerFactory {
        opened: Mutex<HashMap<String, Arc<RecordingPeer>>>,
        opens: AtomicUsize,
        gate: Mutex<Option<Arc<AnswerGate>>>,
        refuse_offers: AtomicBool,
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

        /// Make every peer opened from here on refuse the offers it is given —
        /// the malformed or unusable SDP a real peer connection reports.
        pub fn fail_answers(&self) {
            self.refuse_offers.store(true, Ordering::SeqCst);
        }

        pub fn peer_of(&self, session_id: &str) -> Option<Arc<RecordingPeer>> {
            self.opened.lock().unwrap().get(session_id).cloned()
        }

        /// How many peers were built, not how many sessions hold one: a second
        /// peer for one session is the regression these tests watch for.
        pub fn opened_count(&self) -> usize {
            self.opens.load(Ordering::SeqCst)
        }
    }

    impl SessionPeerFactory for RecordingPeerFactory {
        fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
            let peer = Arc::new(RecordingPeer {
                record: Mutex::new(PeerRecord::default()),
                gate: self.gate.lock().unwrap().clone(),
                refuses_offers: self.refuse_offers.load(Ordering::SeqCst),
                closing: Notify::new(),
            });
            self.opens.fetch_add(1, Ordering::SeqCst);
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
