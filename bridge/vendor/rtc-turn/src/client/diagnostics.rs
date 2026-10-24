//! Bounded, credential-free diagnostics. Endpoint hashes are keyed per opening.
use super::transaction::TransactionType;
use std::collections::{HashMap, hash_map::RandomState};
use std::hash::{BuildHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use stun::error_code::ErrorCodeAttribute;
use stun::message::*;
const MAX_DIAGNOSTIC_PEERS: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PeerKey {
    Known(SocketAddr),
    Overflow,
}
static NEXT_ALLOCATION: AtomicU64 = AtomicU64::new(1);

/// Shared by every TURN allocation within one peer opening.
#[derive(Clone)]
pub struct DiagnosticContext {
    opening: String,
    generation: Arc<AtomicU64>,
    key: RandomState,
}
impl DiagnosticContext {
    /// `opening` must be a process-local diagnostic ordinal or token, never a credential or address.
    pub fn new(opening: String, generation: Arc<AtomicU64>) -> Self {
        Self::with_key(opening, generation, RandomState::new())
    }
    /// Reuses an opening's key across peer-connection recreation.
    pub fn with_key(opening: String, generation: Arc<AtomicU64>, key: RandomState) -> Self {
        Self {
            opening,
            generation,
            key,
        }
    }
}
#[derive(Default)]
struct Peer {
    id: u64,
    ordinal: usize,
    counts: [u64; 4],
    dirty: bool,
    last_summary: u64,
}
struct Folded {
    repeats: u64,
    tid: TransactionId,
    channel: Option<u16>,
}
pub(super) struct AllocationDiagnostics {
    context: DiagnosticContext,
    allocation: u64,
    server_id: u64,
    server_family: &'static str,
    relay_family: &'static str,
    peers: HashMap<SocketAddr, Peer>,
    overflow: Peer,
    overflow_peer_observations: u64,
    overflow_events: u64,
    window: Instant,
    detail_window: Instant,
    summary_round: u64,
    #[cfg(test)]
    identity_hashes: usize,
    emitted: usize,
    suppressed: u64,
    pending: HashMap<TransactionId, (&'static str, Vec<Option<PeerKey>>, Option<u16>, u64)>,
    folded: HashMap<(u64, Option<PeerKey>, &'static str, &'static str, u16), Folded>,
}
impl AllocationDiagnostics {
    pub(super) fn new(context: DiagnosticContext) -> Self {
        Self {
            context,
            allocation: NEXT_ALLOCATION.fetch_add(1, Ordering::Relaxed),
            server_id: 0,
            server_family: "unknown",
            relay_family: "unknown",
            peers: HashMap::new(),
            overflow: Peer {
                ordinal: MAX_DIAGNOSTIC_PEERS + 1,
                ..Default::default()
            },
            overflow_peer_observations: 0,
            overflow_events: 0,
            window: Instant::now(),
            detail_window: Instant::now(),
            summary_round: 0,
            #[cfg(test)]
            identity_hashes: 0,
            emitted: 0,
            suppressed: 0,
            pending: HashMap::new(),
            folded: HashMap::new(),
        }
    }
    pub(super) fn server(&mut self, server: Option<SocketAddr>) {
        self.server_family = server.map_or("none", family);
        if let Some(server) = server {
            let mut hash = self.context.key.build_hasher();
            server.hash(&mut hash);
            self.server_id = hash.finish();
        }
    }
    pub(super) fn relay(&mut self, relay: SocketAddr) {
        self.relay_family = family(relay);
    }
    fn peer_state(&mut self, addr: SocketAddr) -> (PeerKey, &mut Peer) {
        use std::collections::hash_map::Entry;
        let ordinal = self.peers.len() + 1;
        match self.peers.entry(addr) {
            Entry::Occupied(peer) => (PeerKey::Known(addr), peer.into_mut()),
            Entry::Vacant(peer) if ordinal <= MAX_DIAGNOSTIC_PEERS => {
                #[cfg(test)]
                {
                    self.identity_hashes += 1;
                }
                let mut hash = self.context.key.build_hasher();
                addr.hash(&mut hash);
                (
                    PeerKey::Known(addr),
                    peer.insert(Peer {
                        id: hash.finish(),
                        ordinal,
                        ..Default::default()
                    }),
                )
            }
            Entry::Vacant(_) => {
                self.overflow_peer_observations += 1;
                (PeerKey::Overflow, &mut self.overflow)
            }
        }
    }
    #[cfg(test)]
    fn peer(&mut self, addr: SocketAddr) -> (u64, usize) {
        let (_, peer) = self.peer_state(addr);
        (peer.id, peer.ordinal)
    }
    fn state(&self, key: PeerKey) -> &Peer {
        match key {
            PeerKey::Known(addr) => &self.peers[&addr],
            PeerKey::Overflow => &self.overflow,
        }
    }
    pub(super) fn flush(&mut self, now: Instant, force: bool) {
        if !force && now.saturating_duration_since(self.window) < Duration::from_secs(1) {
            return;
        }
        let mut keys: Vec<_> = self.folded.keys().cloned().collect();
        keys.sort_by_key(|key| {
            (
                if matches!(key.3, "error" | "timeout" | "local-error") {
                    0
                } else {
                    1
                },
                key.1.map_or(0, |peer| self.state(peer).ordinal),
            )
        });
        if !force {
            keys.truncate(32);
        }
        for key in keys {
            let folded = self.folded.remove(&key).unwrap();
            let (generation, peer, method, outcome, code) = key;
            self.record(
                &method,
                folded.tid,
                peer,
                folded.channel,
                "folded",
                code,
                generation,
                folded.repeats,
                &outcome,
            );
        }
        let mut dirty: Vec<_> = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.dirty)
            .map(|(addr, peer)| (PeerKey::Known(*addr), peer.last_summary, peer.ordinal))
            .collect();
        if self.overflow.dirty {
            dirty.push((
                PeerKey::Overflow,
                self.overflow.last_summary,
                self.overflow.ordinal,
            ));
        }
        dirty.sort_by_key(|(_, last_summary, ordinal)| (*last_summary, *ordinal));
        if !force {
            dirty.truncate(32);
        }
        self.summary_round += 1;
        for (peer, _, _) in dirty {
            self.record(
                "DataSummary",
                TransactionId::default(),
                Some(peer),
                None,
                "summary",
                0,
                self.context.generation.load(Ordering::Relaxed),
                0,
                "none",
            );
            let peer = match peer {
                PeerKey::Known(addr) => self.peers.get_mut(&addr).unwrap(),
                PeerKey::Overflow => &mut self.overflow,
            };
            peer.dirty = false;
            peer.last_summary = self.summary_round;
        }
        self.window = now;
    }
    pub(super) fn poll_timeout(&self) -> Option<Instant> {
        (!self.folded.is_empty()
            || self.overflow.dirty
            || self.peers.values().any(|peer| peer.dirty))
        .then(|| self.window + Duration::from_secs(1))
    }
    fn admit(&mut self, now: Instant) -> bool {
        // Only timeout/close paths flush summaries: never put a summary batch in
        // front of the control-plane row which triggered this admission check.
        if now.saturating_duration_since(self.detail_window) >= Duration::from_secs(1) {
            self.detail_window = now;
            self.emitted = 0;
            self.suppressed = 0;
        }
        if self.emitted >= 32 {
            self.suppressed += 1;
            false
        } else {
            self.emitted += 1;
            true
        }
    }
    fn record(
        &self,
        method: &str,
        tid: TransactionId,
        peer: Option<PeerKey>,
        channel: Option<u16>,
        outcome: &str,
        code: u16,
        generation: u64,
        repeats: u64,
        folded_outcome: &str,
    ) {
        let state = peer.map(|key| self.state(key));
        let scope = match peer {
            Some(PeerKey::Known(_)) => "tracked",
            Some(PeerKey::Overflow) => "overflow",
            None => "none",
        };
        let peer_family = match peer {
            Some(PeerKey::Known(addr)) => family(addr),
            Some(PeerKey::Overflow) => "mixed",
            None => "none",
        };
        let counts = state.map_or([0; 4], |p| p.counts);
        log::info!(target: "turn_diagnostics", "turn opening={} generation={} allocation={} server={:x} peer={:x} ordinal={} peer_scope={} family={} server_family={} relay_family={} method={} tid={:?} channel={:?} outcome={} code={} reason={} send_indication={} send_channel={} receive_indication={} receive_channel={} repeats={} folded_outcome={} overflow_peer_observations={} overflow_events={}", self.context.opening, generation, self.allocation, self.server_id, state.map_or(0, |p| p.id), state.map_or(0, |p| p.ordinal), scope, peer_family, self.server_family, self.relay_family, method, tid, channel, outcome, code, reason(code), counts[0], counts[1], counts[2], counts[3], repeats, folded_outcome, self.overflow_peer_observations, self.overflow_events);
    }
    fn emit(
        &mut self,
        method: &'static str,
        tid: TransactionId,
        peer: Option<PeerKey>,
        channel: Option<u16>,
        outcome: &'static str,
        code: u16,
        generation: u64,
    ) {
        if peer == Some(PeerKey::Overflow) {
            self.overflow_events += 1;
        }
        if !self.admit(Instant::now()) {
            let folded = self
                .folded
                .entry((generation, peer, method, outcome, code))
                .or_insert(Folded {
                    repeats: 0,
                    tid,
                    channel,
                });
            folded.repeats += 1;
            folded.tid = tid;
            folded.channel = channel;
            return;
        }
        self.record(
            method, tid, peer, channel, outcome, code, generation, 1, "none",
        );
    }
    pub(super) fn request(&mut self, msg: &Message, transaction: &TransactionType) {
        let peers: Vec<_> = request_peers(msg, transaction)
            .into_iter()
            .map(|peer| peer.map(|addr| self.peer_state(addr).0))
            .collect();
        let mut channel = crate::proto::channum::ChannelNumber::default();
        let channel = channel.get_from(msg).ok().map(|_| channel.0);
        let method = method_name(msg.typ.method);
        let generation = self.context.generation.load(Ordering::Relaxed);
        for peer in &peers {
            self.emit(
                method,
                msg.transaction_id,
                *peer,
                channel,
                "request",
                0,
                generation,
            );
        }
        self.pending
            .insert(msg.transaction_id, (method, peers, channel, generation));
    }
    pub(super) fn response(&mut self, msg: &Message) {
        if let Some((method, peers, channel, generation)) = self.pending.remove(&msg.transaction_id)
        {
            let mut code = ErrorCodeAttribute::default();
            let code = code.get_from(msg).ok().map_or(0, |_| code.code.0);
            for peer in peers {
                self.emit(
                    &method,
                    msg.transaction_id,
                    peer,
                    channel,
                    if msg.typ.class == CLASS_ERROR_RESPONSE {
                        "error"
                    } else {
                        "success"
                    },
                    code,
                    generation,
                );
            }
        }
    }
    pub(super) fn timeout(&mut self, tid: TransactionId) {
        if let Some((method, peers, channel, generation)) = self.pending.remove(&tid) {
            for peer in peers {
                self.emit(method, tid, peer, channel, "timeout", 0, generation);
            }
        }
    }
    pub(super) fn send_failure(&mut self, peer: SocketAddr) {
        let peer = self.peer_state(peer).0;
        self.emit(
            "Send",
            TransactionId::default(),
            Some(peer),
            None,
            "local-error",
            0,
            self.context.generation.load(Ordering::Relaxed),
        );
    }
    pub(super) fn data(
        &mut self,
        peer: SocketAddr,
        channel: Option<u16>,
        incoming: bool,
        tid: TransactionId,
    ) {
        let index = usize::from(incoming) * 2 + usize::from(channel.is_some());
        let (peer, state) = self.peer_state(peer);
        state.counts[index] += 1;
        let first_use = state.counts[index] == 1;
        state.dirty |= !first_use;
        if peer == PeerKey::Overflow {
            self.overflow_events += 1;
        }
        if !first_use {
            return;
        }
        // A direction's first use is evidence; subsequent packets only update counters.
        // Packet traffic never consumes the control-plane detail budget.
        self.record(
            if channel.is_some() {
                "ChannelData"
            } else if incoming {
                "Data"
            } else {
                "Send"
            },
            tid,
            Some(peer),
            channel,
            if incoming {
                "received"
            } else {
                "queued-no-ack"
            },
            0,
            self.context.generation.load(Ordering::Relaxed),
            1,
            "none",
        );
    }
}
fn method_name(method: Method) -> &'static str {
    match method {
        METHOD_BINDING => "Binding",
        METHOD_ALLOCATE => "Allocate",
        METHOD_REFRESH => "Refresh",
        METHOD_CREATE_PERMISSION => "CreatePermission",
        METHOD_CHANNEL_BIND => "ChannelBind",
        METHOD_SEND => "Send",
        METHOD_DATA => "Data",
        _ => "Unknown",
    }
}
// Decode each XOR-PEER-ADDRESS independently using the original transaction ID.
// This is observation only: protocol state and permission refresh batching are unchanged.
fn request_peers(msg: &Message, transaction: &TransactionType) -> Vec<Option<SocketAddr>> {
    if let TransactionType::CreatePermissionRequest(_, _) = transaction {
        let mut peers = Vec::new();
        for attr in &msg.attributes.0 {
            if attr.typ != stun::attributes::ATTR_XOR_PEER_ADDRESS {
                continue;
            }
            let single = Message {
                transaction_id: msg.transaction_id,
                attributes: stun::attributes::Attributes(vec![attr.clone()]),
                ..Default::default()
            };
            let mut peer = crate::proto::peeraddr::PeerAddress::default();
            if peer.get_from(&single).is_ok() {
                peers.push(Some(SocketAddr::new(peer.ip, peer.port)));
            }
        }
        if !peers.is_empty() {
            return peers;
        }
    }
    vec![match transaction {
        TransactionType::CreatePermissionRequest(_, peer) => *peer,
        TransactionType::ChannelBindRequest(_, peer) => Some(*peer),
        _ => None,
    }]
}
impl Drop for AllocationDiagnostics {
    fn drop(&mut self) {
        self.flush(Instant::now(), true);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_identity_survives_allocations_and_ordinals_survive_storms() {
        let context = DiagnosticContext::new("opening".into(), Arc::new(AtomicU64::new(1)));
        let mut first = AllocationDiagnostics::new(context.clone());
        let mut second = AllocationDiagnostics::new(context);
        let peer = "192.0.2.1:1234".parse().unwrap();
        let identity = first.peer(peer);
        for n in 1..300 {
            first.peer(SocketAddr::from(([192, 0, 2, 2], n)));
        }
        assert_eq!(first.peer(peer), identity);
        assert_eq!(second.peer(peer).0, identity.0);
        assert_ne!(first.allocation, second.allocation);
    }
    #[test]
    fn storm_is_bounded_and_fold_count_retained() {
        let context = DiagnosticContext::new("opening".into(), Arc::new(AtomicU64::new(1)));
        let mut allocation = AllocationDiagnostics::new(context);
        let now = Instant::now();
        assert_eq!((0..1000).filter(|_| allocation.admit(now)).count(), 32);
        assert_eq!(allocation.suppressed, 968);
        assert!(allocation.admit(now + Duration::from_secs(1)));
        assert_eq!(allocation.suppressed, 0);
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        0 => "none",
        400 => "bad-request",
        401 => "unauthorized",
        403 => "forbidden",
        437 => "allocation-mismatch",
        438 => "stale-nonce",
        441 => "wrong-credentials",
        442 => "unsupported-transport",
        443 => "peer-family-mismatch",
        486 => "allocation-quota",
        508 => "insufficient-capacity",
        _ => "other",
    }
}

#[cfg(test)]
mod local_mock_tests {
    use super::*;
    use crate::client::{Client, ClientConfig, Event, relay::RelayState};
    use sansio::Protocol;
    use shared::TransportProtocol;
    use std::sync::{Mutex, Once};
    use stun::attributes::ATTR_NONCE;
    use stun::error_code::{ErrorCode, ErrorCodeAttribute};
    use stun::integrity::MessageIntegrity;
    use stun::textattrs::Nonce;

