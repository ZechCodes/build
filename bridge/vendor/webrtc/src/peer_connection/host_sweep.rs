//! Bounded, credential-free conntrack probes on an advertised host socket.

use rtc::shared::ifaces::{Interface, NextHop};
use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GRACE: Duration = Duration::from_millis(250);
const PACE: Duration = Duration::from_millis(5);
const REPEAT_DELAY: Duration = Duration::from_secs(1);
const WINDOW: Duration = Duration::from_secs(25);
const MAX_PORTS: usize = 32;
const MAX_PACKETS: u32 = 32768;

/// A content-free observation. Addresses, ports and credentials never leave the driver.
#[derive(Clone, Debug)]
pub struct HostCandidateSweepEvent {
    /// Bridge generation ordinal, supplied by the caller.
    pub generation: u64,
    /// started, progress, stopped or skipped.
    pub status: &'static str,
    /// Successful datagram sends, cumulative for this generation.
    pub addresses_sent: u32,
    /// Scheduled send attempts, including transient socket failures.
    pub addresses_attempted: u32,
    /// Successful anonymous one-byte UDP9 scout enqueues, cumulative per generation.
    pub scout_datagrams_sent: u32,
    /// Paced scout attempts, including socket or neighbor-pressure yields.
    pub scout_attempted: u32,
    /// Unique successfully scouted source/interface/destinations; known neighbors excluded.
    pub destinations_scouted: u32,
    /// Latest current-netns INCOMPLETE count plus unobserved process reservations.
    pub neighbors_pending: u32,
    /// Generation maximum of that conservative pending pressure.
    pub neighbors_pending_peak: u32,
    /// Fixed content-free skip or stop reason.
    pub reason: Option<&'static str>,
    /// An advertised host socket has a safe, bounded on-link subnet.
    pub eligible: bool,
    /// Generation-wide eligible ports whose names remain unresolved.
    pub eligible_unresolved: u32,
    /// An authenticated peer-reflexive pair matched a swept host socket and port.
    pub prflx_followed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SweepSubnet {
    pub local: SocketAddr,
    pub interface_name: String,
    pub interface_index: u32,
    pub addresses: Arc<[Ipv4Addr]>,
    authorized: Arc<[Ipv4Addr]>,
    mask: u32,
}

impl SweepSubnet {
    pub fn own_subnet_identity(&self) -> (IpAddr, u32, u32, String) {
        (
            self.local.ip(),
            self.mask,
            self.interface_index,
            self.interface_name.clone(),
        )
    }
    pub fn same_authorization(&self, other: &Self) -> bool {
        self.local.ip() == other.local.ip()
            && self.interface_index == other.interface_index
            && self.interface_name == other.interface_name
            && self.mask == other.mask
            && (Arc::ptr_eq(&self.authorized, &other.authorized)
                || self.authorized == other.authorized)
    }
    pub fn authorizes(&self, address: Ipv4Addr) -> bool {
        self.authorized.binary_search(&address).is_ok()
    }
    pub fn prioritize(&mut self, neighbors: &[Ipv4Addr]) {
        let hints = neighbors
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        // Reorder only the already authorized set. A neighbor observation never
        // adds a destination or authorizes a candidate, and is not cached.
        Arc::make_mut(&mut self.addresses).sort_by_key(|address| !hints.contains(address));
    }

    pub fn cluster_order(&mut self, neighbors: &[Ipv4Addr]) {
        let authorized = self
            .addresses
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let known = neighbors
            .iter()
            .copied()
            .filter(|ip| authorized.contains(ip))
            .collect::<std::collections::HashSet<_>>();
        let mut anchors = known.iter().copied().map(u32::from).collect::<Vec<_>>();
        if let IpAddr::V4(source) = self.local.ip() {
            anchors.push(source.into());
        }
        anchors.sort_unstable();
        anchors.dedup();
        Arc::make_mut(&mut self.addresses).sort_by_cached_key(|address| {
            let value = u32::from(*address);
            let insertion = anchors.partition_point(|anchor| *anchor < value);
            let left = insertion
                .checked_sub(1)
                .map(|index| value.abs_diff(anchors[index]));
            let right = anchors.get(insertion).map(|anchor| value.abs_diff(*anchor));
            let distance = left.into_iter().chain(right).min().unwrap_or(u32::MAX);
            (!known.contains(address), distance, value)
        });
    }

