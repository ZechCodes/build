//! Anonymous ARP discovery, kept separate from the advertised ICE socket.
use super::host_neighbors::{ScoutSnapshot, scout_snapshot_until};
use super::host_sweep::{HostSweepControl, SweepScoutCounters, SweepSubnet};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PACE: Duration = Duration::from_millis(5);
const FRESH: Duration = Duration::from_millis(100);
const MAX_PENDING: usize = 256;
const MAX_SOCKETS: usize = 5;
const INTERFACE_WINDOW: Duration = Duration::from_secs(60);
const MAX_INTERFACE_WINDOWS: usize = 64;

#[derive(Default)]
struct Admission {
    next_packet: Option<Instant>,
    reservations: Vec<Instant>,
    observed_at: Option<Instant>,
    incomplete: usize,
    total: usize,
    threshold: usize,
    soft_threshold: usize,
    sockets: usize,
    interface_windows: HashMap<u32, Instant>,
}
impl Admission {
    fn interface_available(&mut self, index: u32, owner: Option<Instant>, now: Instant) -> bool {
        self.interface_windows
            .retain(|_, start| now < *start || now.duration_since(*start) < INTERFACE_WINDOW);
        if index == 0 {
            return false;
        }
        if let Some(start) = owner {
            return self.interface_windows.get(&index) == Some(&start)
                && now >= start
                && now.duration_since(start) < INTERFACE_WINDOW;
        }
        !self.interface_windows.contains_key(&index)
            && self.interface_windows.len() < MAX_INTERFACE_WINDOWS
    }
    fn start_interface(&mut self, index: u32, at: Instant) {
        self.interface_windows.entry(index).or_insert(at);
    }

    fn observe(
        &mut self,
        at: Instant,
        incomplete: usize,
        total: usize,
        soft_threshold: usize,
        threshold: usize,
    ) {
        if self.observed_at.is_some_and(|previous| at < previous) {
            return;
        }
        self.reservations.retain(|sent| *sent >= at);
        self.observed_at = Some(at);
        self.incomplete = incomplete;
        self.total = total;
        self.threshold = threshold;
        self.soft_threshold = soft_threshold;
    }
    fn pending(&self) -> usize {
        self.incomplete + self.reservations.len()
    }
    fn admit(&self, now: Instant) -> bool {
        self.observed_at
            .is_some_and(|at| now >= at && now.duration_since(at) < FRESH)
            && self.soft_threshold > 1
            && self.threshold > self.soft_threshold
            && self.pending() < MAX_PENDING.min(self.soft_threshold / 2)
            && self.total.saturating_add(self.reservations.len())
                < self.threshold.saturating_mul(3) / 4
    }
    fn claim_tick(&mut self, now: Instant) -> bool {
        if self.next_packet.is_some_and(|next| now < next) {
            return false;
        }
        self.next_packet = Some(now + PACE);
        true
    }
    fn socket_permit(&mut self) -> bool {
        if self.sockets >= MAX_SOCKETS {
            return false;
        }
        self.sockets += 1;
        true
    }
}

static PROCESS: OnceLock<Mutex<Admission>> = OnceLock::new();
fn process() -> &'static Mutex<Admission> {
    PROCESS.get_or_init(|| Mutex::new(Admission::default()))
}

pub(super) fn send_real(
    expires: Instant,
    send: impl FnOnce() -> io::Result<usize>,
) -> io::Result<usize> {
    let mut budget = process().lock().map_err(|_| io::ErrorKind::WouldBlock)?;
    let now = Instant::now();
    if now >= expires || !budget.claim_tick(now) {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    send()
}

struct OwnedScout {
    socket: Option<UdpSocket>,
}
impl OwnedScout {
    fn bind(source: Ipv4Addr, budget: &mut Admission) -> io::Result<Self> {
        if !budget.socket_permit() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let result = UdpSocket::bind((source, 0)).and_then(|socket| {
            socket.set_nonblocking(true)?;
            Ok(socket)
        });
        match result {
            Ok(socket) => Ok(Self {
                socket: Some(socket),
            }),
            Err(error) => {
                budget.sockets -= 1;
                Err(error)
            }
        }
    }
}
fn release_socket() {
    if let Ok(mut budget) = process().lock() {
        budget.sockets = budget.sockets.saturating_sub(1);
    }
}
impl Drop for OwnedScout {
    fn drop(&mut self) {
        drop(self.socket.take());
        release_socket();
    }
}

#[derive(Default)]
pub(super) struct ScoutResources {
    sockets: Vec<OwnedScout>,
}
impl ScoutResources {
    pub fn close(&mut self) {
        self.sockets.clear();
    }
    fn send(
        &mut self,
        source: Ipv4Addr,
        index: u32,
        target: Ipv4Addr,
        budget: &mut Admission,
    ) -> io::Result<usize> {
        for scout in &self.sockets {
            match crate::runtime::host_egress::send_scout(
                scout.socket.as_ref().expect("live scout socket"),
                source,
                index,
                target,
            ) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                result => return result,
            }
        }
        let scout = OwnedScout::bind(source, budget)?;
        let result = crate::runtime::host_egress::send_scout(
            scout.socket.as_ref().expect("live scout socket"),
            source,
            index,
            target,
        );
        self.sockets.push(scout);
        result
    }
}

