//! Ignored real-socket pressure gate, confined to socket-pressure.py namespaces.

use super::{HostScouts, process};
use crate::peer_connection::host_sweep::{HostSweepControl, SweepSubnet};
use crate::runtime::{AsyncUdpSocket, EcnCodepoint, Transmit};
use futures::FutureExt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn require_disposable_namespace() {
    let outer = std::env::var("BUILD_RTC_PRESSURE_PARENT_NET_NS")
        .expect("socket-pressure.py must supply the outer namespace");
    assert!(
        std::fs::read_link("/proc/self/ns/net").unwrap() != PathBuf::from(outer),
        "this fixture requires a disposable network namespace"
    );
}

fn socket_budget(socket: &UdpSocket) -> io::Result<(i32, i32)> {
    let mut queued: libc::c_int = 0;
    let mut capacity: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&capacity) as libc::socklen_t;
    // SAFETY: both read-only calls write into initialized integers of their
    // documented sizes. The descriptor remains live for the whole call.
    let (read_queue, read_capacity) = unsafe {
        (
            libc::ioctl(socket.as_raw_fd(), libc::TIOCOUTQ, &mut queued),
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&mut capacity as *mut libc::c_int).cast(),
                &mut length,
            ),
        )
    };
    if read_queue < 0 || read_capacity < 0 || length as usize != std::mem::size_of_val(&capacity) {
        return Err(io::Error::last_os_error());
    }
    if queued < 0 || capacity <= 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok((queued, capacity))
}

#[derive(Default)]
struct Pressure {
    sockets_peak: usize,
    scout_queue_peak: i32,
    scout_buffer_min: Option<i32>,
    ordinary_queue_peak: i32,
    ordinary_us: u128,
    maximum_us: u128,
    writes: usize,
}

impl Pressure {
    fn observe(&mut self, scouts: &HostScouts, ordinary: &UdpSocket) {
        let sockets = scouts
            .groups
            .iter()
            .map(|group| {
                let resources = group.resources.lock().unwrap();
                for scout in &resources.sockets {
                    let socket = scout.socket.as_ref().expect("live scout socket");
                    let (queued, capacity) = socket_budget(socket).unwrap();
                    assert!(
                        queued <= capacity / 4,
                        "scout output must stay within one quarter"
                    );
                    self.scout_queue_peak = self.scout_queue_peak.max(queued);
                    self.scout_buffer_min = Some(
                        self.scout_buffer_min
                            .map_or(capacity, |old| old.min(capacity)),
                    );
                }
                resources.sockets.len()
            })
            .sum::<usize>();
        self.sockets_peak = self.sockets_peak.max(sockets);
        assert!(sockets <= 5, "scouts exceed the process socket cap");
        let budget = process().lock().unwrap();
        assert!(
            budget.sockets <= 5,
            "all peer scouts exceed the process socket cap"
        );
        assert!(
            budget.pending() <= 256.min(budget.soft_threshold / 2),
            "observed neighbors and unobserved admissions exceed the pending cap"
        );
        drop(budget);
        self.ordinary_queue_peak = self
            .ordinary_queue_peak
            .max(socket_budget(ordinary).unwrap().0);
    }

    fn writes_now(&mut self, socket: &dyn AsyncUdpSocket, target: SocketAddr) {
        let ordinary = [0u8; 112];
        let started = Instant::now();
        let first = socket.send_to(&ordinary, target).now_or_never();
        self.ordinary_us = self.ordinary_us.max(started.elapsed().as_micros());
        let ready = matches!(first, Some(Ok(112)));
        println!("ordinary_first_poll_ready={ready}; ordinary_bytes=112");
        assert!(
            ready,
            "scout pressure must not park an ordinary advertised-socket write"
        );
        let maximum = vec![0u8; crate::peer_connection::transports::MAX_GSO_BATCH_BYTES.min(65507)];
        let transmit = Transmit {
            destination: target,
            ecn: Some(EcnCodepoint::Ect0),
            contents: &maximum,
            segment_size: Some(1024),
            src_ip: None,
        };
        let started = Instant::now();
        let first = futures::future::poll_fn(|cx| socket.poll_send(cx, &transmit)).now_or_never();
        self.maximum_us = self.maximum_us.max(started.elapsed().as_micros());
        let ready = matches!(first, Some(Ok(length)) if length == maximum.len());
        println!(
            "maximum_first_poll_ready={ready}; maximum_gso_bytes={}",
            maximum.len()
        );
        assert!(
            ready,
            "scout pressure must not park the maximum valid production GSO write"
        );
        self.writes += 1;
    }
}

