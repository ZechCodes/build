//! Current operational ICE evidence for the same-NAT sweep authorization.
use rtc::ice::candidate::{Candidate, CandidateType};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NatEvidence {
    Match,
    Missing,
    Different,
}
impl NatEvidence {
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Match => None,
            Self::Missing => Some("nat-evidence-missing"),
            Self::Different => Some("nat-address-mismatch"),
        }
    }
}
pub(super) fn evidence(
    local: &[Candidate],
    remote: &[Candidate],
    live_hosts: &[SocketAddr],
) -> NatEvidence {
    let local = local
        .iter()
        .filter(|candidate| live_hosts.contains(&candidate.base_addr()))
        .filter_map(ipv4_srflx)
        .collect::<HashSet<_>>();
    let remote = remote.iter().filter_map(ipv4_srflx).collect::<HashSet<_>>();
    if local.is_empty() || remote.is_empty() {
        NatEvidence::Missing
    } else if local.is_disjoint(&remote) {
        NatEvidence::Different
    } else {
        NatEvidence::Match
    }
}

fn ipv4_srflx(candidate: &Candidate) -> Option<Ipv4Addr> {
    if candidate.candidate_type() != CandidateType::ServerReflexive
        || !candidate.network_type().is_udp()
        || candidate.component() != 1
    {
        return None;
    }
    match candidate.addr().ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::ice::candidate::{
        CandidateConfig, candidate_server_reflexive::CandidateServerReflexiveConfig,
    };
    fn srflx(ip: &str, port: u16, base: &str, component: u16) -> Candidate {
        let base: SocketAddr = base.parse().unwrap();
        CandidateServerReflexiveConfig {
            base_config: CandidateConfig {
                network: "udp".into(),
                address: ip.into(),
                port,
                component,
                ..Default::default()
            },
            rel_addr: base.ip().to_string(),
            rel_port: base.port(),
            ..Default::default()
        }
        .new_candidate_server_reflexive()
        .unwrap()
    }
    #[test]
    fn only_current_ipv4_srflx_intersection_qualifies_and_ports_need_not_match() {
        let host = "10.72.0.1:45000".parse().unwrap();
        let local = [srflx("203.0.113.1", 52000, "10.72.0.1:45000", 1)];
        let remote = [srflx("203.0.113.1", 53000, "10.72.3.254:40000", 1)];
        assert_eq!(evidence(&local, &remote, &[host]), NatEvidence::Match);
        assert_eq!(
            evidence(
                &local,
                &[srflx("203.0.113.2", 53000, "10.72.3.254:40000", 1)],
                &[host]
            ),
            NatEvidence::Different
        );
        assert_eq!(evidence(&local, &[], &[host]), NatEvidence::Missing);
        assert_eq!(evidence(&[], &remote, &[host]), NatEvidence::Missing);
        assert_eq!(
            evidence(&local, &remote, &[]),
            NatEvidence::Missing,
            "old/rebound local srflx base does not qualify"
        );
    }
    #[test]
    fn relay_prflx_ipv6_and_other_component_evidence_cannot_authorize() {
        let host = "10.72.0.1:45000".parse().unwrap();
        let local = [srflx("203.0.113.1", 52000, "10.72.0.1:45000", 1)];
        for remote in [
            "candidate:r 1 udp 1 203.0.113.1 53000 typ relay raddr 10.72.3.254 rport 40000",
            "candidate:p 1 udp 1 203.0.113.1 53000 typ prflx",
            "candidate:s 1 tcp 1 203.0.113.1 53000 typ srflx raddr 10.72.3.254 rport 40000 tcptype passive",
            "candidate:v 1 udp 1 2001:db8::1 53000 typ srflx raddr 2001:db8::2 rport 40000",
        ] {
            let candidate = rtc::ice::candidate::unmarshal_candidate(
                remote.strip_prefix("candidate:").unwrap(),
            )
            .unwrap();
            assert_eq!(
                evidence(&local, &[candidate], &[host]),
                NatEvidence::Missing
            );
        }
        assert_eq!(
            evidence(
                &local,
                &[srflx("203.0.113.1", 53000, "10.72.3.254:40000", 2)],
                &[host]
            ),
            NatEvidence::Missing
        );
    }
    #[test]
    fn multihomed_sets_match_any_live_mapping_without_retired_base_substitution() {
        let host = "10.72.0.1:45000".parse().unwrap();
        let local = [
            srflx("203.0.113.1", 52000, "10.72.0.1:45001", 1),
            srflx("203.0.113.2", 52000, "10.72.0.1:45000", 1),
        ];
        let remote = [srflx("203.0.113.1", 53000, "10.72.3.254:40000", 1)];
        assert_eq!(evidence(&local, &remote, &[host]), NatEvidence::Different);
        let both = [
            remote[0].clone(),
            srflx("203.0.113.2", 53000, "10.72.3.254:40000", 1),
        ];
        assert_eq!(evidence(&local, &both, &[host]), NatEvidence::Match);
    }
}