    pub fn from_interface(
        local: SocketAddr,
        interface: &Interface,
        interface_index: u32,
        local_addresses: &[IpAddr],
    ) -> Result<Self, &'static str> {
        let IpAddr::V4(ip) = local.ip() else {
            return Err("non-private-subnet");
        };
        if interface_index == 0 || interface.addr.map(|addr| addr.ip()) != Some(local.ip()) {
            return Err("no-on-link-interface");
        }
        if matches!(interface.hop, Some(NextHop::Destination(_))) {
            return Err("point-to-point");
        }
        let mask = interface
            .mask
            .and_then(|addr| match addr.ip() {
                IpAddr::V4(mask) => Some(u32::from(mask)),
                _ => None,
            })
            .ok_or("invalid-netmask")?;
        let host_bits = !mask;
        if host_bits & host_bits.wrapping_add(1) != 0 {
            return Err("invalid-netmask");
        }
        let network = u32::from(ip) & mask;
        let broadcast = network | host_bits;
        if !private_ipv4(ip) || !private_ipv4(network.into()) || !private_ipv4(broadcast.into()) {
            return Err("non-private-subnet");
        }
        if u64::from(host_bits) + 1 > 1024 {
            return Err("subnet-too-large");
        }
        let addresses = (network..=broadcast)
            .filter(|value| *value != network && *value != broadcast && *value != u32::from(ip))
            .map(Ipv4Addr::from)
            .filter(|ip| !local_addresses.contains(&IpAddr::V4(*ip)))
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err("no-usable-addresses");
        }
        let addresses: Arc<[Ipv4Addr]> = addresses.into();
        Ok(Self {
            authorized: Arc::clone(&addresses),
            local,
            interface_name: interface.name.clone(),
            interface_index,
            addresses,
            mask,
        })
    }

    pub fn for_socket(
        local: SocketAddr,
        interfaces: &[Interface],
        index: impl Fn(&str) -> u32,
    ) -> Result<Self, &'static str> {
        let owners = interfaces
            .iter()
            .filter(|interface| interface.addr.map(|addr| addr.ip()) == Some(local.ip()))
            .collect::<Vec<_>>();
        if owners.len() > 1 {
            return Err("ambiguous-interface");
        }
        let owner = owners.first().ok_or("no-on-link-interface")?;
        let local_addresses = interfaces
            .iter()
            .filter_map(|interface| interface.addr.map(|addr| addr.ip()))
            .collect::<Vec<_>>();
        Self::from_interface(local, owner, index(&owner.name), &local_addresses)
    }

    pub fn still_owned(&self, interfaces: &[Interface], index: u32, destination: Ipv4Addr) -> bool {
        if index != self.interface_index
            || interfaces.iter().any(|interface| {
                interface.addr.map(|addr| addr.ip()) == Some(IpAddr::V4(destination))
            })
        {
            return false;
        }
        if interfaces
            .iter()
            .filter(|interface| interface.addr.map(|addr| addr.ip()) == Some(self.local.ip()))
            .count()
            != 1
        {
            return false;
        }
        interfaces.iter().any(|interface| {
            interface.name == self.interface_name
                && interface.addr.map(|addr| addr.ip()) == Some(self.local.ip())
                && interface.mask.map(|mask| mask.ip()) == Some(IpAddr::V4(self.mask.into()))
                && !matches!(interface.hop, Some(NextHop::Destination(_)))
        })
    }
}

