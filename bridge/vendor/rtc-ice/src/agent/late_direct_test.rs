use super::*;
use crate::attributes::{control::AttrControlling, priority::PriorityAttr};
use crate::candidate::unmarshal_candidate;
use sansio::Protocol;
use stun::fingerprint::FINGERPRINT;
use stun::integrity::MessageIntegrity;
use stun::message::{BINDING_REQUEST, BINDING_SUCCESS, Message};

const LOCAL: &str = "candidate:1 1 udp 2130706431 192.0.2.1 5000 typ host";
const RELAY: &str =
    "candidate:2 1 udp 16777215 203.0.113.1 6000 typ relay raddr 192.0.2.2 rport 5001";
const HOST: &str = "candidate:3 1 udp 2130706431 192.0.2.2 5001 typ host";
const PRFLX: &str = "candidate:4 1 udp 2130706431 192.0.2.3 5002 typ prflx";
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
fn self_reported_peer_reflexive_address_cannot_count_as_authenticated_hit() {
    let mut agent = selected_relay(true, false, false);
    agent
        .add_remote_candidate(unmarshal_candidate(PRFLX).unwrap())
        .unwrap();
    let index = agent
        .candidate_pairs
        .iter()
        .position(|pair| {
            agent.remote_candidates[pair.remote_index].candidate_type()
                == CandidateType::PeerReflexive
        })
        .unwrap();
    assert_eq!(agent.authenticated_peer_reflexive_pairs().count(), 0);
    agent.candidate_pairs[index].state = CandidatePairState::Succeeded;
    assert_eq!(agent.authenticated_peer_reflexive_pairs().count(), 0);
    agent.candidate_pairs[index].requests_received = 1;
    assert_eq!(agent.authenticated_peer_reflexive_pairs().count(), 1);
    agent.candidate_pairs[index].requests_received = 0;
    agent.candidate_pairs[index].responses_received = 1;
    assert_eq!(agent.authenticated_peer_reflexive_pairs().count(), 1);
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

#[test]
fn failed_checking_agent_keeps_unsampled_checks_without_operational_candidates() {
    let mut agent = Agent::new(Arc::new(AgentConfig::default())).unwrap();
    agent
        .add_local_candidate(unmarshal_candidate(LOCAL).unwrap())
        .unwrap();
    agent
        .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
        .unwrap();
    agent
        .start_connectivity_checks(false, "remote-ufrag".into(), REMOTE_PASSWORD.into())
        .unwrap();
    let now = Instant::now();
    agent.handle_timeout(now).unwrap();
    agent
        .handle_timeout(
            now + agent.disconnected_timeout + agent.failed_timeout + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(agent.state(), ConnectionState::Failed);
    assert!(agent.get_local_candidates().is_empty());
    assert!(agent.get_remote_candidates().is_empty());
    assert!(agent.candidate_pairs.is_empty());
    assert!(agent.get_selected_candidate_pair().is_none());
    assert_eq!(agent.poll_timeout(), None);

    let pairs = agent.get_candidate_pairs_stats();
    assert_eq!(
        pairs.len(),
        1,
        "the first stats query after failure retains the checks actually sent"
    );
    assert_eq!(pairs[0].requests_sent, 1);
    assert_eq!(pairs[0].responses_received, 0);
    let remotes = agent.get_remote_candidates_stats();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].id, pairs[0].remote_candidate_id);
    assert_eq!(remotes[0].candidate_type, CandidateType::Host);
    assert_eq!(
        agent.get_candidate_pairs_stats()[0].requests_sent,
        1,
        "repeated sampling cannot spend or duplicate checks"
    );
}

