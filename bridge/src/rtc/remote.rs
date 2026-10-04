//! Remote host discovery, isolated from negotiation so mDNS never holds TURN.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use webrtc::peer_connection::{PeerConnection, RTCIceCandidateInit};

use super::{mdns, RtcError};

const MAX_DISCOVERIES: usize = 32;
const RESOLVED_CACHE_TTL: Duration = Duration::from_secs(60);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(15), Duration::from_secs(40)];

#[async_trait]
pub(super) trait CandidateTarget: Send + Sync {
    async fn add(&self, candidate: RTCIceCandidateInit) -> Result<(), RtcError>;
}

pub(super) struct PeerTarget(pub Arc<dyn PeerConnection>);

#[async_trait]
impl CandidateTarget for PeerTarget {
    async fn add(&self, candidate: RTCIceCandidateInit) -> Result<(), RtcError> {
        self.0.add_ice_candidate(candidate).await?;
        Ok(())
    }
}

#[async_trait]
trait Resolver: Send + Sync {
    async fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String>;
}

struct LanResolver {
    session_id: String,
    interfaces: Option<Vec<String>>,
}

#[async_trait]
impl Resolver for LanResolver {
    async fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String> {
        mdns::resolve(name, self.interfaces.as_deref(), &self.session_id)
            .await
            .map_err(|failure| failure.code().to_string())
    }
}

#[derive(Default, Serialize)]
struct CandidateCounts {
    host_mdns: usize,
    host_ip: usize,
    srflx: usize,
    relay: usize,
    mdns_pending: usize,
    mdns_resolved: usize,
    mdns_unresolved: usize,
}

impl CandidateCounts {
    fn received(&mut self, kind: &str) {
        match kind {
            "host-mdns" => self.host_mdns += 1,
            "host-ip" => self.host_ip += 1,
            "srflx" => self.srflx += 1,
            "relay" => self.relay += 1,
            _ => {}
        }
    }
    fn reason(&self) -> &'static str {
        if self.host_ip + self.mdns_resolved > 0 {
            return "direct-checks-no-success";
        }
        if self.mdns_pending > 0 {
            return "mdns-pending";
        }
        if self.mdns_unresolved > 0 {
            return "mdns-unresolved";
        }
        "no-host-candidates"
    }
}

#[derive(Default)]
struct State {
    generation: u64,
    closed: bool,
    counts: CandidateCounts,
    seen: HashSet<String>,
    jobs: Vec<JoinHandle<()>>,
    resolved: HashMap<String, (Instant, Vec<IpAddr>)>,
}

impl State {
    fn cancel(&mut self) {
        for job in self.jobs.drain(..) {
            job.abort();
        }
        self.generation += 1;
        self.seen.clear();
        self.counts = CandidateCounts::default();
        self.resolved
            .retain(|_, (at, _)| at.elapsed() < RESOLVED_CACHE_TTL);
    }
}

pub(super) struct RemoteCandidates {
    state: Mutex<State>,
    resolver: Arc<dyn Resolver>,
    emit: Arc<dyn Fn(Value) + Send + Sync>,
}

