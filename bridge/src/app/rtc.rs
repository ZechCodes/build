use crate::app::{require_array, require_str, require_value, AppState};
use crate::carrier::SessionSender;
use crate::rtc::{SessionPeerFactory, SessionPeers};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

/// Where a handler finds the sessions' peers without the app mutex.
///
/// Signaling is the one traffic that must be answered while the app is busy:
/// an ICE restart is the phone recovering from the bridge being slow, and it
/// gives the bridge fifteen seconds. On 2026-09-24 `rtc.offer` and `rtc.ice`
/// waited up to 32 s for the app mutex — held by an `tasks.list` — to do
/// nothing but clone the `Arc` that was behind it (task #128). So the `Arc`
/// lives here, behind a lock nobody holds for longer than the clone, and the
/// frame handler is handed this slot beside the state.
pub struct PeersSlot(RwLock<Arc<SessionPeers>>);

impl PeersSlot {
    pub(in crate::app) fn new(peers: Arc<SessionPeers>) -> Arc<PeersSlot> {
        Arc::new(PeersSlot(RwLock::new(peers)))
    }

    /// The peers as they are now.
    pub fn get(&self) -> Arc<SessionPeers> {
        Arc::clone(&self.0.read().unwrap())
    }

    fn set(&self, peers: Arc<SessionPeers>) {
        *self.0.write().unwrap() = peers;
    }
}

/// Answer the browser's offer for this session (spec §Signaling), opening the
/// session's one peer connection if this is its first offer.
///
/// The ICE servers the browser fetched from the api ride with every offer and
/// with nothing else, so the bridge needs no Cloudflare credential of its own
/// and a restart carries fresh ones to the peer it already has.
pub(in crate::app) fn rtc_offer(
    peers: &SessionPeers,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let sdp = require_str(params, "sdp")?;
    let ice_servers = require_array(params, "ice_servers")?;
    let answer = peers
        .offer(&sdp, &ice_servers, sender.clone())
        .map_err(|e| e.to_string())?;
    Ok(json!({ "sdp": answer }))
}

pub(in crate::app) fn rtc_ice(
    peers: &SessionPeers,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let candidate = require_value(params, "candidate")?;
    peers
        .candidate(sender, candidate)
        .map_err(|e| e.to_string())?;
    Ok(json!({}))
}

/// The browser gave up on the peer carrier: tear this session's peer down. The
/// session itself lives on — its rendezvous still carries the signaling of the
/// next offer — but it has no data path until one opens again (rule 1), which
/// is why a browser sends this only when it is done with the device or about
/// to renegotiate.
pub(in crate::app) fn rtc_close(
    peers: &SessionPeers,
    sender: &SessionSender,
) -> Result<Value, String> {
    peers.close(sender).map_err(|e| e.to_string())?;
    Ok(json!({}))
}

impl AppState {
    /// Answer `rtc.offer` with peer connections `factory` builds. Without one
    /// the bridge has no peer transport and every offer is refused.
    ///
    /// Settable after the state is shared because a real factory is built from
    /// the intake, the intake from this state's own handler: the peer transport
    /// is the last thing the daemon hands the app, not something it is born
    /// with.
    pub fn set_peer_factory(&mut self, factory: Arc<dyn SessionPeerFactory>) {
        self.set_peer_factory_on(factory, None);
    }

    /// [`set_peer_factory`](Self::set_peer_factory), with every peer's work
    /// driven on `driver` — the daemon's liveness runtime.
    pub fn set_peer_factory_on(
        &mut self,
        factory: Arc<dyn SessionPeerFactory>,
        driver: Option<tokio::runtime::Handle>,
    ) {
        self.peers
            .set(SessionPeers::with_factory_on(factory, driver));
    }

    /// The sessions' peer connections, for a caller that must not hold the app
    /// mutex while a peer negotiates.
    pub(in crate::app) fn peers(&self) -> Arc<SessionPeers> {
        self.peers.get()
    }

    /// The slot the frame handler reads the peers from, off the app mutex.
    pub(in crate::app) fn peers_slot(&self) -> Arc<PeersSlot> {
        Arc::clone(&self.peers)
    }
}