fn private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// STUN Binding Indications request no reply. Only FINGERPRINT is an attribute;
/// the fresh random transaction ID cannot identify the session.
pub(crate) fn binding_indication() -> rtc::shared::error::Result<Vec<u8>> {
    use rtc::stun::fingerprint::FINGERPRINT;
    use rtc::stun::message::{
        CLASS_INDICATION, METHOD_BINDING, Message, MessageType, TransactionId,
    };
    let mut message = Message::new();
    message.build(&[
        Box::new(MessageType {
            method: METHOD_BINDING,
            class: CLASS_INDICATION,
        }),
        Box::new(TransactionId::new()),
        Box::new(FINGERPRINT),
    ])?;
    Ok(message.raw)
}

#[derive(Clone, Debug)]
pub(crate) struct SweepProbe {
    pub port: u16,
    pub expires: Instant,
    pub subnet: SweepSubnet,
    pub destination: Ipv4Addr,
}

/// Immutable generation credentials accepted while the core is locked. Debug
/// intentionally hides both fields; neither enters a probe or public event.
#[derive(Clone)]
pub(crate) struct SweepCredentials {
    ufrag: String,
    password: String,
}

impl std::fmt::Debug for SweepCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SweepCredentials(<redacted>)")
    }
}
impl SweepCredentials {
    pub fn new(ufrag: &str, password: &str) -> Self {
        Self {
            ufrag: ufrag.into(),
            password: password.into(),
        }
    }
    pub fn matches(&self, credentials: (&str, &str)) -> bool {
        self.ufrag == credentials.0 && self.password == credentials.1
    }
    pub fn ufrag(&self) -> &str {
        &self.ufrag
    }
}

/// Synchronous cancellation makes resolution effective before the driver's next
/// syscall, even if its event queue is full or a callback holds the bridge lock.
#[derive(Default)]
pub(crate) struct HostSweepControl {
    state: std::sync::Mutex<ControlState>,
}

#[derive(Default)]
struct ControlState {
    generation: Option<u64>,
    ufrag: String,
    ports: std::collections::BTreeSet<u16>,
    retired: bool,
    #[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
    scouts: Vec<std::sync::Weak<std::sync::Mutex<super::host_scout::ScoutResources>>>,
}

#[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
impl ControlState {
    fn close_scouts(&mut self) {
        for entry in self.scouts.drain(..) {
            if let Some(resources) = entry.upgrade()
                && let Ok(mut resources) = resources.lock()
            {
                resources.close();
            }
        }
    }
}