impl RemoteCandidates {
    pub(super) fn new(
        session_id: String,
        interfaces: Option<Vec<String>>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> Arc<Self> {
        Self::with_resolver(
            Arc::new(LanResolver {
                session_id,
                interfaces,
            }),
            emit,
        )
    }
    fn with_resolver(
        resolver: Arc<dyn Resolver>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            resolver,
            emit,
        })
    }
    pub(super) async fn begin(&self) {
        let mut state = self.state.lock().await;
        state.cancel();
        state.closed = false;
        self.report(&state, "generation", None);
    }
    pub(super) async fn close(&self) {
        let mut state = self.state.lock().await;
        state.cancel();
        state.closed = true;
        state.resolved.clear();
    }
    fn report(&self, state: &State, event: &str, detail: Option<&str>) {
        (self.emit)(json!({ "type": "rtc.diagnostics", "event": event,
            "reason": state.counts.reason(), "candidates": state.counts,
            "detail": detail }));
    }

    pub(super) async fn observe_offer(&self, offer: &str) {
        let mut state = self.state.lock().await;
        for line in offer
            .lines()
            .filter_map(|line| line.strip_prefix("a=candidate:"))
        {
            let line = format!("candidate:{line}");
            let kind = candidate_kind(&line);
            if kind != "host-mdns" && state.seen.insert(line) {
                state.counts.received(kind);
            }
        }
        self.report(&state, "remote-candidate", None);
    }

    pub(super) async fn add(
        self: &Arc<Self>,
        target: Arc<dyn CandidateTarget>,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), RtcError> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Ok(());
        }
        if !state.seen.insert(candidate.candidate.clone()) {
            return Ok(());
        }
        let kind = candidate_kind(&candidate.candidate);
        state.counts.received(kind);
        if kind != "host-mdns" {
            self.report(&state, "remote-candidate", None);
            return target.add(candidate).await;
        }
        let name = candidate
            .candidate
            .split_whitespace()
            .nth(4)
            .unwrap()
            .to_ascii_lowercase();
        if let Some((at, addresses)) = state.resolved.get(&name) {
            if at.elapsed() < RESOLVED_CACHE_TTL {
                let addresses = addresses.clone();
                add_resolved(&*target, &candidate, &addresses).await?;
                state.counts.mdns_resolved += 1;
                self.report(&state, "mdns-resolved", Some("cached"));
                return Ok(());
            }
        }
        if state.jobs.len() >= MAX_DISCOVERIES {
            state.counts.mdns_unresolved += 1;
            self.report(&state, "mdns-unresolved", Some("mdns-capacity"));
            return Ok(());
        }
        state.counts.mdns_pending += 1;
        self.report(&state, "remote-candidate", None);
        let generation = state.generation;
        let remote = self.clone();
        state.jobs.push(tokio::spawn(async move {
            remote.discover(generation, target, candidate, name).await;
        }));
        Ok(())
    }

    async fn discover(
        self: &Arc<Self>,
        generation: u64,
        target: Arc<dyn CandidateTarget>,
        candidate: RTCIceCandidateInit,
        name: String,
    ) {
        for attempt in 0..=RETRY_DELAYS.len() {
            let result = self.resolver.resolve(&name).await;
            if self
                .complete(generation, &*target, &candidate, &name, result)
                .await
            {
                return;
            }
            let Some(delay) = RETRY_DELAYS.get(attempt) else {
                return;
            };
            tokio::time::sleep(*delay).await;
            let mut state = self.state.lock().await;
            if state.closed || state.generation != generation {
                return;
            }
            state.counts.mdns_unresolved -= 1;
            state.counts.mdns_pending += 1;
            self.report(&state, "remote-candidate", Some("mdns-retry"));
        }
    }

    async fn complete(
        &self,
        generation: u64,
        target: &dyn CandidateTarget,
        candidate: &RTCIceCandidateInit,
        name: &str,
        result: Result<Vec<IpAddr>, String>,
    ) -> bool {
        // Held through insertion: restart cannot replace credentials between
        // this generation check and adding the resolved candidate to the core.
        let mut state = self.state.lock().await;
        if state.closed || state.generation != generation {
            return true;
        }
        state.counts.mdns_pending -= 1;
        let failure = match result {
            Ok(addresses) if !addresses.is_empty() => {
                match add_resolved(target, candidate, &addresses).await {
                    Ok(()) => {
                        state
                            .resolved
                            .insert(name.to_string(), (Instant::now(), addresses));
                        state.counts.mdns_resolved += 1;
                        self.report(&state, "mdns-resolved", None);
                        return true;
                    }
                    Err(_) => "mdns-candidate-rejected".to_string(),
                }
            }
            Ok(_) => "mdns-unresolved".to_string(),
            Err(failure) => failure,
        };
        state.counts.mdns_unresolved += 1;
        self.report(&state, "mdns-unresolved", Some(&failure));
        false
    }
}