struct Group {
    subnet: SweepSubnet,
    cursor: usize,
    snapshot: Option<ScoutSnapshot>,
    refresh_at: Instant,
    resources: Arc<Mutex<ScoutResources>>,
    retry: bool,
    ordered: bool,
    failed: u32,
    usable: HashSet<Ipv4Addr>,
    last_admission: Option<Instant>,
    scout_lease: Option<Instant>,
}
impl Group {
    fn scout_available(&mut self, budget: &mut Admission, now: Instant) -> bool {
        if budget.interface_available(self.subnet.interface_index, self.scout_lease, now) {
            return true;
        }
        self.cursor = self.subnet.addresses.len();
        self.failed += 1;
        false
    }
    fn same_subnet(&self, subnet: &SweepSubnet) -> bool {
        self.subnet.same_authorization(subnet)
    }
    fn fresh(&self, now: Instant) -> Option<&ScoutSnapshot> {
        self.snapshot.as_ref().filter(|snapshot| {
            now >= snapshot.observed_at && now.duration_since(snapshot.observed_at) < FRESH
        })
    }
    fn destination(&mut self, now: Instant) -> Option<Ipv4Addr> {
        self.fresh(now)?;
        while self.cursor < self.subnet.addresses.len()
            && self.usable.contains(&self.subnet.addresses[self.cursor])
        {
            self.cursor += 1;
        }
        self.subnet.addresses.get(self.cursor).copied()
    }
    fn permanent_failure(&mut self) {
        self.cursor += 1;
        self.retry = false;
        self.failed += 1;
    }
    fn discovery_complete(&self) -> bool {
        self.cursor >= self.subnet.addresses.len() && self.failed == 0
    }
    fn clear(&mut self) {
        if let Ok(mut resources) = self.resources.lock() {
            resources.close();
        }
        self.snapshot = None;
        self.usable.clear();
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Default)]
pub(super) struct HostScouts {
    generation: u64,
    groups: Vec<Group>,
    pub counters: SweepScoutCounters,
    pause: Option<&'static str>,
    retired: HashSet<(IpAddr, u32, u32, String)>,
    retirement_limit: bool,
}
impl HostScouts {
    pub fn sync(
        &mut self,
        generation: u64,
        subnets: Vec<SweepSubnet>,
        control: &HostSweepControl,
        ufrag: &str,
        now: Instant,
    ) {
        if generation != self.generation {
            self.groups.clear();
            self.retired.clear();
            self.retirement_limit = false;
            self.counters = SweepScoutCounters::default();
            self.generation = generation;
            self.pause = None;
        }
        let groups = std::mem::take(&mut self.groups);
        for group in groups {
            if subnets.iter().any(|subnet| group.same_subnet(subnet)) {
                self.groups.push(group);
            } else {
                self.remember_retired(&group.subnet);
            }
        }
        for subnet in subnets {
            if self.groups.iter().any(|group| group.same_subnet(&subnet)) {
                continue;
            }
            let resources = Arc::new(Mutex::new(ScoutResources::default()));
            if !control.attach_scout_resources(generation, ufrag, Arc::clone(&resources)) {
                continue;
            }
            let suppressed =
                self.retirement_limit || self.retired.contains(&subnet.own_subnet_identity());
            let cursor = if suppressed {
                subnet.addresses.len()
            } else {
                0
            };
            self.groups.push(Group {
                subnet,
                cursor,
                snapshot: None,
                refresh_at: now,
                resources,
                retry: false,
                ordered: false,
                failed: u32::from(suppressed),
                usable: HashSet::new(),
                last_admission: None,
                scout_lease: None,
            });
        }
    }
    pub fn close(&mut self) {
        self.groups.clear();
        self.retired.clear();
        self.retirement_limit = false;
        self.pause = None;
    }
    fn remember_retired(&mut self, subnet: &SweepSubnet) {
        if self.retired.len() < 32 {
            self.retired.insert(subnet.own_subnet_identity());
        } else {
            self.retirement_limit = true;
        }
    }
    pub fn suspend(&mut self) {
        self.retire_active();
    }
    fn retire_active(&mut self) {
        for group in std::mem::take(&mut self.groups) {
            self.remember_retired(&group.subnet);
        }
    }
    pub fn refresh(&mut self, now: Instant) {
        let deadline = Instant::now() + Duration::from_millis(5);
        for group in &mut self.groups {
            if now < group.refresh_at {
                continue;
            }
            group.refresh_at = now + FRESH;
            group.usable.clear();
            group.snapshot = scout_snapshot_until(group.subnet.interface_index, deadline).ok();
            if let Some(snapshot) = &group.snapshot {
                group.usable = snapshot
                    .usable
                    .iter()
                    .copied()
                    .filter(|ip| group.subnet.authorizes(*ip))
                    .collect();
                if !group.ordered {
                    group.subnet.cluster_order(&snapshot.usable);
                    group.ordered = true;
                }
                if let Ok(mut budget) = process().lock() {
                    budget.observe(
                        snapshot.observed_at,
                        snapshot.incomplete_total,
                        snapshot.table_entries,
                        snapshot.gc_thresh2,
                        snapshot.gc_thresh3,
                    );
                }
            }
        }
        self.update_pending();
    }
    pub fn usable(&self, subnet: &SweepSubnet, address: Ipv4Addr, now: Instant) -> bool {
        self.groups
            .iter()
            .find(|group| group.same_subnet(subnet))
            .is_some_and(|group| group.fresh(now).is_some() && group.usable.contains(&address))
    }
    pub fn usable_until(&self, subnet: &SweepSubnet) -> Option<Instant> {
        self.groups
            .iter()
            .find(|group| group.same_subnet(subnet))
            .and_then(|group| group.snapshot.as_ref())
            .map(|snapshot| snapshot.observed_at + FRESH)
    }
    pub fn settled(&self, subnet: &SweepSubnet, now: Instant) -> bool {
        self.groups
            .iter()
            .find(|group| group.same_subnet(subnet))
            .is_some_and(|group| {
                group.discovery_complete()
                    && group.fresh(now).is_some_and(|snapshot| {
                        group
                            .last_admission
                            .is_none_or(|sent| snapshot.observed_at > sent)
                            && !snapshot
                                .incomplete
                                .iter()
                                .any(|ip| group.subnet.authorizes(*ip))
                    })
            })
    }
    pub fn send_one(
        &mut self,
        now: Instant,
        control: &HostSweepControl,
        ufrag: &str,
        port: u16,
        expires: Instant,
    ) -> Option<&'static str> {
        let Some(index) = self
            .groups
            .iter_mut()
            .position(|group| group.destination(now).is_some())
        else {
            return self.changed_pause(
                if self.groups.iter().any(|group| group.fresh(now).is_none()) {
                    Some("neighbor-snapshot-unavailable")
                } else {
                    None
                },
            );
        };
        let result = control.while_allowed(self.generation, ufrag, port, || {
            self.send_group(index, now, expires)
        });
        self.update_pending();
        self.finish_attempt(result)
    }
    fn send_group(
        &mut self,
        index: usize,
        now: Instant,
        expires: Instant,
    ) -> Result<bool, &'static str> {
        let group = &mut self.groups[index];
        let destination = group
            .destination(now)
            .ok_or("neighbor-snapshot-unavailable")?;
        let IpAddr::V4(source) = group.subnet.local.ip() else {
            return Err("unsupported-platform");
        };
        let valid = rtc::shared::ifaces::ifaces()
            .ok()
            .is_some_and(|interfaces| {
                let Ok(name) = std::ffi::CString::new(group.subnet.interface_name.as_str()) else {
                    return false;
                };
                // SAFETY: the name is terminated and only a read-only interface lookup occurs.
                let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
                group.subnet.still_owned(&interfaces, index, destination)
            });
        if !valid {
            group.clear();
            return Err("no-on-link-interface");
        }
        let resource_handle = Arc::clone(&group.resources);
        let mut resources = resource_handle.lock().map_err(|_| "send-error")?;
        let mut budget = process()
            .lock()
            .map_err(|_| "neighbor-snapshot-unavailable")?;
        let send_at = Instant::now();
        if !group.scout_available(&mut budget, send_at) {
            return Err("interface-scout-cooldown");
        }
        if send_at >= expires {
            return Err("window-expired");
        }
        if group.fresh(send_at).is_none() {
            return Err("neighbor-snapshot-unavailable");
        }
        if !budget.claim_tick(send_at) {
            return Ok(false);
        }
        self.counters.attempted = self.counters.attempted.saturating_add(1);
        if !budget.admit(send_at) {
            return Err(
                if budget
                    .observed_at
                    .is_some_and(|at| send_at >= at && send_at.duration_since(at) < FRESH)
                {
                    "neighbor-pressure"
                } else {
                    "neighbor-snapshot-unavailable"
                },
            );
        }
        let result = resources.send(
            source,
            group.subnet.interface_index,
            destination,
            &mut budget,
        );
        match result {
            Ok(1) => {
                let accepted = Instant::now();
                budget.reservations.push(accepted);
                group.last_admission = Some(accepted);
                if group.scout_lease.is_none() {
                    budget.start_interface(group.subnet.interface_index, accepted);
                    group.scout_lease = Some(accepted);
                }
                self.counters.sent += 1;
                self.counters.destinations += 1;
                group.cursor += 1;
                group.retry = false;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err("scout-socket-limit"),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ECONNREFUSED) | Some(libc::EHOSTUNREACH) | Some(libc::ENETUNREACH)
                ) && !group.retry =>
            {
                group.retry = true;
                Err("send-error")
            }
            _ => {
                group.permanent_failure();
                Err("send-error")
            }
        }
    }
    fn update_pending(&mut self) {
        if let Ok(budget) = process().lock() {
            self.counters.pending = budget.pending().min(u32::MAX as usize) as u32;
        }
        self.counters.pending_peak = self.counters.pending_peak.max(self.counters.pending);
    }
    fn finish_attempt(
        &mut self,
        result: Option<Result<bool, &'static str>>,
    ) -> Option<&'static str> {
        let reason = match result {
            Some(Ok(false)) => return None,
            Some(Ok(true)) => None,
            Some(Err(reason)) => Some(reason),
            None => {
                self.retire_active();
                None
            }
        };
        self.changed_pause(reason)
    }
    fn changed_pause(&mut self, reason: Option<&'static str>) -> Option<&'static str> {
        if self.pause == reason {
            return None;
        }
        self.pause = reason;
        reason
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_subnet(own: &[IpAddr]) -> SweepSubnet {
        use rtc::shared::ifaces::{Interface, Kind};
        let interface = Interface {
            name: "wifi".into(),
            kind: Kind::Ipv4,
            addr: Some("10.72.0.1:0".parse().unwrap()),
            mask: Some("255.255.255.248:0".parse().unwrap()),
            hop: None,
        };
        SweepSubnet::from_interface("10.72.0.1:45000".parse().unwrap(), &interface, 2, own).unwrap()
    }
    fn test_group(subnet: SweepSubnet) -> Group {
        Group {
            subnet,
            cursor: 0,
            snapshot: None,
            refresh_at: Instant::now(),
            resources: Arc::new(Mutex::new(ScoutResources::default())),
            retry: false,
            ordered: false,
            failed: 0,
            usable: HashSet::new(),
            last_admission: None,
            scout_lease: None,
        }
    }
    #[test]
    fn interface_window_allows_one_pass_across_peers_and_generations_without_sliding() {
        let now = Instant::now();
        let mut budget = Admission::default();
        assert!(budget.interface_available(2, None, now));
        // Only a successful scout enqueue creates the timestamp.
        assert!(budget.interface_available(2, None, now + PACE));
        budget.start_interface(2, now + PACE);
        assert!(budget.interface_available(2, Some(now + PACE), now + 2 * PACE));
        assert!(
            !budget.interface_available(2, None, now + 2 * PACE),
            "second peer cannot start a pass"
        );
        assert!(
            !budget.interface_available(2, None, now + Duration::from_secs(59)),
            "new generation cannot renew it"
        );
        assert!(
            budget.interface_available(3, None, now + Duration::from_secs(59)),
            "different owning interface independent"
        );
        assert!(budget.interface_available(2, None, now + PACE + Duration::from_secs(60)));
        assert!(
            !budget.interface_available(2, Some(now + PACE), now + PACE + Duration::from_secs(60)),
            "old owner cannot renew after expiry"
        );
    }
    #[test]
    fn full_interface_registry_never_evicts_an_unexpired_pass() {
        let now = Instant::now();
        let mut budget = Admission::default();
        for index in 1..=MAX_INTERFACE_WINDOWS as u32 {
            budget.start_interface(index, now);
        }
        assert!(!budget.interface_available(65, None, now + PACE));
        assert_eq!(budget.interface_windows.len(), MAX_INTERFACE_WINDOWS);
        assert!(budget.interface_available(1, Some(now), now + PACE));
        assert!(!budget.interface_available(1, None, now - PACE));
        assert_eq!(budget.interface_windows.len(), MAX_INTERFACE_WINDOWS);
        assert!(budget.interface_available(65, None, now + INTERFACE_WINDOW));
        assert!(budget.interface_windows.is_empty());
    }
    #[test]
    fn second_peer_and_generation_reuse_known_neighbor_without_scouting_again() {
        use super::super::host_sweep::HostSweep;
        let now = Instant::now();
        let subnet = test_subnet(&[]);
        let phone = Ipv4Addr::new(10, 72, 0, 6);
        let mut budget = Admission::default();
        budget.start_interface(subnet.interface_index, now);
        for generation in [7, 8] {
            let current = now + Duration::from_secs(generation);
            let mut group = test_group(subnet.clone());
            assert!(
                !group.scout_available(&mut budget, current),
                "another peer or ICE generation cannot scout that interface again"
            );
            assert_eq!(group.cursor, subnet.addresses.len());
            group.snapshot = Some(ScoutSnapshot {
                observed_at: current,
                usable: vec![phone],
                incomplete: Vec::new(),
                failed: Vec::new(),
                netns_total: 1,
                incomplete_total: 0,
                table_entries: 1,
                gc_thresh2: 512,
                gc_thresh3: 1024,
            });
            group.usable.insert(phone);
            let scouts = HostScouts {
                groups: vec![group],
                ..Default::default()
            };
            let mut sweep = HostSweep::default();
            sweep.start(
                generation,
                "fresh".into(),
                40000,
                current - Duration::from_millis(250),
            );
            sweep.prepare(40000, vec![subnet.clone()]);
            let probe = sweep
                .next_usable_probe(current, true, |source, address| {
                    scouts.usable(source, address, current)
                })
                .expect("a fresh resolved neighbor still gets a real-host probe");
            assert_eq!(probe.destination, phone);
            assert_eq!(scouts.counters.sent, 0);
            assert_eq!(budget.interface_windows[&subnet.interface_index], now);
        }
    }
    #[test]
    fn a_denied_shared_tick_never_rearms_identical_pressure_logging() {
        let mut scouts = HostScouts::default();
        assert_eq!(
            scouts.finish_attempt(Some(Err("neighbor-pressure"))),
            Some("neighbor-pressure")
        );
        assert_eq!(scouts.finish_attempt(Some(Ok(false))), None);
        assert_eq!(scouts.finish_attempt(Some(Err("neighbor-pressure"))), None);
        assert_eq!(scouts.finish_attempt(Some(Ok(true))), None);
        assert_eq!(
            scouts.finish_attempt(Some(Err("neighbor-pressure"))),
            Some("neighbor-pressure")
        );
    }
    #[test]
    fn late_same_generation_port_never_restarts_a_retired_discovery_cursor() {
        let now = Instant::now();
        let control = HostSweepControl::default();
        control.start(7, "ufrag", 40000);
        let mut scouts = HostScouts::default();
        let subnet = test_subnet(&[]);
        scouts.sync(7, vec![subnet.clone()], &control, "ufrag", now);
        scouts.groups[0].cursor = 2;
        scouts.sync(7, Vec::new(), &control, "ufrag", now + PACE);
        control.cancel(7, "ufrag", 40000);
        control.start(7, "ufrag", 40001);
        scouts.sync(7, vec![subnet.clone()], &control, "ufrag", now + 2 * PACE);
        assert_eq!(scouts.groups[0].cursor, subnet.addresses.len());
        assert!(
            !scouts.groups[0].discovery_complete(),
            "a canceled partial pass cannot claim coverage"
        );
        control.start(8, "new", 40002);
        scouts.sync(8, vec![subnet], &control, "new", now + 3 * PACE);
        assert_eq!(
            scouts.groups[0].cursor, 0,
            "fresh generation gets a fresh discovery budget"
        );
    }
    #[test]
    fn same_size_alias_replacement_is_a_different_authorization() {
        let first = test_subnet(&["10.72.0.2".parse().unwrap()]);
        let second = test_subnet(&["10.72.0.3".parse().unwrap()]);
        assert_eq!(first.addresses.len(), second.addresses.len());
        assert!(!test_group(first).same_subnet(&second));
    }
    #[test]
    fn persistent_error_can_never_claim_successful_discovery_coverage() {
        let mut group = test_group(test_subnet(&[]));
        while group.cursor < group.subnet.addresses.len() {
            group.permanent_failure();
        }
        assert!(!group.discovery_complete());
        assert_eq!(group.failed as usize, group.subnet.addresses.len());
    }
    #[test]
    fn synchronous_last_port_cancel_closes_scout_fds_before_returning() {
        let control = HostSweepControl::default();
        assert!(control.start(7, "ufrag", 40000));
        assert!(control.start(7, "ufrag", 40001));
        let resources = Arc::new(Mutex::new(ScoutResources::default()));
        assert!(control.attach_scout_resources(7, "ufrag", Arc::clone(&resources)));
        let socket = {
            let mut budget = process().lock().unwrap();
            OwnedScout::bind(Ipv4Addr::LOCALHOST, &mut budget).unwrap()
        };
        resources.lock().unwrap().sockets.push(socket);
        control.cancel(7, "ufrag", 40000);
        assert_eq!(
            resources.lock().unwrap().sockets.len(),
            1,
            "unresolved sibling can share scout"
        );
        control.cancel(7, "ufrag", 40001);
        assert!(
            resources.lock().unwrap().sockets.is_empty(),
            "last-port cancel closes fd synchronously"
        );
    }
    #[test]
    fn fresh_complete_pressure_and_global_headroom_are_required_for_scouts() {
        let now = Instant::now();
        let mut budget = Admission::default();
        assert!(
            !budget.admit(now),
            "missing global table metadata fails closed"
        );
        budget.observe(now, 255, 767, 512, 1024);
        assert!(budget.admit(now));
        budget.reservations.push(now);
        assert!(!budget.admit(now), "background plus reservations cap at256");
        budget.observe(now + PACE, 0, 768, 512, 1024);
        assert!(
            !budget.admit(now + PACE),
            "global occupancy cap includes FAILED and other namespaces"
        );
        budget.observe(now + 2 * PACE, 0, 200, 512, 1024);
        assert!(budget.admit(now + 2 * PACE));
        assert!(
            !budget.admit(now + FRESH + 2 * PACE),
            "100ms staleness fails closed"
        );
    }
    #[test]
    fn actual_tuned_soft_threshold_reduces_admission_without_guessing() {
        let now = Instant::now();
        let mut budget = Admission::default();
        budget.observe(now, 63, 200, 128, 1024);
        assert!(budget.admit(now));
        budget.reservations.push(now);
        assert!(!budget.admit(now));
        budget.observe(now + PACE, 0, 100, 0, 1024);
        assert!(!budget.admit(now + PACE));
    }
    #[test]
    fn reservations_survive_cancel_and_dump_overlap_and_reject_old_observations() {
        let now = Instant::now();
        let mut budget = Admission::default();
        budget
            .reservations
            .extend([now, now + PACE, now + 2 * PACE]);
        budget.observe(now + PACE, 2, 10, 512, 1024);
        assert_eq!(budget.reservations, [now + PACE, now + 2 * PACE]);
        assert_eq!(budget.pending(), 4);
        budget.observe(now, 0, 0, 512, 1024);
        assert_eq!(
            budget.pending(),
            4,
            "out-of-order peer dump cannot erase pressure"
        );
        // No fd/candidate cancellation operation releases numeric reservations.
        budget.observe(now + 3 * PACE, 3, 20, 512, 1024);
        assert!(budget.reservations.is_empty());
        assert_eq!(budget.pending(), 3);
    }
    #[test]
    fn real_probes_and_scouts_share_one_process_clock_and_five_socket_permits() {
        let now = Instant::now();
        let mut budget = Admission::default();
        assert!(budget.claim_tick(now));
        assert!(!budget.claim_tick(now));
        assert!(!budget.claim_tick(now + PACE / 2));
        assert!(budget.claim_tick(now + PACE));
        for _ in 0..MAX_SOCKETS {
            assert!(budget.socket_permit());
        }
        assert!(!budget.socket_permit(), "socket bound is process-wide");
    }
}

#[cfg(test)]
#[path = "host_scout_socket_tests.rs"]
mod socket_tests;