    struct Capture(Mutex<Vec<(std::thread::ThreadId, String)>>);
    static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));
    static INIT: Once = Once::new();
    impl log::Log for Capture {
        fn enabled(&self, record: &log::Metadata<'_>) -> bool {
            record.target() == "turn_diagnostics"
        }
        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                self.0
                    .lock()
                    .unwrap()
                    .push((std::thread::current().id(), record.args().to_string()));
            }
        }
        fn flush(&self) {}
    }
    fn logs() -> String {
        CAPTURE
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| *id == std::thread::current().id())
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn client() -> (Client, SocketAddr) {
        INIT.call_once(|| {
            log::set_logger(&CAPTURE).unwrap();
            log::set_max_level(log::LevelFilter::Info);
        });
        let mut client = Client::new(ClientConfig {
            stun_serv_addr: String::new(),
            turn_serv_addr: "127.0.0.1:3478".into(),
            local_addr: "127.0.0.1:40000".parse().unwrap(),
            transport_protocol: TransportProtocol::UDP,
            username: "mock-username-private".into(),
            password: "mock-password-private".into(),
            realm: "mock-realm-private".into(),
            software: String::new(),
            rto_in_ms: 1,
        })
        .unwrap();
        client.set_diagnostic_context(DiagnosticContext::new(
            "mock-opening".into(),
            Arc::new(AtomicU64::new(7)),
        ));
        let relay = "127.0.0.1:45000".parse().unwrap();
        client.relays.insert(
            relay,
            RelayState::new(
                relay,
                MessageIntegrity::new_short_term_integrity("mock-password-private".into()),
                Nonce::new(ATTR_NONCE, "mock-nonce-private".into()),
                Duration::from_secs(600),
            ),
        );
        (client, relay)
    }
    // An in-process sans-I/O TURN mock, with no sockets, DNS or credentials.
    fn respond(client: &mut Client, request: Message, reject: bool) {
        let mut response = Message::new();
        let mut attrs: Vec<Box<dyn Setter>> = vec![
            Box::new(request.transaction_id),
            Box::new(MessageType::new(
                request.typ.method,
                if reject {
                    CLASS_ERROR_RESPONSE
                } else {
                    CLASS_SUCCESS_RESPONSE
                },
            )),
        ];
        if reject {
            attrs.push(Box::new(ErrorCodeAttribute {
                code: ErrorCode(400),
                reason: b"mock-password-private".to_vec(),
            }));
        }
        response.build(&attrs).unwrap();
        client.handle_stun_message(&response.raw).unwrap();
    }
    fn next_request(client: &mut Client) -> Message {
        loop {
            let packet = client.poll_write().unwrap();
            let mut msg = Message::new();
            msg.raw = packet.message.to_vec();
            msg.decode().unwrap();
            if msg.typ.class == CLASS_REQUEST {
                return msg;
            }
        }
    }
    #[test]
    fn diagnostics_eleventh_refused_bind_uses_indications_across_ice_ticks() {
        let (mut client, relay) = client();
        let refused_peer = SocketAddr::from(([192, 0, 2, 1], 50011));
        let mut refused_bind_requests = 0;
        for port in 50001..50012 {
            let peer = SocketAddr::from(([192, 0, 2, 1], port));
            client
                .relay(relay)
                .unwrap()
                .create_permission(peer)
                .unwrap();
            let request = next_request(&mut client);
            respond(&mut client, request, false);
            client.relay(relay).unwrap().send_to(b"test", peer).unwrap();
            let request = next_request(&mut client);
            assert_eq!(request.typ.method, METHOD_CHANNEL_BIND);
            if peer == refused_peer {
                refused_bind_requests += 1;
            }
            respond(&mut client, request, peer == refused_peer);
            // Drain the indication sent while this bind was pending.
            while client.poll_write().is_some() {}
        }
        let start = Instant::now();
        let mut indications = 0;
        for tick in 1..=5 {
            client
                .handle_timeout(start + Duration::from_millis(tick * 200))
                .unwrap();
            client
                .relay(relay)
                .unwrap()
                .send_to(b"test", refused_peer)
                .unwrap();
            while let Some(packet) = client.poll_write() {
                let mut message = Message::new();
                message.raw = packet.message.to_vec();
                message.decode().unwrap();
                if message.typ.method == METHOD_CHANNEL_BIND {
                    refused_bind_requests += 1;
                } else {
                    assert_eq!(message.typ, MessageType::new(METHOD_SEND, CLASS_INDICATION));
                    indications += 1;
                }
            }
        }
        assert_eq!(refused_bind_requests, 1);
        assert_eq!(indications, 5);
    }

    #[test]
    fn diagnostics_authentication_channel_bind_errors_can_retry() {
        for code in [401, 438] {
            let (mut client, relay) = client();
            let peer = SocketAddr::from(([192, 0, 2, 1], 50001));
            client
                .relay(relay)
                .unwrap()
                .create_permission(peer)
                .unwrap();
            let request = next_request(&mut client);
            respond(&mut client, request, false);
            client.relay(relay).unwrap().send_to(b"test", peer).unwrap();
            let request = next_request(&mut client);
            let mut response = Message::new();
            response
                .build(&[
                    Box::new(request.transaction_id),
                    Box::new(MessageType::new(METHOD_CHANNEL_BIND, CLASS_ERROR_RESPONSE)),
                    Box::new(ErrorCodeAttribute {
                        code: ErrorCode(code),
                        reason: vec![],
                    }),
                    Box::new(Nonce::new(ATTR_NONCE, "replacement-nonce".into())),
                ])
                .unwrap();
            client.handle_stun_message(&response.raw).unwrap();
            while client.poll_write().is_some() {}
            client.relay(relay).unwrap().send_to(b"test", peer).unwrap();
            let retry = next_request(&mut client);
            assert_eq!(retry.typ.method, METHOD_CHANNEL_BIND);
            if code == 438 {
                assert_eq!(
                    Nonce::get_from_as(&retry, ATTR_NONCE).unwrap().text,
                    "replacement-nonce"
                );
            }
        }
    }

    #[test]
    fn diagnostics_local_mock_rejects_eleventh_channel_bind_without_secrets() {
        let (mut client, relay) = client();
        for port in 50001..50012 {
            let peer = SocketAddr::from(([192, 0, 2, 1], port));
            client
                .relay(relay)
                .unwrap()
                .create_permission(peer)
                .unwrap();
            let request = next_request(&mut client);
            assert_eq!(request.typ.method, METHOD_CREATE_PERMISSION);
            respond(&mut client, request, false);
            client
                .relay(relay)
                .unwrap()
                .send_to(b"mock-payload-private", peer)
                .unwrap();
            let request = next_request(&mut client);
            assert_eq!(request.typ.method, METHOD_CHANNEL_BIND);
            respond(&mut client, request, port == 50011);
        }
        // Early peers continue using ChannelData while the eleventh is refused.
        let first = SocketAddr::from(([192, 0, 2, 1], 50001));
        client
            .relay(relay)
            .unwrap()
            .send_to(b"test", first)
            .unwrap();
        let mut channel = crate::proto::chandata::ChannelData {
            number: crate::proto::channum::ChannelNumber(16384),
            data: b"test".to_vec(),
            ..Default::default()
        };
        channel.encode();
        client.handle_channel_data(&channel.raw).unwrap();
        client
            .diagnostics
            .as_mut()
            .unwrap()
            .flush(Instant::now(), true);
        let logs = logs();
        assert!(logs.contains("send_channel=1"), "{logs}");
        assert!(logs.contains("receive_channel=1"), "{logs}");
        assert!(logs.contains("method=ChannelBind"), "{logs}");
        assert!(logs.contains("ordinal=11"), "{logs}");
        assert!(
            logs.lines().any(|line| line.contains("ordinal=11")
                && line.contains("code=400 reason=bad-request")
                && line.contains("folded_outcome=error")),
            "{logs}"
        );
        assert!(logs.contains("method=Send"), "{logs}");
        assert!(logs.contains("send_indication=1"), "{logs}");
        assert!(logs.contains("server_family=ipv4"), "{logs}");
        for secret in [
            "mock-password-private",
            "mock-username-private",
            "mock-realm-private",
            "mock-nonce-private",
            "mock-payload-private",
            "192.0.2.1",
        ] {
            assert!(!logs.contains(secret), "leaked {secret}");
        }
    }
    #[test]
    fn diagnostics_close_preserves_more_than_32_folded_peer_outcomes() {
        let (mut client, _) = client();
        for port in 50000..50100 {
            for _ in 0..2 {
                client.diagnostics.as_mut().unwrap().data(
                    SocketAddr::from(([192, 0, 2, 4], port)),
                    None,
                    false,
                    TransactionId::default(),
                );
            }
        }
        drop(client);
        let logs = logs();
        for ordinal in 33..101 {
            assert!(
                logs.lines()
                    .any(|line| line.contains(&format!("ordinal={ordinal} "))
                        && line.contains("outcome=summary")),
                "missing {ordinal}"
            );
        }
    }
    #[test]
    fn diagnostics_batched_permission_refresh_names_each_peer_on_error_and_timeout() {
        let (mut client, relay) = client();
        let mut request = Message::new();
        request
            .build(&[
                Box::new(TransactionId::new()),
                Box::new(MessageType::new(METHOD_CREATE_PERMISSION, CLASS_REQUEST)),
                Box::new(crate::proto::peeraddr::PeerAddress {
                    ip: "192.0.2.3".parse().unwrap(),
                    port: 50000,
                }),
                Box::new(crate::proto::peeraddr::PeerAddress {
                    ip: "192.0.2.4".parse().unwrap(),
                    port: 50001,
                }),
            ])
            .unwrap();
        let diag = client.diagnostics.as_mut().unwrap();
        diag.request(
            &request,
            &TransactionType::CreatePermissionRequest(relay, None),
        );
        let mut error = Message::new();
        error
            .build(&[
                Box::new(request.transaction_id),
                Box::new(MessageType::new(
                    METHOD_CREATE_PERMISSION,
                    CLASS_ERROR_RESPONSE,
                )),
                Box::new(ErrorCodeAttribute {
                    code: ErrorCode(400),
                    reason: b"private".to_vec(),
                }),
            ])
            .unwrap();
        diag.response(&error);
        request.transaction_id = TransactionId::new();
        diag.request(
            &request,
            &TransactionType::CreatePermissionRequest(relay, None),
        );
        diag.timeout(request.transaction_id);
        let logs = logs();
        for ordinal in 1..3 {
            for outcome in ["request", "error", "timeout"] {
                assert!(
                    logs.lines()
                        .any(|line| line.contains(&format!("ordinal={ordinal} "))
                            && line.contains(&format!("outcome={outcome}"))),
                    "{logs}"
                );
            }
        }
    }
    #[test]
    fn diagnostics_overflow_keeps_channel_bind_error_method_and_code() {
        let (mut client, relay) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        for port in 10000..10256 {
            diag.peer(SocketAddr::from(([192, 0, 2, 8], port)));
        }
        let peer = "192.0.2.9:20000".parse().unwrap();
        let mut request = Message::new();
        request
            .build(&[
                Box::new(TransactionId::new()),
                Box::new(MessageType::new(METHOD_CHANNEL_BIND, CLASS_REQUEST)),
            ])
            .unwrap();
        diag.request(&request, &TransactionType::ChannelBindRequest(relay, peer));
        let mut error = Message::new();
        error
            .build(&[
                Box::new(request.transaction_id),
                Box::new(MessageType::new(METHOD_CHANNEL_BIND, CLASS_ERROR_RESPONSE)),
                Box::new(ErrorCodeAttribute {
                    code: ErrorCode(400),
                    reason: b"private".to_vec(),
                }),
            ])
            .unwrap();
        diag.response(&error);
        assert!(
            logs()
                .lines()
                .any(|line| line.contains("peer_scope=overflow")
                    && line.contains("method=ChannelBind")
                    && line.contains("outcome=error code=400 reason=bad-request"))
        );
        assert_eq!(diag.peers.len(), 256);
        assert!(!logs().contains("192.0.2.9"));
    }
    #[test]
    fn diagnostics_caps_peer_identity_and_normalizes_overflow_fold_keys() {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let first = SocketAddr::from(([192, 0, 2, 7], 10000));
        let identity = diag.peer(first);
        for port in 10001..10256 {
            diag.peer(SocketAddr::from(([192, 0, 2, 7], port)));
        }
        diag.emitted = 32;
        for port in 20000..21000 {
            let peer = SocketAddr::from(([192, 0, 2, 7], port));
            diag.send_failure(peer);
            diag.data(peer, None, false, TransactionId::default());
        }
        assert_eq!(
            diag.peers.len(),
            256,
            "unknown peers must not grow retained identity state"
        );
        assert_eq!(diag.identity_hashes, 256);
        assert_eq!(
            diag.folded.len(),
            1,
            "overflow control rows must share a key, never retain raw endpoints"
        );
        assert_eq!(
            diag.peer(first),
            identity,
            "admitted ordinals must never be evicted"
        );
        diag.flush(Instant::now(), true);
        let logs = logs();
        assert!(
            logs.lines().any(|line| line.contains("peer_scope=overflow")
                && line.contains("ordinal=257 ")
                && line.contains("outcome=folded")
                && line.contains("folded_outcome=local-error")
                && line.contains("repeats=1000")),
            "{logs}"
        );
        assert!(
            logs.lines().any(|line| line.contains("peer_scope=overflow")
                && line.contains("outcome=summary")
                && line.contains("send_indication=1000")
                && line.contains("overflow_peer_observations=2000")
                && line.contains("overflow_events=2000")),
            "{logs}"
        );
    }
    #[test]
    fn diagnostics_repeated_packets_compute_identity_only_on_first_insert() {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let peer = "192.0.2.94:50001".parse().unwrap();
        for _ in 0..1000 {
            diag.data(peer, None, false, TransactionId::default());
        }
        assert_eq!(diag.identity_hashes, 1);
        assert_eq!(diag.peers.len(), 1);
        for _ in 0..100 {
            diag.send_failure(peer);
        }
        let key = diag.folded.keys().next().unwrap();
        assert_eq!(std::any::type_name_of_val(&key.2), "&str");
        assert_eq!(std::any::type_name_of_val(&key.3), "&str");
        assert_eq!(
            diag.identity_hashes, 1,
            "control rows must reuse the identity too"
        );
    }
    #[test]
    fn diagnostics_control_never_flushes_packet_summaries_inline() {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let peer = "192.0.2.95:50001".parse().unwrap();
        for _ in 0..2 {
            diag.data(peer, None, false, TransactionId::default());
        }
        diag.window = Instant::now() - Duration::from_secs(2);
        diag.send_failure(peer);
        let rows = logs();
        assert_eq!(
            rows.lines().count(),
            2,
            "a due packet summary must not precede control"
        );
        assert!(rows.lines().last().unwrap().contains("outcome=local-error"));
        diag.flush(Instant::now(), false);
        assert!(logs().lines().last().unwrap().contains("outcome=summary"));
    }
    #[test]
    fn diagnostics_periodic_summaries_fairly_visit_continuously_dirty_peers() {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        for port in 50000..50033 {
            for _ in 0..2 {
                diag.data(
                    SocketAddr::from(([192, 0, 2, 95], port)),
                    None,
                    false,
                    TransactionId::default(),
                );
            }
        }
        let now = Instant::now();
        diag.flush(now + Duration::from_secs(1), false);
        for port in 50000..50032 {
            diag.data(
                SocketAddr::from(([192, 0, 2, 95], port)),
                None,
                false,
                TransactionId::default(),
            );
        }
        diag.flush(now + Duration::from_secs(2), false);
        assert!(
            logs()
                .lines()
                .any(|line| line.contains("ordinal=33 ") && line.contains("outcome=summary")),
            "the last dirty peer must not starve behind active early peers"
        );
    }
    #[test]
    fn diagnostics_packet_storm_counts_silently_and_leaves_control_budget_free() {
        let (mut client, relay) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let peer = "192.0.2.98:50001".parse().unwrap();
        for _ in 0..1000 {
            for (channel, incoming) in [
                (None, false),
                (Some(16384), false),
                (None, true),
                (Some(16384), true),
            ] {
                diag.data(peer, channel, incoming, TransactionId::default());
            }
        }
        assert_eq!(
            logs().lines().count(),
            4,
            "only each direction's first use is immediate"
        );
        assert_eq!(
            diag.emitted, 0,
            "packet counts must leave the control budget untouched"
        );
        let mut request = Message::new();
        request
            .build(&[
                Box::new(TransactionId::new()),
                Box::new(MessageType::new(METHOD_CHANNEL_BIND, CLASS_REQUEST)),
            ])
            .unwrap();
        diag.request(&request, &TransactionType::ChannelBindRequest(relay, peer));
        diag.timeout(request.transaction_id);
        assert!(logs().lines().any(|line| line.contains("method=ChannelBind") && line.contains("outcome=timeout")), "control must be immediate after packet traffic");
        diag.flush(Instant::now(), true);
        assert!(logs().lines().any(|line| line.contains("outcome=summary") && line.contains("send_indication=1000 send_channel=1000 receive_indication=1000 receive_channel=1000")));
    }
    #[test]
    fn diagnostics_send_failure_has_a_peer_and_safe_outcome() {
        let (mut client, relay) = client();
        let peer = "192.0.2.98:50001".parse().unwrap();
        assert!(
            client
                .relay(relay)
                .unwrap()
                .send_to(b"private payload", peer)
                .is_err()
        );
        assert!(logs().lines().any(|line| line.contains("method=Send")
            && line.contains("outcome=local-error")
            && line.contains("ordinal=1")));
    }
    fn mixed_direction_first_use_preserves_pending_summary(terminal: bool) {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let peer = "192.0.2.96:50000".parse().unwrap();
        // Direction A has unsummarized repeated traffic when direction B starts.
        diag.data(peer, None, false, TransactionId::default());
        diag.data(peer, None, false, TransactionId::default());
        diag.data(peer, Some(16384), true, TransactionId::default());
        let deadline = diag.window + Duration::from_secs(1);
        if terminal {
            drop(client);
        } else {
            client.handle_timeout(deadline).unwrap();
        }
        assert!(
            logs()
                .lines()
                .any(|line| line.contains("method=DataSummary")
                    && line.contains("outcome=summary")
                    && line.contains("send_indication=2")
                    && line.contains("receive_channel=1")),
            "a new direction must preserve earlier unsummarized traffic"
        );
    }
    #[test]
    fn diagnostics_mixed_direction_keeps_periodic_summary_pending() {
        mixed_direction_first_use_preserves_pending_summary(false);
    }
    #[test]
    fn diagnostics_mixed_direction_keeps_terminal_summary_pending() {
        mixed_direction_first_use_preserves_pending_summary(true);
    }
    #[test]
    fn diagnostics_idle_storm_gets_a_summary_timer() {
        let (mut client, _) = client();
        let peer = "192.0.2.98:50000".parse().unwrap();
        for _ in 0..1000 {
            client
                .diagnostics
                .as_mut()
                .unwrap()
                .data(peer, None, false, TransactionId::default());
        }
        let deadline = client.poll_timeout().unwrap();
        assert!(deadline <= Instant::now() + Duration::from_secs(1));
        client.handle_timeout(deadline).unwrap();
        assert!(
            logs()
                .lines()
                .any(|line| line.contains("outcome=summary")
                    && line.contains("send_indication=1000"))
        );
    }
    #[test]
    fn diagnostics_tail_storm_flushes_per_peer_counts_on_drop() {
        let (mut client, _) = client();
        let diag = client.diagnostics.as_mut().unwrap();
        let peer = "192.0.2.99:50000".parse().unwrap();
        for _ in 0..1000 {
            diag.data(peer, None, false, TransactionId::default());
        }
        drop(client);
        let logs = logs();
        assert!(
            logs.lines().any(|line| line.contains("outcome=summary")
                && line.contains("ordinal=1")
                && line.contains("method=DataSummary")
                && line.contains("send_indication=1000")),
            "{logs}"
        );
        assert!(logs.lines().count() <= 34);
    }
    #[test]
    fn diagnostics_local_mock_timeout_names_original_channel_bind_and_peer() {
        let (mut client, relay) = client();
        let peer = "192.0.2.2:50000".parse().unwrap();
        client
            .relay(relay)
            .unwrap()
            .create_permission(peer)
            .unwrap();
        let request = next_request(&mut client);
        respond(&mut client, request, false);
        client.relay(relay).unwrap().send_to(b"test", peer).unwrap();
        let request = next_request(&mut client);
        // Advance virtual time only. No network activity or real waiting.
        client
            .diagnostics
            .as_ref()
            .unwrap()
            .context
            .generation
            .store(8, Ordering::Relaxed);
        let start = Instant::now();
        for n in 1..10 {
            client
                .handle_timeout(start + Duration::from_secs(n * 2))
                .unwrap();
        }
        while let Some(event) = client.poll_event() {
            if let Event::TransactionTimeout(tid) = event {
                assert_eq!(tid, request.transaction_id);
            }
        }
        let logs = logs();
        assert!(
            logs.lines().any(|line| line.contains("method=ChannelBind")
                && line.contains("outcome=timeout")
                && line.contains("ordinal=1")
                && line.contains("generation=7")),
            "{logs}"
        );
    }
}

fn family(addr: SocketAddr) -> &'static str {
    if addr.is_ipv4() { "ipv4" } else { "ipv6" }
}
