//! One redacted, bounded summary per lookup; packet traffic only increments counters.
use std::fmt::Write;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::{AnswerRejection, Datagram, LanInterface, PacketAnswer, ResolveFailure};

const MAX_INTERFACES: usize = 8;

pub(super) fn add(counter: &mut u64, amount: u64) {
    *counter = counter.saturating_add(amount);
}

pub(super) fn add_atomic(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(amount))
    });
}

#[derive(Clone, Copy, Default)]
pub(super) struct ReceiveHealth {
    pub errors: u64,
    pub lagged: u64,
}

struct InterfaceCounters {
    address: Ipv4Addr,
    index: u32,
    joined: bool,
    sent: u64,
    failed: u64,
    arrivals: u64,
    direct_sent: u64,
    direct_failed: u64,
}

pub(super) struct RoundDiagnostics {
    started: tokio::time::Instant,
    timeout: Duration,
    eligible: usize,
    interfaces: Vec<InterfaceCounters>,
    pub joined: u64,
    pub sent: u64,
    failed_sends: u64,
    send_timeouts: u64,
    pub retries: u64,
    direct_sent: u64,
    direct_failed: u64,
    datagrams: u64,
    responses: u64,
    queries: u64,
    malformed: u64,
    unsupported: u64,
    wrong_name: u64,
    no_matching_a: u64,
    accepted: u64,
    rejected: [u64; 4],
    unknown_arrival: u64,
    missing_arrival: u64,
    pub receive_errors: u64,
    pub receive_lagged: u64,
    pub shared_receiver: bool,
    pub stop: &'static str,
}

impl RoundDiagnostics {
    pub fn new(interfaces: &[LanInterface], timeout: Duration) -> Self {
        Self {
            started: tokio::time::Instant::now(),
            timeout,
            eligible: interfaces.len(),
            interfaces: interfaces
                .iter()
                .take(MAX_INTERFACES)
                .map(|interface| InterfaceCounters {
                    address: interface.address,
                    index: interface.index,
                    joined: false,
                    sent: 0,
                    failed: 0,
                    arrivals: 0,
                    direct_sent: 0,
                    direct_failed: 0,
                })
                .collect(),
            joined: 0,
            sent: 0,
            failed_sends: 0,
            send_timeouts: 0,
            retries: 0,
            direct_sent: 0,
            direct_failed: 0,
            datagrams: 0,
            responses: 0,
            queries: 0,
            malformed: 0,
            unsupported: 0,
            wrong_name: 0,
            no_matching_a: 0,
            accepted: 0,
            rejected: [0; 4],
            unknown_arrival: 0,
            missing_arrival: 0,
            receive_errors: 0,
            receive_lagged: 0,
            shared_receiver: false,
            stop: "setup-failed",
        }
    }

    pub fn joined(&mut self, interface: &LanInterface) {
        add(&mut self.joined, 1);
        if let Some(counters) = self.interface(interface) {
            counters.joined = true;
        }
    }

    fn interface(&mut self, interface: &LanInterface) -> Option<&mut InterfaceCounters> {
        self.interfaces
            .iter_mut()
            .find(|entry| entry.address == interface.address)
    }

    pub fn sent(&mut self, interface: &LanInterface, succeeded: bool, timed_out: bool) {
        let counter = if succeeded {
            &mut self.sent
        } else {
            &mut self.failed_sends
        };
        add(counter, 1);
        if timed_out {
            add(&mut self.send_timeouts, 1);
        }
        if let Some(counters) = self.interface(interface) {
            add(
                if succeeded {
                    &mut counters.sent
                } else {
                    &mut counters.failed
                },
                1,
            );
        }
    }

    pub fn direct(&mut self, interface: &LanInterface, succeeded: bool, timed_out: bool) {
        add(
            if succeeded {
                &mut self.direct_sent
            } else {
                &mut self.direct_failed
            },
            1,
        );
        if timed_out {
            add(&mut self.send_timeouts, 1);
        }
        if let Some(counters) = self.interface(interface) {
            add(
                if succeeded {
                    &mut counters.direct_sent
                } else {
                    &mut counters.direct_failed
                },
                1,
            );
        }
    }

