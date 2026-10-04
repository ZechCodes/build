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
use webrtc::peer_connection::{PeerConnection, RTCIceCandidateInit, RTCStatsReport};

use super::{checks::DirectChecks, mdns, RtcError};

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
    async fn resolve(&self, name: &str, use_remembered: bool) -> Result<Vec<IpAddr>, String>;
    fn bind_client_hint(&self, _hint: uuid::Uuid) {}
}

struct LanResolver {
    session_id: String,
    interfaces: Option<Vec<String>>,
    lan_addresses: Arc<mdns::LanAddressCache>,
    client_hint: super::client_hint::Binding,
}

#[async_trait]
impl Resolver for LanResolver {
    fn bind_client_hint(&self, hint: uuid::Uuid) {
        self.client_hint.bind(hint);
    }

    async fn resolve(&self, name: &str, use_remembered: bool) -> Result<Vec<IpAddr>, String> {
        mdns::resolve(
            name,
            self.interfaces.as_deref(),
            &self.session_id,
            &self.lan_addresses,
            self.client_hint.get(),
            use_remembered,
        )
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
            return "direct-checks-pending";
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
    credentials: Option<Vec<IceCredentials>>,
    counts: CandidateCounts,
    checks: DirectChecks,
    seen: HashSet<String>,
    jobs: HashMap<String, JoinHandle<()>>,
    resolved: HashMap<String, (Instant, Vec<IpAddr>)>,
    last_reason: Option<&'static str>,
    resolution_reported: bool,
    reported_failures: HashSet<&'static str>,
    failures: HashMap<&'static str, usize>,
}

impl State {
    fn cancel(&mut self) {
        for (_, job) in self.jobs.drain() {
            job.abort();
        }
        self.generation += 1;
        self.seen.clear();
        self.counts = CandidateCounts::default();
        self.checks = DirectChecks::default();
        self.last_reason = None;
        self.resolution_reported = false;
        self.reported_failures.clear();
        self.failures.clear();
        self.resolved
            .retain(|_, (at, _)| at.elapsed() < RESOLVED_CACHE_TTL);
    }

    fn failed(&mut self, failure: &'static str) {
        self.counts.mdns_unresolved += 1;
        *self.failures.entry(failure).or_default() += 1;
    }

    fn cached(&self, name: &str) -> Option<Vec<IpAddr>> {
        self.resolved
            .get(name)
            .filter(|(at, _)| at.elapsed() < RESOLVED_CACHE_TTL)
            .map(|(_, addresses)| addresses.clone())
    }

