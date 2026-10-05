use super::*;
use std::sync::Mutex as StdMutex;
use tokio::sync::Semaphore;
use webrtc::peer_connection::HostCandidateSweepEvent;

const NAME: &str = "73af967b-f3ee-4e8d-b0bd-213da4ec5901.local";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Start(u64, String, u16),
    Cancel(u64, String, u16),
    Clear,
}

#[derive(Default)]
struct SweepTarget {
    added: StdMutex<Vec<RTCIceCandidateInit>>,
    actions: StdMutex<Vec<Action>>,
}

#[async_trait]
impl CandidateTarget for SweepTarget {
    async fn add(&self, candidate: RTCIceCandidateInit) -> Result<(), RtcError> {
        self.added.lock().unwrap().push(candidate);
        Ok(())
    }
    async fn start_sweep(&self, generation: u64, ufrag: String, port: u16) -> Result<(), RtcError> {
        self.actions
            .lock()
            .unwrap()
            .push(Action::Start(generation, ufrag, port));
        Ok(())
    }
    async fn cancel_sweep(
        &self,
        generation: u64,
        ufrag: String,
        port: u16,
    ) -> Result<(), RtcError> {
        self.actions
            .lock()
            .unwrap()
            .push(Action::Cancel(generation, ufrag, port));
        Ok(())
    }
    async fn clear_sweeps(&self) -> Result<(), RtcError> {
        self.actions.lock().unwrap().push(Action::Clear);
        Ok(())
    }
}

struct SweepResolver {
    ready: Semaphore,
    answer: StdMutex<Result<Vec<IpAddr>, String>>,
}

#[async_trait]
impl Resolver for SweepResolver {
    async fn resolve(&self, _: &str, _: bool) -> Result<Vec<IpAddr>, String> {
        self.ready.acquire().await.unwrap().forget();
        self.answer.lock().unwrap().clone()
    }
}

type Fixture = (
    Arc<RemoteCandidates>,
    Arc<SweepResolver>,
    Arc<SweepTarget>,
    Arc<StdMutex<Vec<Value>>>,
);

fn fixture() -> Fixture {
    let resolver = Arc::new(SweepResolver {
        ready: Semaphore::new(0),
        answer: StdMutex::new(Ok(vec!["192.168.68.30".parse().unwrap()])),
    });
    let events = Arc::new(StdMutex::new(Vec::new()));
    let emitted = events.clone();
    let remote = RemoteCandidates::with_resolver(
        resolver.clone(),
        Arc::new(move |event| emitted.lock().unwrap().push(event)),
        "sweep-test".into(),
    );
    (remote, resolver, Arc::new(SweepTarget::default()), events)
}

fn offer(ufrag: &str) -> String {
    format!("v=0\r\na=ice-ufrag:{ufrag}\r\na=ice-pwd:private-password-{ufrag}\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\n")
}

fn candidate(name: &str, ufrag: Option<&str>, port: &str) -> RTCIceCandidateInit {
    RTCIceCandidateInit {
        candidate: format!("candidate:1 1 udp 2130706431 {name} {port} typ host"),
        username_fragment: ufrag.map(str::to_string),
        sdp_mid: Some("0".into()),
        sdp_mline_index: Some(0),
        url: None,
    }
}

