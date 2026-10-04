use super::*;
use crate::candidate::unmarshal_candidate;
use sansio::Protocol;
use stun::fingerprint::FINGERPRINT;
use stun::integrity::MessageIntegrity;
use stun::message::{BINDING_REQUEST, BINDING_SUCCESS, Message};

const LOCAL: &str = "candidate:1 1 udp 2130706431 192.0.2.1 5000 typ host";
const RELAY: &str =
    "candidate:2 1 udp 16777215 203.0.113.1 6000 typ relay raddr 192.0.2.2 rport 5001";
const HOST: &str = "candidate:3 1 udp 2130706431 192.0.2.2 5001 typ host";
const REMOTE_PASSWORD: &str = "remote-password-for-checks";

fn selected_relay(enabled: bool, controlling: bool, lite: bool) -> Agent {
    let mut agent = Agent::new(Arc::new(AgentConfig {
        check_pending_direct_pairs: enabled,
        max_binding_requests: Some(2),
        candidate_types: if lite {
            vec![CandidateType::Host]
        } else {
            vec![]
        },
        lite,
        ..Default::default()
    }))
    .unwrap();
    agent
        .add_local_candidate(unmarshal_candidate(LOCAL).unwrap())
        .unwrap();
    agent
        .add_remote_candidate(unmarshal_candidate(RELAY).unwrap())
        .unwrap();
    agent
        .start_connectivity_checks(controlling, "remote-ufrag".into(), REMOTE_PASSWORD.into())
        .unwrap();
    agent.candidate_pairs[0].state = CandidatePairState::Succeeded;
    agent.set_selected_pair(Some(0));
    agent.nominated_pair = Some(0);
    agent.remote_candidates[0].seen(false);
    agent.local_candidates[0].seen(true);
    agent.write_outs.clear();
    agent.event_outs.clear();
    agent
}

fn tick(agent: &mut Agent) -> Vec<TaggedBytesMut> {
    let deadline = agent.poll_timeout().unwrap();
    agent.handle_timeout(deadline).unwrap();
    let mut writes = Vec::new();
    while let Some(write) = agent.poll_write() {
        writes.push(write);
    }
    writes
}

fn host_writes(writes: &[TaggedBytesMut]) -> Vec<&TaggedBytesMut> {
    writes
        .iter()
        .filter(|write| write.transport.peer_addr == "192.0.2.2:5001".parse().unwrap())
        .collect()
}

#[test]
fn late_direct_pair_sends_real_bounded_checks_without_replacing_the_relay() {
    for controlling in [false, true] {
        let mut agent = selected_relay(true, controlling, false);
        agent
            .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
            .unwrap();
        let initial = tick(&mut agent);
        let checks = host_writes(&initial);
        assert_eq!(
            checks.len(),
            1,
            "a selected TURN pair must not suppress the late host check"
        );
        assert_eq!(
            checks[0].transport.local_addr,
            "192.0.2.1:5000".parse().unwrap()
        );
        let mut request = Message::new();
        request.raw = checks[0].message.to_vec();
        request.decode().unwrap();
        assert_eq!(request.typ, BINDING_REQUEST);
        assert_eq!(agent.get_candidate_pairs_stats()[1].requests_sent, 1);

        let mut sent = 1;
        for _ in 0..12 {
            sent += host_writes(&tick(&mut agent)).len();
            assert_eq!(agent.selected_pair, Some(0));
            assert_eq!(agent.state(), ConnectionState::Connected);
        }
        assert_eq!(
            sent, 3,
            "the normal max-binding-request budget bounds pending checks"
        );
        assert_eq!(
            agent.get_candidate_pairs_stats()[1].state,
            CandidatePairState::Failed
        );
        assert!(
            agent.event_outs.is_empty(),
            "checking a late address must not announce another selection"
        );

        agent.last_consent_sent = Instant::now() - Duration::from_secs(10);
        let keepalive = tick(&mut agent);
        assert!(
            keepalive
                .iter()
                .any(|write| write.transport.peer_addr == "203.0.113.1:6000".parse().unwrap()),
            "TURN consent keepalives continue after the direct budget is exhausted"
        );
        assert!(host_writes(&keepalive).is_empty());
    }
}

#[test]
fn a_successful_direct_response_stops_checks_and_keeps_the_selected_relay() {
    let mut agent = selected_relay(true, false, false);
    agent
        .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
        .unwrap();
    let writes = tick(&mut agent);
    let checks = host_writes(&writes);
    assert_eq!(checks.len(), 1);
    let mut request = Message::new();
    request.raw = checks[0].message.to_vec();
    request.decode().unwrap();
    let mut response = Message::new();
    response
        .build(&[
            Box::new(BINDING_SUCCESS),
            Box::new(request.transaction_id),
            Box::new(MessageIntegrity::new_short_term_integrity(
                REMOTE_PASSWORD.into(),
            )),
            Box::new(FINGERPRINT),
        ])
        .unwrap();
    agent
        .handle_read(TaggedBytesMut {
            now: Instant::now(),
            transport: checks[0].transport,
            message: response.raw.as_slice().into(),
        })
        .unwrap();
    let stats = agent.get_candidate_pairs_stats();
    assert_eq!(stats[1].responses_received, 1);
    assert_eq!(stats[1].state, CandidatePairState::Succeeded);
    assert!(!stats[1].nominated);
    for _ in 0..12 {
        assert!(host_writes(&tick(&mut agent)).is_empty());
    }
    assert_eq!(agent.selected_pair, Some(0));
}

#[test]
fn defaults_and_lite_do_not_probe_unselected_pairs() {
    assert!(!AgentConfig::default().check_pending_direct_pairs);
    for (enabled, lite) in [(false, false), (true, true)] {
        let mut agent = selected_relay(enabled, false, lite);
        agent
            .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
            .unwrap();
        for _ in 0..12 {
            assert!(host_writes(&tick(&mut agent)).is_empty());
        }
        assert_eq!(agent.get_candidate_pairs_stats()[1].requests_sent, 0);
        assert_eq!(agent.selected_pair, Some(0));
    }
}

#[test]
fn late_checks_skip_pairs_with_a_relay_at_either_end() {
    let mut agent = selected_relay(true, false, false);
    agent
        .add_local_candidate(
            unmarshal_candidate(
                "candidate:4 1 udp 16777215 203.0.113.2 6001 typ relay raddr 192.0.2.1 rport 5000",
            )
            .unwrap(),
        )
        .unwrap();
    agent
        .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
        .unwrap();
    let writes = tick(&mut agent);
    assert_eq!(writes.len(), 1, "only the host-to-host check is sent");
    assert_eq!(
        writes[0].transport.local_addr,
        "192.0.2.1:5000".parse().unwrap()
    );
    let stats = agent.get_candidate_pairs_stats();
    assert_eq!(stats[1].requests_sent, 0);
    assert_eq!(stats[3].requests_sent, 0);
}