#[test]
fn failed_connected_agent_keeps_success_and_clears_it_only_on_valid_restart_or_close() {
    let mut agent = selected_relay(true, false, false);
    agent
        .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
        .unwrap();
    let writes = tick(&mut agent);
    let check = host_writes(&writes)[0];
    let mut request = Message::new();
    request.raw = check.message.to_vec();
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
            transport: check.transport,
            message: response.raw.as_slice().into(),
        })
        .unwrap();

    agent.remote_candidates[0]
        .set_last_received(Instant::now() - agent.disconnected_timeout - Duration::from_millis(1));
    tick(&mut agent);
    assert_eq!(agent.state(), ConnectionState::Disconnected);
    agent.remote_candidates[0].set_last_received(
        Instant::now() - agent.disconnected_timeout - agent.failed_timeout - Duration::from_secs(1),
    );
    tick(&mut agent);
    assert_eq!(agent.state(), ConnectionState::Failed);
    let pairs = agent.get_candidate_pairs_stats();
    assert_eq!(
        pairs.len(),
        2,
        "failure retains one stats snapshot before cleaning up active vectors"
    );
    assert_eq!(pairs[1].requests_sent, 1);
    assert_eq!(pairs[1].responses_received, 1);
    assert_eq!(pairs[1].state, CandidatePairState::Succeeded);

    assert!(agent.restart("x".into(), "short".into(), false).is_err());
    assert_eq!(
        agent.get_candidate_pairs_stats()[1].responses_received,
        1,
        "rejected restart credentials must not erase the final snapshot"
    );
    assert!(
        agent
            .restart("valid-fragment".into(), "short".into(), false)
            .is_err()
    );
    assert_eq!(
        agent.get_remote_candidates_stats().len(),
        2,
        "password validation also precedes snapshot retirement"
    );
    agent
        .restart("next-generation".into(), REMOTE_PASSWORD.into(), false)
        .unwrap();
    assert!(agent.get_candidate_pairs_stats().is_empty());
    assert!(agent.get_remote_candidates_stats().is_empty());
    agent
        .add_local_candidate(unmarshal_candidate(LOCAL).unwrap())
        .unwrap();
    agent
        .add_remote_candidate(unmarshal_candidate(HOST).unwrap())
        .unwrap();
    agent
        .start_connectivity_checks(false, "new-remote-ufrag".into(), REMOTE_PASSWORD.into())
        .unwrap();
    agent.update_connection_state(ConnectionState::Failed);
    assert_eq!(
        agent.get_candidate_pairs_stats()[0].requests_sent,
        0,
        "the new generation cannot inherit an earlier request"
    );
    agent.close().unwrap();
    assert!(agent.get_candidate_pairs_stats().is_empty());
    assert!(agent.get_remote_candidates_stats().is_empty());
}

#[test]
fn failed_agent_keeps_metadata_discovered_from_an_inbound_check() {
    let mut agent = Agent::new(Arc::new(AgentConfig::default())).unwrap();
    agent
        .add_local_candidate(unmarshal_candidate(LOCAL).unwrap())
        .unwrap();
    agent
        .start_connectivity_checks(false, "remote-ufrag".into(), REMOTE_PASSWORD.into())
        .unwrap();
    let credentials = agent.get_local_credentials();
    let mut request = Message::new();
    request
        .build(&[
            Box::new(BINDING_REQUEST),
            Box::new(TransactionId::new()),
            Box::new(Username::new(
                ATTR_USERNAME,
                format!("{}:remote-ufrag", credentials.ufrag),
            )),
            Box::new(AttrControlling(0)),
            Box::new(PriorityAttr(2130706431)),
            Box::new(MessageIntegrity::new_short_term_integrity(
                credentials.pwd.clone(),
            )),
            Box::new(FINGERPRINT),
        ])
        .unwrap();
    agent
        .handle_read(TaggedBytesMut {
            now: Instant::now(),
            transport: TransportContext {
                local_addr: "192.0.2.1:5000".parse().unwrap(),
                peer_addr: "192.0.2.2:5001".parse().unwrap(),
                ..Default::default()
            },
            message: request.raw.as_slice().into(),
        })
        .unwrap();
    agent.update_connection_state(ConnectionState::Failed);
    let pairs = agent.get_candidate_pairs_stats();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].requests_received, 1);
    assert_eq!(pairs[0].requests_sent, 1);
    let metadata = agent.get_remote_candidates_stats();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].id, pairs[0].remote_candidate_id);
    assert_eq!(metadata[0].candidate_type, CandidateType::PeerReflexive);
    assert_eq!(metadata[0].ip, "192.0.2.2");
    assert!(agent.get_remote_candidates().is_empty());
}