    fn reason(&self) -> &'static str {
        self.checks.reason().unwrap_or_else(|| self.counts.reason())
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct IceCredentials {
    ufrag: Option<String>,
    password: Option<String>,
}

impl IceCredentials {
    fn observe(&mut self, line: &str) {
        if let Some(ufrag) = line.strip_prefix("a=ice-ufrag:") {
            self.ufrag = Some(ufrag.to_string());
        }
        if let Some(password) = line.strip_prefix("a=ice-pwd:") {
            self.password = Some(password.to_string());
        }
    }
}

/// Media-level credentials override session-level credentials. Other SDP
/// changes leave the ICE generation, and its pending discovery, intact.
fn ice_credentials(offer: &str) -> Vec<IceCredentials> {
    let mut session = IceCredentials::default();
    let mut current = None;
    let mut media = Vec::new();
    for line in offer.lines() {
        if line.starts_with("m=") {
            if let Some(previous) = current.take() {
                media.push(previous);
            }
            current = Some(session.clone());
        } else {
            current.as_mut().unwrap_or(&mut session).observe(line);
        }
    }
    if let Some(last) = current {
        media.push(last);
    }
    if media.is_empty() {
        media.push(session);
    }
    media
}

pub(super) struct RemoteCandidates {
    session_id: String,
    state: Mutex<State>,
    resolver: Arc<dyn Resolver>,
    emit: Arc<dyn Fn(Value) + Send + Sync>,
}

impl RemoteCandidates {
    pub(super) fn bind_client_hint(&self, hint: uuid::Uuid) {
        self.resolver.bind_client_hint(hint);
    }
    pub(super) fn new(
        session_id: String,
        interfaces: Option<Vec<String>>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
        lan_addresses: Arc<mdns::LanAddressCache>,
        client_hint: Option<uuid::Uuid>,
    ) -> Arc<Self> {
        Self::with_resolver(
            Arc::new(LanResolver {
                session_id: session_id.clone(),
                interfaces,
                lan_addresses,
                client_hint: super::client_hint::Binding::new(client_hint),
            }),
            emit,
            session_id,
        )
    }
    fn with_resolver(
        resolver: Arc<dyn Resolver>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
        session_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            state: Mutex::new(State::default()),
            resolver,
            emit,
        })
    }
    #[cfg(test)]
    pub(super) async fn begin(&self, offer: &str) {
        self.begin_with_stats(offer, None).await;
    }
    pub(super) async fn begin_with_stats(&self, offer: &str, stats: Option<&RTCStatsReport>) {
        let mut state = self.state.lock().await;
        let credentials = ice_credentials(offer);
        if !state.closed && state.credentials.as_ref() == Some(&credentials) {
            return;
        }
        self.observe_checks(&mut state, stats);
        self.summarize(&state, "restart");
        state.cancel();
        state.closed = false;
        state.credentials = Some(credentials);
        self.report(&mut state, "generation", None);
    }
    #[cfg(test)]
    pub(super) async fn close(&self) {
        self.close_with_stats(None).await;
    }
    pub(super) async fn close_with_stats(&self, stats: Option<&RTCStatsReport>) {
        let mut state = self.state.lock().await;
        self.observe_checks(&mut state, stats);
        self.summarize(&state, "close");
        state.cancel();
        state.closed = true;
        state.credentials = None;
        state.resolved.clear();
    }
    fn observe_checks(&self, state: &mut State, stats: Option<&RTCStatsReport>) {
        if state.closed || state.credentials.is_none() {
            return;
        }
        if let Some(stats) = stats {
            state.checks.observe(stats);
            (self.emit)(json!({ "type": "rtc.diagnostics", "event": "direct-checks",
                "reason": state.reason(), "candidates": state.counts,
                "detail": null, "hosts": state.checks.hosts() }));
        }
    }
    fn summarize(&self, state: &State, ended: &str) {
        if state.credentials.is_some() && !state.closed {
            super::diagnostic(
                &self.session_id,
                &format!(
                    "remote_candidates {}",
                    json!({"generation": state.generation,
                    "ended": ended, "reason": state.reason(),
                    "candidates": state.counts, "failures": state.failures,
                    "hosts": state.checks.hosts()})
                ),
            );
        }
    }
    fn report(&self, state: &mut State, event: &str, detail: Option<&'static str>) {
        let first_failure = detail.is_some_and(|failure| state.reported_failures.insert(failure));
        let first_resolution = event == "mdns-resolved" && !state.resolution_reported;
        if event != "generation"
            && state.last_reason == Some(state.reason())
            && !first_resolution
            && !first_failure
        {
            return;
        }
        state.last_reason = Some(state.reason());
        state.resolution_reported |= first_resolution;
        (self.emit)(json!({ "type": "rtc.diagnostics", "event": event,
            "reason": state.reason(), "candidates": state.counts,
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
            if kind != "host-mdns" && state.seen.insert(line.clone()) {
                state.counts.received(kind);
                state.checks.track_candidate(&line);
            }
        }
        self.report(&mut state, "remote-candidate", None);
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
            state.checks.track_candidate(&candidate.candidate);
            self.report(&mut state, "remote-candidate", None);
            return target.add(candidate).await;
        }
        let supplied_name = candidate.candidate.split_whitespace().nth(4).unwrap();
        let name = match mdns::normalized_name(supplied_name) {
            Ok(name) => name,
            Err(_) => {
                self.rejected(&mut state, "mdns-invalid-name");
                return Ok(());
            }
        };
        if let Some(addresses) = state.cached(&name) {
            add_resolved(&*target, &candidate, &addresses, &mut state.checks).await?;
            state.counts.mdns_resolved += 1;
            self.report(&mut state, "mdns-resolved", None);
            return Ok(());
        }
        self.start_discovery(&mut state, target, candidate, name);
        Ok(())
    }

    fn rejected(&self, state: &mut State, failure: &'static str) {
        state.failed(failure);
        self.report(state, "mdns-unresolved", Some(failure));
    }

    fn start_discovery(
        self: &Arc<Self>,
        state: &mut State,
        target: Arc<dyn CandidateTarget>,
        candidate: RTCIceCandidateInit,
        name: String,
    ) {
        if state.jobs.len() >= MAX_DISCOVERIES {
            self.rejected(state, "mdns-capacity");
            return;
        }
        state.counts.mdns_pending += 1;
        self.report(state, "remote-candidate", None);
        let generation = state.generation;
        let remote = self.clone();
        let key = candidate.candidate.clone();
        let job_key = key.clone();
        let job = tokio::spawn(async move {
            remote.discover(generation, target, candidate, name).await;
            let mut state = remote.state.lock().await;
            if state.generation == generation {
                state.jobs.remove(&job_key);
            }
        });
        state.jobs.insert(key, job);
    }

    async fn discover(
        self: &Arc<Self>,
        generation: u64,
        target: Arc<dyn CandidateTarget>,
        candidate: RTCIceCandidateInit,
        name: String,
    ) {
        for attempt in 0..=RETRY_DELAYS.len() {
            let result = self.resolver.resolve(&name, attempt == 0).await;
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
            self.report(&mut state, "remote-candidate", None);
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
                match add_resolved(target, candidate, &addresses, &mut state.checks).await {
                    Ok(()) => {
                        state
                            .resolved
                            .insert(name.to_string(), (Instant::now(), addresses));
                        state.counts.mdns_resolved += 1;
                        self.report(&mut state, "mdns-resolved", None);
                        return true;
                    }
                    Err(_) => "mdns-candidate-rejected",
                }
            }
            Ok(_) => "mdns-unresolved",
            Err(failure) => failure_code(&failure),
        };
        self.rejected(&mut state, failure);
        matches!(
            failure,
            "mdns-invalid-name" | "mdns-no-lan-interfaces" | "mdns-candidate-rejected"
        )
    }
}