impl HostSweepControl {
    #[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
    pub(super) fn attach_scout_resources(
        &self,
        generation: u64,
        ufrag: &str,
        resources: Arc<std::sync::Mutex<super::host_scout::ScoutResources>>,
    ) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.generation != Some(generation)
            || state.ufrag != ufrag
            || state.retired
            || state.ports.is_empty()
        {
            return false;
        }
        state.scouts.retain(|entry| entry.strong_count() > 0);
        state.scouts.push(Arc::downgrade(&resources));
        true
    }

    pub fn start(&self, generation: u64, ufrag: &str, port: u16) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state
            .generation
            .is_some_and(|current| generation < current || (generation == current && state.retired))
        {
            return false;
        }
        if state.generation != Some(generation) {
            #[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
            state.close_scouts();
            state.generation = Some(generation);
            state.ufrag = ufrag.into();
            state.ports.clear();
            state.retired = false;
        }
        if state.ufrag != ufrag || (state.ports.len() >= MAX_PORTS && !state.ports.contains(&port))
        {
            return false;
        }
        state.ports.insert(port);
        true
    }
    pub fn cancel(&self, generation: u64, ufrag: &str, port: u16) {
        if let Ok(mut state) = self.state.lock()
            && state.generation == Some(generation)
            && state.ufrag == ufrag
        {
            state.ports.remove(&port);
            #[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
            if state.ports.is_empty() {
                state.close_scouts();
            }
        }
    }
    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.ports.clear();
            #[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
            state.close_scouts();
            state.retired = true;
        }
    }
    pub fn generation_allowed(&self, generation: u64, ufrag: &str) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.generation == Some(generation) && state.ufrag == ufrag && !state.retired
        })
    }
    pub fn allowed(&self, generation: u64, ufrag: &str, port: u16) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.generation == Some(generation)
                && state.ufrag == ufrag
                && state.ports.contains(&port)
                && !state.retired
        })
    }
    pub fn while_allowed<T>(
        &self,
        generation: u64,
        ufrag: &str,
        port: u16,
        send: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.state.lock().ok()?;
        if state.generation != Some(generation)
            || state.ufrag != ufrag
            || !state.ports.contains(&port)
            || state.retired
        {
            return None;
        }
        Some(send())
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SweepScoutCounters {
    pub sent: u32,
    pub attempted: u32,
    pub destinations: u32,
    pub pending: u32,
    pub pending_peak: u32,
}

#[derive(Default)]
pub(crate) struct HostSweep {
    generation: u64,
    remote_ufrag: String,
    remote_password: String,
    plans: BTreeMap<u16, PortSweep>,
    next_packet: Option<Instant>,
    packets: u32,
    sent: u32,
    prflx_followed: bool,
    scout: SweepScoutCounters,
    events: VecDeque<HostCandidateSweepEvent>,
    relay: bool,
}

struct PortSweep {
    due: Instant,
    expires: Instant,
    subnets: Option<Vec<SweepSubnet>>,
    cursor: usize,
    repeat: bool,
    stopped: bool,
    sent: u32,
    attempted: u32,
    prflx_followed: bool,
    report_pending: bool,
    eligible: bool,
    unresolved: bool,
    successful_destinations: std::collections::HashSet<(SocketAddr, Ipv4Addr)>,
    pass_destinations: std::collections::HashSet<(SocketAddr, Ipv4Addr)>,
    attempted_destination: Option<(SocketAddr, Ipv4Addr)>,
    attempted_at: Instant,
}

impl HostSweep {
    pub fn start(&mut self, generation: u64, ufrag: String, port: u16, now: Instant) {
        if generation < self.generation {
            return;
        }
        if self.remote_ufrag != ufrag || generation != self.generation {
            self.stop_all("generation-changed");
            self.snapshot_queued_events();
            self.plans.clear();
            self.prflx_followed = false;
            self.packets = 0;
            self.sent = 0;
            self.scout = SweepScoutCounters::default();
            self.next_packet = None;
            self.remote_ufrag = ufrag;
            self.generation = generation;
        }
        if port == 0 || self.plans.contains_key(&port) {
            return;
        }
        if self.plans.len() >= MAX_PORTS {
            self.events
                .push_back(self.event("skipped", Some("port-limit"), false, 0, 0, false));
            return;
        }
        self.plans.insert(
            port,
            PortSweep {
                due: now + GRACE,
                expires: now + WINDOW,
                subnets: None,
                cursor: 0,
                repeat: false,
                stopped: false,
                sent: 0,
                attempted: 0,
                prflx_followed: false,
                report_pending: false,
                eligible: false,
                unresolved: true,
                successful_destinations: std::collections::HashSet::new(),
                pass_destinations: std::collections::HashSet::new(),
                attempted_destination: None,
                attempted_at: now,
            },
        );
    }

    pub fn unprepared_ports(&self, now: Instant) -> Vec<u16> {
        self.plans
            .iter()
            .filter(|(_, plan)| !plan.stopped && plan.subnets.is_none() && now >= plan.due)
            .map(|(port, _)| *port)
            .collect()
    }

    pub fn prepare(&mut self, port: u16, subnets: Vec<SweepSubnet>) {
        if let Some(plan) = self.plans.get_mut(&port) {
            plan.eligible = !subnets.is_empty();
            plan.subnets = Some(subnets);
            self.events
                .push_back(self.event("started", None, true, 0, 0, false));
        }
    }

    pub fn skip(&mut self, port: u16, reason: &'static str) {
        if let Some(plan) = self.plans.get_mut(&port) {
            plan.stopped = true;
            plan.eligible = false;
            plan.retire_addresses();
            self.events
                .push_back(plan.event(self.generation, "skipped", Some(reason), false));
        }
    }

    #[cfg(test)]
    pub fn next_probe(&mut self, now: Instant, relay: bool) -> Option<SweepProbe> {
        self.next_usable_probe(now, relay, |_, _| true)
    }

    pub fn next_usable_probe(
        &mut self,
        now: Instant,
        relay: bool,
        usable: impl Fn(&SweepSubnet, Ipv4Addr) -> bool,
    ) -> Option<SweepProbe> {
        self.relay = relay;
        self.expire(now);
        if self.next_packet.is_some_and(|next| now < next) {
            return None;
        }
        if self.packets.saturating_add(self.scout.attempted) >= MAX_PACKETS {
            self.stop_all("packet-limit");
            return None;
        }
        for (port, plan) in &mut self.plans {
            if let Some((subnet, destination)) = plan.take_next(now, relay, &usable) {
                plan.attempted += 1;
                plan.attempted_at = now;
                self.packets += 1;
                self.next_packet = Some(now + PACE);
                return Some(SweepProbe {
                    port: *port,
                    expires: plan.expires,
                    subnet,
                    destination,
                });
            }
        }
        None
    }

    pub fn record_result(&mut self, port: u16, sent: bool) {
        if sent {
            self.sent += 1;
        }
        if let Some(plan) = self.plans.get_mut(&port) {
            if sent {
                plan.sent += 1;
                plan.cursor += 1;
                if let Some(destination) = plan.attempted_destination.take() {
                    plan.successful_destinations.insert(destination);
                    plan.pass_destinations.insert(destination);
                }
                if plan.finished_pass() {
                    plan.report_pending = true;
                    plan.finish_pass(plan.attempted_at);
                }
            } else {
                plan.attempted_destination = None;
            }
            if plan.report_pending {
                plan.report_pending = false;
                let status = if plan.stopped { "stopped" } else { "progress" };
                let reason = if plan.stopped {
                    Some("completed")
                } else {
                    None
                };
                self.events
                    .push_back(plan.event(self.generation, status, reason, true));
            }
        }
    }

    pub fn cancel_inactive(&mut self, control: &HostSweepControl) {
        let ports = self
            .plans
            .keys()
            .copied()
            .filter(|port| !control.allowed(self.generation, &self.remote_ufrag, *port))
            .collect::<Vec<_>>();
        let ufrag = self.remote_ufrag.clone();
        for port in ports {
            self.cancel(self.generation, &ufrag, port);
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn remote_ufrag(&self) -> &str {
        &self.remote_ufrag
    }

    pub fn cancel(&mut self, generation: u64, ufrag: &str, port: u16) {
        if generation != self.generation || ufrag != self.remote_ufrag {
            return;
        }
        if let Some(plan) = self.plans.get_mut(&port)
            && plan.unresolved
        {
            plan.stopped = true;
            plan.unresolved = false;
            plan.retire_addresses();
            self.events
                .push_back(plan.event(self.generation, "stopped", Some("resolved"), false));
        }
    }

    pub fn sync_generation(&mut self, ufrag: &str) {
        if ufrag != self.remote_ufrag {
            self.clear("generation-changed");
        }
    }

    pub fn clear(&mut self, reason: &'static str) {
        self.stop_all(reason);
        self.plans.clear();
        self.remote_ufrag.clear();
        self.remote_password.clear();
        self.next_packet = None;
    }

    pub fn stop_all(&mut self, reason: &'static str) {
        let retire = [
            "direct-selected",
            "resolved",
            "generation-changed",
            "closed",
        ]
        .contains(&reason);
        for plan in self.plans.values_mut() {
            if !plan.stopped || (retire && plan.eligible && plan.unresolved) {
                plan.stopped = true;
                if retire {
                    plan.unresolved = false;
                    plan.retire_addresses();
                }
                self.events
                    .push_back(plan.event(self.generation, "stopped", Some(reason), false));
            }
        }
    }

    pub fn observe_prflx(&mut self, local: SocketAddr, remote: SocketAddr) {
        let Some(plan) = self.plans.get_mut(&remote.port()) else {
            return;
        };
        let IpAddr::V4(ip) = remote.ip() else {
            return;
        };
        let matched = plan.successful_destinations.contains(&(local, ip));
        if matched && plan.sent > 0 && !plan.prflx_followed {
            plan.prflx_followed = true;
            self.prflx_followed = true;
            self.events
                .push_back(plan.event(self.generation, "progress", None, true));
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.plans
            .values()
            .filter(|plan| !plan.stopped || plan.subnets.is_some())
            .map(|plan| {
                let due = if plan.stopped || (plan.repeat && !self.relay) {
                    plan.expires
                } else {
                    plan.due
                };
                due.max(self.next_packet.unwrap_or(due)).min(plan.expires)
            })
            .min()
    }

    pub fn pop_event(&mut self) -> Option<HostCandidateSweepEvent> {
        let mut event = self.events.pop_front()?;
        if event.generation == self.generation {
            event.prflx_followed = self.prflx_followed;
            event.addresses_sent = self.sent;
            event.addresses_attempted = self.packets;
            self.scout.fill(&mut event);
            event.eligible_unresolved = self
                .plans
                .values()
                .filter(|plan| plan.eligible && plan.unresolved)
                .count() as u32;
        }
        Some(event)
    }
    pub fn sync_credentials(&mut self, ufrag: &str, password: &str) {
        if self.remote_ufrag != ufrag || self.remote_password != password {
            self.clear("generation-changed");
        }
        self.remote_ufrag = ufrag.into();
        self.remote_password = password.into();
    }
    pub fn defer_preparation(&mut self, port: u16, now: Instant) {
        if let Some(plan) = self.plans.get_mut(&port) {
            plan.due = now + Duration::from_millis(50);
        }
    }
    pub fn note_skip(&mut self, reason: &'static str) {
        self.events
            .push_back(self.event("skipped", Some(reason), false, 0, 0, false));
    }
    pub fn set_scout_counters(&mut self, counters: SweepScoutCounters) {
        self.scout = counters;
    }
    pub fn defer_tick(&mut self, now: Instant) {
        if self.next_packet.is_none_or(|next| now >= next) {
            self.next_packet = Some(now + PACE);
        }
    }
    pub fn settle_discovery(
        &mut self,
        now: Instant,
        relay: bool,
        usable: impl Fn(&SweepSubnet, Ipv4Addr) -> bool,
        settled: impl Fn(&SweepSubnet) -> bool,
    ) {
        self.expire(now);
        for plan in self.plans.values_mut() {
            if !plan.can_settle(now, relay, &usable, &settled) {
                continue;
            }
            plan.finish_pass(now);
            let status = if plan.stopped { "stopped" } else { "progress" };
            self.events.push_back(plan.event(
                self.generation,
                status,
                if plan.stopped {
                    Some("completed")
                } else {
                    None
                },
                true,
            ));
        }
    }
    pub fn active_subnets(&mut self, now: Instant) -> Vec<SweepSubnet> {
        self.expire(now);
        self.plans
            .values()
            .filter(|plan| !plan.stopped)
            .filter_map(|plan| plan.subnets.as_ref())
            .flatten()
            .cloned()
            .collect()
    }
    pub fn active_port(&self) -> Option<(u16, Instant)> {
        self.plans
            .iter()
            .find(|(_, plan)| !plan.stopped && plan.unresolved)
            .map(|(port, plan)| (*port, plan.expires))
    }
    pub fn combined_attempts(&self) -> u32 {
        self.packets.saturating_add(self.scout.attempted)
    }
    pub fn note_progress(&mut self, reason: &'static str) {
        self.events
            .push_back(self.event("progress", Some(reason), true, 0, 0, false));
    }
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    fn snapshot_queued_events(&mut self) {
        for event in &mut self.events {
            if event.generation == self.generation {
                event.prflx_followed = self.prflx_followed;
                event.addresses_sent = self.sent;
                event.addresses_attempted = self.packets;
                self.scout.fill(event);
            }
        }
    }

    fn expire(&mut self, now: Instant) {
        for plan in self.plans.values_mut() {
            if now < plan.expires {
                continue;
            }
            if !plan.stopped {
                plan.stopped = true;
                self.events.push_back(plan.event(
                    self.generation,
                    "stopped",
                    Some("window-expired"),
                    false,
                ));
            }
            plan.retire_addresses();
        }
    }

    fn event(
        &self,
        status: &'static str,
        reason: Option<&'static str>,
        eligible: bool,
        sent: u32,
        attempted: u32,
        prflx_followed: bool,
    ) -> HostCandidateSweepEvent {
        HostCandidateSweepEvent {
            generation: self.generation,
            status,
            reason,
            eligible,
            eligible_unresolved: 0,
            addresses_sent: sent,
            addresses_attempted: attempted,
            prflx_followed,
            scout_datagrams_sent: 0,
            scout_attempted: 0,
            destinations_scouted: 0,
            neighbors_pending: 0,
            neighbors_pending_peak: 0,
        }
    }
}

impl SweepScoutCounters {
    fn fill(&self, event: &mut HostCandidateSweepEvent) {
        event.scout_datagrams_sent = self.sent;
        event.scout_attempted = self.attempted;
        event.destinations_scouted = self.destinations;
        event.neighbors_pending = self.pending;
        event.neighbors_pending_peak = self.pending_peak;
    }
}

impl PortSweep {
    fn can_settle(
        &self,
        now: Instant,
        relay: bool,
        usable: &impl Fn(&SweepSubnet, Ipv4Addr) -> bool,
        settled: &impl Fn(&SweepSubnet) -> bool,
    ) -> bool {
        if self.stopped || now < self.due || (self.repeat && !relay) {
            return false;
        }
        self.subnets.as_ref().is_some_and(|subnets| {
            !subnets.is_empty()
                && subnets.iter().all(|subnet| {
                    settled(subnet)
                        && subnet.addresses.iter().all(|ip| {
                            !usable(subnet, *ip)
                                || self.pass_destinations.contains(&(subnet.local, *ip))
                        })
                })
        })
    }
    fn retire_addresses(&mut self) {
        self.subnets = None;
        self.successful_destinations.clear();
        self.pass_destinations.clear();
        self.attempted_destination = None;
    }

    fn take_next(
        &mut self,
        now: Instant,
        relay: bool,
        usable: &impl Fn(&SweepSubnet, Ipv4Addr) -> bool,
    ) -> Option<(SweepSubnet, Ipv4Addr)> {
        if self.stopped || now < self.due || (self.repeat && !relay) {
            return None;
        }
        let subnets = self.subnets.as_ref()?;
        for subnet in subnets {
            if let Some(destination) = subnet.addresses.iter().find(|ip| {
                !self.pass_destinations.contains(&(subnet.local, **ip)) && usable(subnet, **ip)
            }) {
                self.attempted_destination = Some((subnet.local, *destination));
                return Some((subnet.clone(), *destination));
            }
        }
        None
    }

    fn finished_pass(&self) -> bool {
        self.subnets.as_ref().is_some_and(|subnets| {
            self.cursor == subnets.iter().map(|s| s.addresses.len()).sum::<usize>()
        })
    }

    fn finish_pass(&mut self, now: Instant) {
        if self.repeat {
            self.stopped = true;
        } else {
            self.repeat = true;
            self.cursor = 0;
            self.pass_destinations.clear();
            self.due = now + REPEAT_DELAY;
        }
    }

    fn event(
        &self,
        generation: u64,
        status: &'static str,
        reason: Option<&'static str>,
        eligible: bool,
    ) -> HostCandidateSweepEvent {
        HostCandidateSweepEvent {
            generation,
            status,
            reason,
            eligible,
            eligible_unresolved: 0,
            addresses_sent: self.sent,
            addresses_attempted: self.attempted,
            prflx_followed: self.prflx_followed,
            scout_datagrams_sent: 0,
            scout_attempted: 0,
            destinations_scouted: 0,
            neighbors_pending: 0,
            neighbors_pending_peak: 0,
        }
    }
}

#[cfg(test)]
#[path = "host_sweep_tests.rs"]
mod tests;