async fn add_resolved(
    target: &dyn CandidateTarget,
    original: &RTCIceCandidateInit,
    addresses: &[IpAddr],
) -> Result<(), RtcError> {
    for address in addresses {
        let mut candidate = original.clone();
        let mut fields: Vec<String> = candidate
            .candidate
            .split_whitespace()
            .map(str::to_string)
            .collect();
        fields[4] = address.to_string();
        candidate.candidate = fields.join(" ");
        target.add(candidate).await?;
    }
    Ok(())
}

fn candidate_kind(line: &str) -> &'static str {
    let fields: Vec<&str> = line.split_whitespace().collect();
    match fields
        .get(7)
        .copied()
        .filter(|_| fields.get(6) == Some(&"typ"))
    {
        Some("host")
            if fields[4]
                .trim_end_matches('.')
                .to_ascii_lowercase()
                .ends_with(".local") =>
        {
            "host-mdns"
        }
        Some("host") => "host-ip",
        Some("srflx") => "srflx",
        Some("relay") => "relay",
        _ => "other",
    }
}

pub(super) fn without_mdns(offer: &str) -> (String, Vec<RTCIceCandidateInit>) {
    // Track complete media sections so a=mid may follow a candidate line.
    let mut media_index = None;
    let mut result = String::new();
    let mut deferred = Vec::new();
    let mut section = Vec::new();
    for line in offer.split_inclusive('\n') {
        if line.starts_with("m=") {
            defer_section(&section, media_index, &mut result, &mut deferred);
            section.clear();
            media_index = Some(media_index.map_or(0, |index| index + 1));
        }
        section.push(line);
    }
    defer_section(&section, media_index, &mut result, &mut deferred);
    (result, deferred)
}