async fn settled() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn an_allowed_normalized_mdns_port_starts_before_dns_and_is_deduplicated() {
    let (remote, _, target, events) = fixture();
    remote.begin(&offer("first")).await;
    let uppercase = NAME.to_uppercase() + ".";
    remote
        .add(
            target.clone(),
            candidate(&uppercase, Some("first"), "48861"),
        )
        .await
        .unwrap();
    remote
        .add(target.clone(), candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    assert_eq!(
        *target.actions.lock().unwrap(),
        vec![Action::Start(1, "first".into(), 48861)]
    );
    assert!(
        target.added.lock().unwrap().is_empty(),
        "neither DNS nor remote IP is needed to start"
    );
    assert_eq!(events.lock().unwrap()[0]["event"], "generation");
    assert_eq!(events.lock().unwrap()[0]["generation"], 1);
    remote.close().await;
}

#[tokio::test]
async fn invalid_names_ip_candidates_and_unusable_ports_never_start_sweeps() {
    let (remote, _, target, _) = fixture();
    remote.begin(&offer("first")).await;
    for name in ["printer.local", "192.168.68.30"] {
        remote
            .add(target.clone(), candidate(name, Some("first"), "48861"))
            .await
            .unwrap();
    }
    for port in ["0", "65536", "-1", "invalid"] {
        remote
            .add(target.clone(), candidate(NAME, Some("first"), port))
            .await
            .unwrap();
    }
    let mut tcp = candidate(NAME, Some("first"), "48862");
    tcp.candidate = tcp.candidate.replace(" 1 udp ", " 1 tcp ");
    remote.add(target.clone(), tcp).await.unwrap();
    let mut rtcp = candidate(NAME, Some("first"), "48863");
    rtcp.candidate = rtcp.candidate.replace(" 1 udp ", " 2 udp ");
    remote.add(target.clone(), rtcp).await.unwrap();
    assert!(target.actions.lock().unwrap().is_empty());
    remote.close().await;
}

#[tokio::test]
async fn a_stale_candidate_ufrag_cannot_start_for_the_current_generation() {
    let (remote, _, target, _) = fixture();
    remote.begin(&offer("second")).await;
    remote
        .add(target.clone(), candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    assert!(target.actions.lock().unwrap().is_empty());
    remote.close().await;
}

#[tokio::test]
async fn validated_resolution_cancels_and_cached_resolution_does_not_sweep_again() {
    let (remote, resolver, target, _) = fixture();
    remote.begin(&offer("first")).await;
    remote
        .add(target.clone(), candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    resolver.ready.add_permits(1);
    settled().await;
    assert_eq!(
        *target.actions.lock().unwrap(),
        vec![
            Action::Start(1, "first".into(), 48861),
            Action::Cancel(1, "first".into(), 48861)
        ]
    );
    remote.begin(&offer("second")).await;
    remote
        .add(target.clone(), candidate(NAME, Some("second"), "48862"))
        .await
        .unwrap();
    assert_eq!(
        *target.actions.lock().unwrap(),
        vec![
            Action::Start(1, "first".into(), 48861),
            Action::Cancel(1, "first".into(), 48861),
            Action::Clear
        ]
    );
    assert_eq!(target.added.lock().unwrap().len(), 2);
    remote.close().await;
}

#[tokio::test]
async fn restart_and_close_clear_early_sweeps_and_ignore_late_resolvers() {
    let (remote, resolver, target, _) = fixture();
    remote.begin(&offer("first")).await;
    remote
        .add(target.clone(), candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    remote.begin(&offer("first")).await;
    assert_eq!(
        target.actions.lock().unwrap().len(),
        1,
        "same-credential offers keep the generation"
    );
    remote.begin(&offer("recovery")).await;
    resolver.ready.add_permits(1);
    settled().await;
    assert!(target.added.lock().unwrap().is_empty());
    remote
        .add(target.clone(), candidate(NAME, Some("recovery"), "48862"))
        .await
        .unwrap();
    remote.close().await;
    resolver.ready.add_permits(1);
    settled().await;
    assert!(target.added.lock().unwrap().is_empty());
    assert_eq!(
        *target.actions.lock().unwrap(),
        vec![
            Action::Start(1, "first".into(), 48861),
            Action::Clear,
            Action::Start(2, "recovery".into(), 48862),
            Action::Clear
        ]
    );
}

fn observation(generation: u64) -> HostCandidateSweepEvent {
    let ordinal = u32::try_from(generation).unwrap();
    HostCandidateSweepEvent {
        generation,
        status: "started",
        addresses_sent: 0,
        addresses_attempted: 0,
        scout_datagrams_sent: 13 * ordinal,
        scout_attempted: 14 * ordinal,
        destinations_scouted: 12 * ordinal,
        neighbors_pending: 5 * ordinal,
        neighbors_pending_peak: 8 * ordinal,
        reason: None,
        eligible: true,
        eligible_unresolved: 1,
        prflx_followed: false,
    }
}

#[tokio::test]
async fn sweep_observations_follow_the_marker_and_stale_generations_are_ignored() {
    let (remote, _, target, events) = fixture();
    remote.begin(&offer("first")).await;
    remote
        .add(target, candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    remote.observe_sweep(observation(1)).await;
    let first = events.lock().unwrap().last().unwrap().clone();
    assert_eq!(first["event"], "host-sweep");
    assert_eq!(first["generation"], 1);
    assert_eq!(first["reason"], "mdns-pending");
    assert_eq!(first["candidates"]["host_mdns"], 1);
    assert_eq!(first["sweep"]["eligible"], true);
    assert_eq!(first["sweep"]["eligible_unresolved"], 1);
    assert_eq!(first["sweep"]["addresses_sent"], 0);
    assert_eq!(first["sweep"]["addresses_attempted"], 0);
    assert_eq!(first["sweep"]["scout_datagrams_sent"], 13);
    assert_eq!(first["sweep"]["scout_attempted"], 14);
    assert_eq!(first["sweep"]["destinations_scouted"], 12);
    assert_eq!(first["sweep"]["neighbors_pending"], 5);
    assert_eq!(first["sweep"]["neighbors_pending_peak"], 8);
    let serialized = first.to_string();
    for private in [NAME, "48861", "first", "private-password"] {
        assert!(!serialized.contains(private));
    }
    remote.begin(&offer("second")).await;
    let before = events.lock().unwrap().len();
    remote.observe_sweep(observation(1)).await;
    assert_eq!(events.lock().unwrap().len(), before);
    remote.observe_sweep(observation(2)).await;
    let fresh = events.lock().unwrap().last().unwrap().clone();
    assert_eq!(fresh["generation"], 2);
    assert_eq!(fresh["sweep"]["scout_datagrams_sent"], 26);
    assert_eq!(fresh["sweep"]["neighbors_pending_peak"], 16);
    remote.close().await;
    let before = events.lock().unwrap().len();
    remote.observe_sweep(observation(2)).await;
    assert_eq!(events.lock().unwrap().len(), before);
}

#[tokio::test]
async fn nat_safeguards_keep_ineligible_current_evidence_and_ignore_stale_cooldowns() {
    let (remote, _, target, events) = fixture();
    remote.begin(&offer("first")).await;
    remote
        .add(target, candidate(NAME, Some("first"), "48861"))
        .await
        .unwrap();
    for reason in ["nat-evidence-missing", "nat-address-mismatch"] {
        let mut blocked = observation(1);
        blocked.status = "progress";
        blocked.reason = Some(reason);
        blocked.eligible = false;
        blocked.eligible_unresolved = 0;
        remote.observe_sweep(blocked).await;
        let current = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(current["sweep"]["reason"], reason);
        assert_eq!(current["sweep"]["eligible"], false);
        assert_eq!(current["sweep"]["eligible_unresolved"], 0);
        for private in [NAME, "48861", "first", "private-password"] {
            assert!(!current.to_string().contains(private));
        }
    }
    remote.begin(&offer("second")).await;
    let mut cooldown = observation(1);
    cooldown.status = "progress";
    cooldown.reason = Some("interface-scout-cooldown");
    let before = events.lock().unwrap().len();
    remote.observe_sweep(cooldown.clone()).await;
    assert_eq!(events.lock().unwrap().len(), before);
    cooldown.generation = 2;
    remote.observe_sweep(cooldown).await;
    let current = events.lock().unwrap().last().unwrap().clone();
    assert_eq!(current["sweep"]["reason"], "interface-scout-cooldown");
    assert_eq!(current["sweep"]["eligible"], true);
    assert_eq!(current["sweep"]["eligible_unresolved"], 1);
    remote.close().await;
}

#[tokio::test]
async fn media_credentials_supply_the_sweep_when_candidate_ufrag_is_absent() {
    let (remote, _, target, _) = fixture();
    let media = offer("session") + "a=ice-ufrag:media\r\na=ice-pwd:media-password\r\n";
    remote.begin(&media).await;
    remote
        .add(target.clone(), candidate(NAME, None, "48861"))
        .await
        .unwrap();
    assert_eq!(
        *target.actions.lock().unwrap(),
        vec![Action::Start(1, "media".into(), 48861)]
    );
    remote.close().await;
}