fn saturate_ordinary_fixture(socket: &UdpSocket, subnet: &SweepSubnet, peer: Ipv4Addr) {
    let payload = [0u8; 1024];
    let mut blocked = false;
    for address in subnet
        .addresses
        .iter()
        .copied()
        .filter(|address| *address != peer)
    {
        match socket.send_to(&payload, (address, 9)) {
            Ok(length) => assert!(length == payload.len(), "fixture datagram was truncated"),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                blocked = true;
                break;
            }
            Err(error) => panic!("private pressure fixture failed: {error}"),
        }
    }
    assert!(
        blocked,
        "the negative control must saturate the ordinary output queue"
    );
}

#[test]
#[ignore = "requires socket-pressure.py --scout disposable network namespace"]
fn scout_pressure_keeps_advertised_ice_socket_immediately_writable() {
    require_disposable_namespace();
    let runtime = crate::runtime::default_runtime().unwrap();
    let driver_runtime = runtime.clone();
    runtime.block_on(Box::pin(async move {
        let source = Ipv4Addr::new(10, 72, 0, 1);
        let peer = Ipv4Addr::new(10, 72, 0, 2);
        let target = SocketAddr::from((peer, 40000));
        let interfaces = rtc::shared::ifaces::ifaces().unwrap();
        assert!(interfaces.iter().any(|interface| interface.addr.map(|address| address.ip()) == Some(IpAddr::V4(source))
            && interface.mask.map(|mask| mask.ip()) == Some(IpAddr::V4(Ipv4Addr::new(255, 255, 252, 0)))),
            "the disposable fixture must own its private bounded subnet");
        let bound = UdpSocket::bind((source, 0)).unwrap();
        bound.set_nonblocking(true).unwrap();
        let local = bound.local_addr().unwrap();
        let socket = driver_runtime.wrap_udp_socket(bound.try_clone().unwrap()).unwrap();
        assert!(socket.local_addr().unwrap() == local, "runtime wrapper must retain the advertised source port");
        assert!(socket.max_gso_segments() >= 64, "GSO unsupported in the private fixture");
        let subnet = SweepSubnet::for_socket(local, &interfaces, |name| {
            let name = std::ffi::CString::new(name).unwrap();
            // SAFETY: read-only lookup of a terminated, authorized interface name.
            unsafe { libc::if_nametoindex(name.as_ptr()) }
        }).unwrap();
        socket.send_to(b"warm neighbor", target).await.unwrap();
        driver_runtime.sleep(Duration::from_millis(100)).await;
        let control = HostSweepControl::default();
        assert!(control.start(7, "fixture", 40000));
        let mut scouts = HostScouts::default();
        let started = Instant::now();
        scouts.sync(7, vec![subnet.clone()], &control, "fixture", started);
        let expires = started + Duration::from_secs(25);
        let mut pressure = Pressure::default();
        pressure.writes_now(&*socket, target);
        let mut iteration = 0;
        while started.elapsed() < Duration::from_secs(2) {
            let now = Instant::now();
            scouts.refresh(now);
            scouts.send_one(now, &control, "fixture", 40000, expires);
            pressure.observe(&scouts, &bound);
            if iteration % 20 == 0 { pressure.writes_now(&*socket, target); }
            iteration += 1;
            driver_runtime.sleep(Duration::from_millis(5)).await;
        }
        println!("scout_datagrams_sent={}; scout_attempted={}; destinations_scouted={}; neighbors_pending_peak={}; scout_sockets_peak={}; scout_queue_peak={}; scout_buffer_min={}; ordinary_queue_peak={}; ordinary_buffer={}",
            scouts.counters.sent, scouts.counters.attempted, scouts.counters.destinations,
            scouts.counters.pending_peak, pressure.sockets_peak, pressure.scout_queue_peak,
            pressure.scout_buffer_min.unwrap_or(0), pressure.ordinary_queue_peak, socket_budget(&bound).unwrap().1);
        assert!(scouts.counters.sent > 64 && scouts.counters.pending_peak > 64 && pressure.sockets_peak >= 2,
            "the unanswered subnet must exercise multiple scout sockets and sustained neighbor pressure");
        assert!(scouts.counters.pending_peak <= 256, "scouts exceeded the conservative pending cap");
        if std::env::var_os("BUILD_RTC_SCOUT_PRESSURE_SATURATE_HOST").is_some() {
            saturate_ordinary_fixture(&bound, &subnet, peer);
            let (queued, capacity) = socket_budget(&bound).unwrap();
            println!("negative_control=true; ordinary_queue={queued}; ordinary_buffer={capacity}");
        }
        pressure.writes_now(&*socket, target);
        assert!(socket.local_addr().unwrap() == local && bound.local_addr().unwrap() == local,
            "ordinary and GSO writes must retain the advertised source port");
        control.clear();
        scouts.close();
        assert!(process().lock().unwrap().sockets == 0, "closing the plan must release every scout socket");
        println!("ordinary_bytes=112; ordinary_max_us={}; maximum_gso_bytes=65507; maximum_gso_max_us={}; write_pairs={}; source_port_matches=true; scout_sockets_after_close=0",
            pressure.ordinary_us, pressure.maximum_us, pressure.writes);
    }));
}