fn defer_section(
    lines: &[&str],
    index: Option<u16>,
    output: &mut String,
    candidates: &mut Vec<RTCIceCandidateInit>,
) {
    let mid = lines
        .iter()
        .find_map(|line| line.trim().strip_prefix("a=mid:"))
        .map(str::to_string);
    let fragment = lines
        .iter()
        .find_map(|line| line.trim().strip_prefix("a=ice-ufrag:"))
        .map(str::to_string);
    for line in lines {
        let candidate = line
            .trim()
            .strip_prefix("a=")
            .filter(|line| line.starts_with("candidate:") && candidate_kind(line) == "host-mdns");
        if let Some(candidate) = candidate {
            candidates.push(RTCIceCandidateInit {
                candidate: candidate.to_string(),
                sdp_mid: mid.clone(),
                sdp_mline_index: index,
                username_fragment: fragment.clone(),
                url: None,
            });
        } else {
            output.push_str(line);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::sync::Semaphore;

    struct FakeResolver {
        ready: Semaphore,
        answer: Result<Vec<IpAddr>, String>,
    }
    #[async_trait]
    impl Resolver for FakeResolver {
        async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            self.ready.acquire().await.unwrap().forget();
            self.answer.clone()
        }
    }
    #[derive(Default)]
    struct Target(Mutex<Vec<RTCIceCandidateInit>>);
    #[async_trait]
    impl CandidateTarget for Target {
        async fn add(&self, candidate: RTCIceCandidateInit) -> Result<(), RtcError> {
            self.0.lock().unwrap().push(candidate);
            Ok(())
        }
    }
    fn candidate(address: &str, kind: &str) -> RTCIceCandidateInit {
        serde_json::from_value(json!({
            "candidate": format!("candidate:1 1 udp 2130706431 {address} 48861 typ {kind}"),
            "sdpMid": "0", "sdpMLineIndex": 0, "usernameFragment": "generation"
        }))
        .unwrap()
    }
    type Fixture = (
        Arc<RemoteCandidates>,
        Arc<FakeResolver>,
        Arc<Target>,
        Arc<Mutex<Vec<Value>>>,
    );
    fn fixture(answer: Result<Vec<IpAddr>, String>) -> Fixture {
        let resolver = Arc::new(FakeResolver {
            ready: Semaphore::new(0),
            answer,
        });
        let events = Arc::new(Mutex::new(vec![]));
        let emit = events.clone();
        let remote = RemoteCandidates::with_resolver(
            resolver.clone(),
            Arc::new(move |event| emit.lock().unwrap().push(event)),
        );
        (remote, resolver, Arc::new(Target::default()), events)
    }
    async fn settled() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn unresolved_host_does_not_hold_relay_and_late_resolution_preserves_candidate_metadata()
    {
        let (remote, resolver, target, events) =
            fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        remote.begin().await;
        let original = candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host");
        remote.add(target.clone(), original.clone()).await.unwrap();
        remote
            .add(target.clone(), candidate("203.0.113.4", "relay"))
            .await
            .unwrap();
        assert_eq!(
            target.0.lock().unwrap().len(),
            1,
            "relay is accepted while discovery waits"
        );
        assert_eq!(
            events.lock().unwrap().last().unwrap()["reason"],
            "mdns-pending"
        );
        resolver.ready.add_permits(1);
        settled().await;
        let added = target.0.lock().unwrap();
        assert_eq!(added.len(), 2);
        assert!(added[1].candidate.contains("192.168.68.30 48861 typ host"));
        assert_eq!(added[1].sdp_mid, original.sdp_mid);
        assert_eq!(added[1].username_fragment, original.username_fragment);
        assert_eq!(
            events.lock().unwrap().last().unwrap()["event"],
            "mdns-resolved"
        );
    }

    #[tokio::test]
    async fn failed_resolution_says_why_no_host_pair_is_available() {
        let (remote, resolver, target, events) = fixture(Err("mdns-unresolved".into()));
        remote.begin().await;
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        resolver.ready.add_permits(1);
        settled().await;
        assert!(target.0.lock().unwrap().is_empty());
        let events = events.lock().unwrap();
        let event = events.last().unwrap();
        assert_eq!(event["reason"], "mdns-unresolved");
        assert_eq!(event["candidates"]["mdns_unresolved"], 1);
        assert!(
            !event.to_string().contains("73af967b"),
            "diagnostics contain no address or name"
        );
    }

    #[tokio::test]
    async fn restart_and_close_cancel_discovery_from_the_previous_generation() {
        let (remote, resolver, target, _) = fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        remote.begin().await;
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        remote.begin().await;
        resolver.ready.add_permits(1);
        settled().await;
        assert!(target.0.lock().unwrap().is_empty());
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        remote.close().await;
        resolver.ready.add_permits(1);
        settled().await;
        assert!(target.0.lock().unwrap().is_empty());
    }

    #[test]
    fn embedded_mdns_is_deferred_and_keeps_its_media_section() {
        let offer = "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:data\r\na=candidate:1 1 udp 2130706431 73af967b-f3ee-4e8d-b0bd-213da4ec5901.local 48861 typ host\r\na=candidate:2 1 udp 12 203.0.113.4 40000 typ relay\r\na=end-of-candidates\r\n";
        let (stripped, deferred) = without_mdns(offer);
        assert!(!stripped.contains(".local"));
        assert!(stripped.contains("typ relay\r\n"));
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].sdp_mid.as_deref(), Some("data"));
        assert_eq!(deferred[0].sdp_mline_index, Some(0));
    }

    #[tokio::test]
    async fn absolute_mdns_names_are_resolved_instead_of_forwarded_as_ip_candidates() {
        let (remote, resolver, target, events) =
            fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        remote.begin().await;
        remote
            .add(
                target.clone(),
                candidate("73AF967B-F3EE-4E8D-B0BD-213DA4EC5901.local.", "host"),
            )
            .await
            .unwrap();
        assert!(target.0.lock().unwrap().is_empty());
        resolver.ready.add_permits(1);
        settled().await;
        assert!(target.0.lock().unwrap()[0]
            .candidate
            .contains("192.168.68.30"));
        assert_eq!(
            events.lock().unwrap().last().unwrap()["event"],
            "mdns-resolved"
        );
    }
}
