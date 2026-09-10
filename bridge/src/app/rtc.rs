use crate::app::{require_array, require_str, require_value, AppState};
use crate::carrier::SessionSender;
use crate::rtc::{SessionPeerFactory, SessionPeers};
use crate::timing::FrameTimer;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Answer the browser's offer for this session (spec §Signaling), opening the
/// session's one peer connection if this is its first offer.
///
/// The ICE servers the browser fetched from the api ride with every offer and
/// with nothing else, so the bridge needs no Cloudflare credential of its own
/// and a restart carries fresh ones to the peer it already has.
pub(in crate::app) fn rtc_offer(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let sdp = require_str(params, "sdp")?;
    let ice_servers = require_array(params, "ice_servers")?;
    let peers = timer.lock(state).peers();
    #[cfg(test)]
    let gate = timer.lock(state).off_lock_gate.clone();
    #[cfg(test)]
    if let Some(gate) = gate {
        gate.arrive();
    }
    let answer = peers
        .offer(sender.session_id(), &sdp, &ice_servers, sender.clone())
        .map_err(|e| e.to_string())?;
    Ok(json!({ "sdp": answer }))
}

pub(in crate::app) fn rtc_ice(
    state: &Arc<Mutex<AppState>>,
    session_id: &str,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let candidate = require_value(params, "candidate")?;
    let peers = timer.lock(state).peers();
    peers
        .candidate(session_id, candidate)
        .map_err(|e| e.to_string())?;
    Ok(json!({}))
}

/// The browser gave up on the peer carrier: tear this session's peer down and
/// leave the session working over the relay.
pub(in crate::app) fn rtc_close(
    state: &Arc<Mutex<AppState>>,
    session_id: &str,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let peers = timer.lock(state).peers();
    peers.close(session_id).map_err(|e| e.to_string())?;
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
        self.peers = SessionPeers::with_factory(factory);
    }

    /// The sessions' peer connections, for a caller that must not hold the app
    /// mutex while a peer negotiates.
    pub(in crate::app) fn peers(&self) -> Arc<SessionPeers> {
        self.peers.clone()
    }
}