fn failure_code(failure: &str) -> &'static str {
    match failure {
        "mdns-invalid-name" => "mdns-invalid-name",
        "mdns-interfaces-unavailable" => "mdns-interfaces-unavailable",
        "mdns-no-lan-interfaces" => "mdns-no-lan-interfaces",
        "mdns-socket-unavailable" => "mdns-socket-unavailable",
        "mdns-answer-rejected" => "mdns-answer-rejected",
        _ => "mdns-unresolved",
    }
}

async fn add_resolved(
    target: &dyn CandidateTarget,
    original: &RTCIceCandidateInit,
    addresses: &[IpAddr],
    checks: &mut DirectChecks,
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
        target.add(candidate.clone()).await?;
        checks.track_candidate(&candidate.candidate);
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
    use tokio::sync::{mpsc, oneshot, Semaphore};

    #[tokio::test]
    async fn a_sessions_first_hint_survives_later_offers_and_ice_restarts() {
        let first = uuid::Uuid::new_v4();
        for initial in [None, Some(first)] {
            let resolver = Arc::new(LanResolver {
                session_id: "hint-binding".into(),
                interfaces: None,
                lan_addresses: Arc::new(mdns::LanAddressCache::new()),
                client_hint: super::super::client_hint::Binding::new(initial),
            });
            let remote = RemoteCandidates::with_resolver(
                resolver.clone(),
                Arc::new(|_| {}),
                "hint-binding".into(),
            );
            remote.begin(&offer("initial", "initial-password")).await;
            remote.bind_client_hint(first);
            remote.bind_client_hint(uuid::Uuid::new_v4());
            remote.begin(&offer("initial", "initial-password")).await;
            remote.bind_client_hint(uuid::Uuid::new_v4());
            remote.begin(&offer("restart", "restart-password")).await;
            remote.bind_client_hint(uuid::Uuid::new_v4());
            assert_eq!(resolver.client_hint.get(), Some(first));
            remote.close().await;
        }
    }

    struct FakeResolver {
        ready: Semaphore,
        answer: Mutex<Result<Vec<IpAddr>, String>>,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl Resolver for FakeResolver {
        async fn resolve(&self, _: &str, _: bool) -> Result<Vec<IpAddr>, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ready.acquire().await.unwrap().forget();
            self.answer.lock().unwrap().clone()
        }
    }

    struct HandshakeResolver {
        requests: mpsc::UnboundedSender<oneshot::Sender<Result<Vec<IpAddr>, String>>>,
    }

    #[async_trait]
    impl Resolver for HandshakeResolver {
        async fn resolve(&self, _: &str, _: bool) -> Result<Vec<IpAddr>, String> {
            let (reply, result) = oneshot::channel();
            self.requests.send(reply).unwrap();
            result.await.unwrap()
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
            answer: Mutex::new(answer),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let events = Arc::new(Mutex::new(vec![]));
        let emit = events.clone();
        let remote = RemoteCandidates::with_resolver(
            resolver.clone(),
            Arc::new(move |event| emit.lock().unwrap().push(event)),
            "test".into(),
        );
        (remote, resolver, Arc::new(Target::default()), events)
    }
    async fn settled() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn actual_check_counters_belong_to_the_generation_that_sent_them() {
        let (remote, _, target, events) = fixture(Err("unused".into()));
        let first = offer("first", "first-password");
        remote.begin(&first).await;
        remote
            .add(target.clone(), candidate("192.0.2.2", "host"))
            .await
            .unwrap();
        assert_eq!(
            events.lock().unwrap().last().unwrap()["reason"],
            "direct-checks-pending"
        );
        let report = super::super::checks::unanswered_report();
        remote.begin_with_stats(&first, Some(&report)).await;
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .all(|event| event["event"] != "direct-checks"),
            "same-credential offers do not end or sample the generation"
        );
        remote
            .begin_with_stats(&offer("second", "second-password"), Some(&report))
            .await;
        let finished = events
            .lock()
            .unwrap()
            .iter()
            .find(|event| event["event"] == "direct-checks")
            .cloned()
            .expect("restart reports the old generation's actual checks");
        assert_eq!(finished["reason"], "direct-checks-no-reply");
        assert_eq!(
            finished["hosts"],
            json!([
                {"ordinal": 1, "requests_sent": 2, "responses_received": 0, "succeeded": false}
            ])
        );
        assert_eq!(
            events.lock().unwrap().last().unwrap()["event"],
            "generation"
        );
        remote
            .add(target, candidate("192.0.2.3", "host"))
            .await
            .unwrap();
        let empty = rtc::peer_connection::RTCPeerConnectionBuilder::new()
            .build()
            .unwrap()
            .get_stats(Instant::now(), rtc::statistics::StatsSelector::None);
        remote.close_with_stats(Some(&empty)).await;
        let last = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(last["event"], "direct-checks");
        assert_eq!(last["reason"], "direct-checks-not-sent");
        assert_eq!(
            last["hosts"],
            json!([
                {"ordinal": 1, "requests_sent": 0, "responses_received": 0, "succeeded": false}
            ])
        );
        for event in [finished, last] {
            let serialized = event.to_string();
            assert!(!serialized.contains("192.0.2."));
            assert!(!serialized.contains("password"));
        }
    }

    fn offer(ufrag: &str, password: &str) -> String {
        format!("v=0\r\na=ice-ufrag:{ufrag}\r\na=ice-pwd:{password}\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\n")
    }

    fn calls(resolver: &FakeResolver) -> usize {
        resolver.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test(start_paused = true)]
    async fn completed_discoveries_release_capacity_for_a_later_valid_host() {
        let (remote, resolver, target, _) = fixture(Err("mdns-no-lan-interfaces".into()));
        remote.begin(&offer("first", "password")).await;
        resolver.ready.add_permits(200);
        for index in 1..=MAX_DISCOVERIES {
            let name = format!("{}.local", uuid::Uuid::from_u128(index as u128));
            remote
                .add(target.clone(), candidate(&name, "host"))
                .await
                .unwrap();
        }
        settled().await;
        tokio::time::advance(Duration::from_secs(120)).await;
        settled().await;
        tokio::time::advance(Duration::from_secs(120)).await;
        settled().await;
        let previous = calls(&resolver);
        *resolver.answer.lock().unwrap() = Ok(vec!["192.168.68.30".parse().unwrap()]);
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        assert_eq!(
            calls(&resolver),
            previous + 1,
            "completed jobs cannot consume the live discovery limit"
        );
        assert_eq!(target.0.lock().unwrap().len(), 1);
        assert!(remote.state.lock().await.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_names_never_spawn_a_resolver_or_retry() {
        let (remote, resolver, target, events) = fixture(Err("mdns-unresolved".into()));
        remote.begin(&offer("first", "password")).await;
        resolver.ready.add_permits(200);
        remote
            .add(target.clone(), candidate("printer.local", "host"))
            .await
            .unwrap();
        settled().await;
        tokio::time::advance(Duration::from_secs(120)).await;
        settled().await;
        assert_eq!(calls(&resolver), 0);
        assert!(remote.state.lock().await.jobs.is_empty());
        assert_eq!(
            events.lock().unwrap().last().unwrap()["detail"],
            "mdns-invalid-name"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn permanent_resolution_failures_do_not_retry() {
        for failure in ["mdns-invalid-name", "mdns-no-lan-interfaces"] {
            let (remote, resolver, target, _) = fixture(Err(failure.into()));
            remote.begin(&offer("first", "password")).await;
            resolver.ready.add_permits(200);
            remote
                .add(
                    target.clone(),
                    candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
                )
                .await
                .unwrap();
            settled().await;
            tokio::time::advance(Duration::from_secs(120)).await;
            settled().await;
            assert_eq!(
                calls(&resolver),
                1,
                "{failure} is terminal for this candidate"
            );
            assert!(remote.state.lock().await.jobs.is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejected_answer_round_retries_then_resolves_and_caches_the_host() {
        let (requests, mut resolutions) = mpsc::unbounded_channel();
        let (diagnostics, mut reported) = mpsc::unbounded_channel();
        let remote = RemoteCandidates::with_resolver(
            Arc::new(HandshakeResolver { requests }),
            Arc::new(move |event| {
                if event["event"] == "mdns-unresolved" || event["event"] == "mdns-resolved" {
                    let _ = diagnostics.send(event);
                }
            }),
            "test".into(),
        );
        let target = Arc::new(Target::default());
        let name = "73af967b-f3ee-4e8d-b0bd-213da4ec5901.local";
        let address = "192.168.68.30".parse().unwrap();
        remote.begin(&offer("first", "password")).await;
        remote
            .add(target.clone(), candidate(name, "host"))
            .await
            .unwrap();
        resolutions
            .recv()
            .await
            .unwrap()
            .send(Err("mdns-answer-rejected".into()))
            .unwrap();
        assert_eq!(
            reported.recv().await.unwrap()["detail"],
            "mdns-answer-rejected"
        );
        {
            let state = remote.state.lock().await;
            assert_eq!(state.jobs.len(), 1, "a rejected round keeps its retry job");
            assert_eq!(state.counts.mdns_unresolved, 1);
            assert!(state.cached(name).is_none());
        }
        let retry_started = tokio::time::Instant::now();
        tokio::time::advance(RETRY_DELAYS[0]).await;
        let reply = resolutions.recv().await.unwrap();
        assert_eq!(retry_started.elapsed(), RETRY_DELAYS[0]);
        reply.send(Ok(vec![address])).unwrap();
        assert_eq!(reported.recv().await.unwrap()["event"], "mdns-resolved");
        {
            let state = remote.state.lock().await;
            assert_eq!(state.cached(name), Some(vec![address]));
            assert_eq!(state.counts.mdns_unresolved, 0);
            assert_eq!(state.counts.mdns_resolved, 1);
            assert!(state.jobs.is_empty());
        }
        assert!(target.0.lock().unwrap()[0]
            .candidate
            .contains("192.168.68.30 48861"));
        remote.begin(&offer("second", "new-password")).await;
        let mut fresh = candidate(name, "host");
        fresh.username_fragment = Some("second".into());
        fresh.candidate = fresh.candidate.replace("48861", "48862");
        remote.add(target.clone(), fresh).await.unwrap();
        let added = target.0.lock().unwrap();
        assert_eq!(added.len(), 2, "the later answer is cached across restart");
        assert!(added[1].candidate.contains("192.168.68.30 48862"));
        assert_eq!(added[1].username_fragment.as_deref(), Some("second"));
        assert!(matches!(
            resolutions.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn the_same_effective_ice_credentials_preserve_pending_discovery() {
        let (remote, resolver, target, events) =
            fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        let first = offer("first", "password");
        remote.begin(&first).await;
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        let same = "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\na=ice-ufrag:first\r\na=ice-pwd:password\r\n";
        remote.begin(same).await;
        resolver.ready.add_permits(1);
        settled().await;
        assert_eq!(
            target.0.lock().unwrap().len(),
            1,
            "ordinary renegotiation keeps the pending LAN query"
        );
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["event"] == "generation")
                .count(),
            1
        );
        remote.begin(&offer("first", "new-password")).await;
        assert_eq!(
            remote.state.lock().await.generation,
            2,
            "a password change starts a new ICE generation"
        );
        remote.begin(&offer("second", "new-password")).await;
        let mut fresh = candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host");
        fresh.username_fragment = Some("second".into());
        fresh.candidate = fresh.candidate.replace("48861", "48862");
        remote.add(target.clone(), fresh).await.unwrap();
        let added = target.0.lock().unwrap();
        assert_eq!(
            added.len(),
            2,
            "the resolved LAN address is reused across ICE restart"
        );
        assert!(added[1].candidate.contains("192.168.68.30 48862"));
        assert_eq!(added[1].username_fragment.as_deref(), Some("second"));
        assert_eq!(
            calls(&resolver),
            1,
            "cache reuse does not begin another query"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn transient_unresolved_names_retry_twice_then_release_their_slot() {
        let (remote, resolver, target, _) = fixture(Err("mdns-unresolved".into()));
        remote.begin(&offer("first", "password")).await;
        resolver.ready.add_permits(3);
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        assert_eq!(calls(&resolver), 1);
        tokio::time::advance(Duration::from_secs(15)).await;
        settled().await;
        assert_eq!(calls(&resolver), 2);
        tokio::time::advance(Duration::from_secs(40)).await;
        settled().await;
        assert_eq!(calls(&resolver), 3);
        assert!(remote.state.lock().await.jobs.is_empty());
    }

    #[tokio::test]
    async fn distinct_failure_categories_are_reported_once_without_private_error_text() {
        let (remote, resolver, target, events) = fixture(Err("mdns-answer-rejected".into()));
        remote.begin(&offer("first", "password")).await;
        remote
            .add(target.clone(), candidate("192.168.68.30", "host"))
            .await
            .unwrap();
        for name in ["printer.local", "other-printer.local"] {
            remote
                .add(target.clone(), candidate(name, "host"))
                .await
                .unwrap();
        }
        resolver.ready.add_permits(2);
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        let rejected = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(rejected["reason"], "direct-checks-pending");
        assert_eq!(
            rejected["detail"], "mdns-answer-rejected",
            "a new failure category stays visible despite an unchanged coarse reason"
        );
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["detail"] == "mdns-invalid-name")
                .count(),
            1
        );
        *resolver.answer.lock().unwrap() = Err("private.local 192.168.1.99".into());
        remote
            .add(
                target.clone(),
                candidate("8c14a372-9db5-4faa-bbf1-d93583114e89.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        assert!(!serde_json::to_string(&*events.lock().unwrap())
            .unwrap()
            .contains("private.local"));
        assert_eq!(
            remote.state.lock().await.failures.get("mdns-unresolved"),
            Some(&1)
        );
        remote.close().await;
    }

    #[tokio::test]
    async fn diagnostics_coalesce_candidate_counts_but_keep_first_resolution_evidence() {
        let (remote, resolver, target, events) =
            fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        remote.begin(&offer("first", "password")).await;
        for suffix in 1..=100 {
            remote
                .add(
                    target.clone(),
                    candidate(&format!("192.168.68.{suffix}"), "host"),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            events.lock().unwrap().len(),
            2,
            "counts do not push once per candidate"
        );
        resolver.ready.add_permits(2);
        for index in 1..=2 {
            let name = format!("{}.local", uuid::Uuid::from_u128(index));
            remote
                .add(target.clone(), candidate(&name, "host"))
                .await
                .unwrap();
            settled().await;
        }
        let events = events.lock().unwrap();
        assert_eq!(
            events.len(),
            3,
            "one resolution event remains necessary even when the reason is unchanged"
        );
        assert_eq!(events.last().unwrap()["event"], "mdns-resolved");
    }

    #[tokio::test]
    async fn unresolved_host_does_not_hold_relay_and_late_resolution_preserves_candidate_metadata()
    {
        let (remote, resolver, target, events) =
            fixture(Ok(vec!["192.168.68.30".parse().unwrap()]));
        remote.begin(&offer("first", "password")).await;
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
        remote.begin(&offer("first", "password")).await;
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
        remote.begin(&offer("first", "password")).await;
        remote
            .add(
                target.clone(),
                candidate("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local", "host"),
            )
            .await
            .unwrap();
        settled().await;
        remote.begin(&offer("second", "new-password")).await;
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
        remote.begin(&offer("first", "password")).await;
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