    pub fn packet(
        &mut self,
        datagram: &Datagram,
        outcome: PacketAnswer,
        interfaces: &[LanInterface],
    ) {
        add(&mut self.datagrams, 1);
        // Even a truncated reply can carry QR. Do not call incoming queries replies.
        if datagram
            .packet
            .get(2)
            .is_some_and(|flags| flags & 0x80 != 0)
        {
            add(&mut self.responses, 1);
        }
        self.arrival(datagram.arrival, interfaces);
        match outcome {
            PacketAnswer::Query => add(&mut self.queries, 1),
            PacketAnswer::Malformed => add(&mut self.malformed, 1),
            PacketAnswer::Unsupported => add(&mut self.unsupported, 1),
            PacketAnswer::WrongName => add(&mut self.wrong_name, 1),
            PacketAnswer::NoMatchingA => add(&mut self.no_matching_a, 1),
            PacketAnswer::Answer(_) => {}
        }
    }

    fn arrival(&mut self, arrival: Option<u32>, interfaces: &[LanInterface]) {
        let Some(index) = arrival else {
            add(&mut self.missing_arrival, 1);
            return;
        };
        for entry in self
            .interfaces
            .iter_mut()
            .filter(|entry| entry.index == index)
        {
            add(&mut entry.arrivals, 1);
        }
        if !interfaces.iter().any(|interface| interface.index == index) {
            add(&mut self.unknown_arrival, 1);
        }
    }

    pub fn accepted(&mut self) {
        add(&mut self.accepted, 1);
    }

    pub fn rejected(&mut self, reason: AnswerRejection) {
        let index = match reason {
            AnswerRejection::OffSubnet => 0,
            AnswerRejection::UnqueriedInterface => 1,
            AnswerRejection::NonLocalAddress => 2,
            AnswerRejection::MissingArrival => 3,
        };
        add(&mut self.rejected[index], 1);
    }

    pub fn has_rejections(&self) -> bool {
        self.rejected.iter().any(|count| *count != 0)
    }

    pub fn summary(&self, result: &Result<Vec<std::net::IpAddr>, ResolveFailure>) -> String {
        let outcome = result
            .as_ref()
            .map_or_else(|failure| failure.code(), |_| "resolved");
        let mut summary = format!(
            "mdns-round outcome={outcome} timeout_ms={} elapsed_ms={} stop={} eligible={} joined={} failed_joins={} sent={} failed_sends={} send_timeouts={} retries={} direct_sent={} direct_failed={} datagrams={} responses={} queries={} malformed={} unsupported={} wrong_name={} no_matching_a={} accepted={} off-subnet={} unqueried-interface={} nonlocal-address={} arrival-interface-unavailable={} unknown_arrival={} missing_arrival={} receive_errors={} receive_lagged={} shared_receiver={} omitted_interfaces={}",
            self.timeout.as_millis(), self.started.elapsed().as_millis(), self.stop,
            self.eligible, self.joined, self.eligible.saturating_sub(self.joined as usize),
            self.sent, self.failed_sends, self.send_timeouts, self.retries, self.direct_sent,
            self.direct_failed, self.datagrams, self.responses, self.queries, self.malformed,
            self.unsupported, self.wrong_name, self.no_matching_a, self.accepted,
            self.rejected[0], self.rejected[1], self.rejected[2], self.rejected[3],
            self.unknown_arrival, self.missing_arrival, self.receive_errors, self.receive_lagged,
            self.shared_receiver, self.eligible.saturating_sub(MAX_INTERFACES),
        );
        // Ordinals and kernel ifindices identify local interfaces without emitting
        // arbitrary OS names (which may themselves contain addresses or log text).
        for (ordinal, entry) in self.interfaces.iter().enumerate() {
            let _ = write!(
                summary,
                " if{}(index={},joined={},sent={},failed={},arrivals={},direct_sent={},direct_failed={})",
                ordinal + 1,
                entry.index,
                u8::from(entry.joined),
                entry.sent,
                entry.failed,
                entry.arrivals,
                entry.direct_sent,
                entry.direct_failed
            );
        }
        summary
    }
}
