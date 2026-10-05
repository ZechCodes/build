//! Linux-only explicit-interface, gateway-free sends on an existing ICE socket.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::AsRawFd;

fn queue_headroom(used: i32, capacity: i32) -> io::Result<()> {
    if used < 0 || capacity <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "socket budget unavailable",
        ));
    }
    // Reserve room for the tiny probe's kernel bookkeeping as well as its payload.
    // Keep at least three quarters of the socket available to ordinary ICE/DTLS/SCTP.
    if used.saturating_add(4096) > capacity / 4 {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    Ok(())
}

fn require_queue_headroom(socket: &::tokio::net::UdpSocket) -> io::Result<()> {
    let (used, capacity) = socket_budget(socket)?;
    queue_headroom(used, capacity)
}

fn socket_budget(socket: &::tokio::net::UdpSocket) -> io::Result<(i32, i32)> {
    let mut used: libc::c_int = 0;
    let mut capacity: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&capacity) as libc::socklen_t;
    // SAFETY: both calls write into live c_int values of the documented size.
    let (queued, sized) = unsafe {
        (
            libc::ioctl(socket.as_raw_fd(), libc::TIOCOUTQ, &mut used),
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&mut capacity as *mut libc::c_int).cast(),
                &mut length,
            ),
        )
    };
    if queued < 0 || sized < 0 || length as usize != std::mem::size_of_val(&capacity) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket budget unavailable",
        ));
    }
    Ok((used, capacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Transmit;
    use futures::FutureExt;

    #[test]
    fn sweep_yields_before_it_consumes_normal_ice_socket_headroom() {
        assert!(queue_headroom(0, 212992).is_ok());
        assert!(queue_headroom(48000, 212992).is_ok());
        for used in [50000, 53248, 106496, 213120] {
            assert_eq!(
                queue_headroom(used, 212992).unwrap_err().kind(),
                io::ErrorKind::WouldBlock,
                "probes must yield while ordinary ICE output still fits"
            );
        }
    }

    #[test]
    fn unusable_socket_budget_never_authorizes_a_probe() {
        for (used, capacity) in [(-1, 212992), (0, 0), (0, -1)] {
            assert!(queue_headroom(used, capacity).is_err());
        }
    }

    #[test]
    #[ignore = "requires socket-pressure.py disposable network namespace"]
    fn blocked_neighbors_leave_ordinary_ice_and_maximum_gso_send_immediately_ready() {
        assert_ne!(
            std::fs::read_link("/proc/self/ns/net").unwrap(),
            std::path::PathBuf::from(
                std::env::var("BUILD_RTC_PRESSURE_PARENT_NET_NS")
                    .expect("socket-pressure.py must supply the outer namespace")
            ),
            "this fixture must run in a disposable network namespace"
        );
        let runtime = crate::runtime::default_runtime().unwrap();
        let driver_runtime = runtime.clone();
        runtime.block_on(Box::pin(async move {
            let source = Ipv4Addr::new(10, 72, 0, 1);
            let target = SocketAddrV4::new(Ipv4Addr::new(10, 72, 0, 2), 40000);
            let bound = std::net::UdpSocket::bind((source, 0)).unwrap();
            bound.set_nonblocking(true).unwrap();
            let local = bound.local_addr().unwrap();
            let socket = driver_runtime.wrap_udp_socket(bound.try_clone().unwrap()).unwrap();
            let raw = ::tokio::net::UdpSocket::from_std(bound).unwrap();
            assert_eq!(socket.local_addr().unwrap(), raw.local_addr().unwrap());
            assert_eq!(socket.local_addr().unwrap().port(), local.port());
            assert!(socket.max_gso_segments() >= 64, "GSO unsupported in the test environment");
            socket.send_to(b"warm neighbor", target.into()).await.unwrap();
            driver_runtime.sleep(std::time::Duration::from_millis(100)).await;
            let name = std::ffi::CString::new("eth0").unwrap();
            // SAFETY: name is a terminated interface name in this private namespace.
            let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
            assert_ne!(index, 0);
            let payload = [0u8; 28]; // Same size as the credential-free production indication.
            let mut probes = 0;
            let mut yielded = false;
            for offset in 3..1023u32 {
                let unreachable = Ipv4Addr::from(u32::from(source) - 1 + offset);
                match send(&raw, &payload, source, index, SocketAddrV4::new(unreachable, 40000)) {
                    Ok(length) => {
                        assert_eq!(length, payload.len());
                        probes += 1;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        yielded = true;
                        break;
                    }
                    Err(error) => panic!("probe send failed: {error}"),
                }
                driver_runtime.sleep(std::time::Duration::from_millis(5)).await;
            }
            assert!(yielded && probes > 0, "the unanswered subnet must exercise pressure");
            let (queued, capacity) = socket_budget(&raw).unwrap();
            println!("probes_sent={probes}; send_queue={queued}; send_buffer={capacity}");
            let ordinary = [0u8; 112];
            let ordinary_started = std::time::Instant::now();
            assert!(
                matches!(socket.send_to(&ordinary, target.into()).now_or_never(), Some(Ok(112))),
                "probe pressure must not park an ordinary same-port ICE-sized send"
            );
            let ordinary_elapsed = ordinary_started.elapsed();
            // IPv4's aggregate UDP payload ceiling is lower than the generic
            // production batching bound; do not turn a size error into a pressure result.
            let maximum = vec![0u8; crate::peer_connection::transports::MAX_GSO_BATCH_BYTES.min(65507)];
            let transmit = Transmit {
                destination: target.into(),
                ecn: Some(crate::runtime::EcnCodepoint::Ect0),
                contents: &maximum,
                segment_size: Some(1024),
                src_ip: None,
            };
            let maximum_started = std::time::Instant::now();
            let maximum_poll = futures::future::poll_fn(|cx| socket.poll_send(cx, &transmit)).now_or_never();
            assert!(
                matches!(maximum_poll, Some(Ok(length)) if length == maximum.len()),
                "the maximum production GSO batch must remain immediately writable: {maximum_poll:?}"
            );
            println!("probes_sent={probes}; send_queue={queued}; send_buffer={capacity}; ordinary=112; ordinary_us={}; maximum_gso={}; maximum_gso_us={}; source_port_matches=true", ordinary_elapsed.as_micros(), maximum.len(), maximum_started.elapsed().as_micros());
        }));
    }
}

pub(super) fn send(
    socket: &::tokio::net::UdpSocket,
    payload: &[u8],
    source: Ipv4Addr,
    interface_index: u32,
    target: SocketAddrV4,
) -> io::Result<usize> {
    if interface_index == 0 || socket.local_addr()?.ip() != source {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "host source unavailable",
        ));
    }
    require_queue_headroom(socket)?;
    let destination = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: target.port().to_be(),
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes(target.ip().octets()),
        },
        sin_zero: [0; 8],
    };
    let information = libc::in_pktinfo {
        ipi_ifindex: interface_index as libc::c_int,
        ipi_spec_dst: libc::in_addr {
            s_addr: u32::from_ne_bytes(source.octets()),
        },
        ipi_addr: libc::in_addr { s_addr: 0 },
    };
    let mut control = [0usize; 8];
    let mut vector = libc::iovec {
        iov_base: payload.as_ptr().cast_mut().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: zero is a valid msghdr; all pointees remain alive through sendmsg.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = (&destination as *const libc::sockaddr_in).cast_mut().cast();
    message.msg_namelen = std::mem::size_of_val(&destination) as libc::socklen_t;
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: the usize-aligned buffer fits cmsghdr plus one in_pktinfo. sendmsg
    // reads initialized ancillary data and payload, without changing either.
    // DONTROUTE forbids gateway fallback; DONTWAIT permits one syscall without
    // parking the driver behind a socket while resolution/selection cancels.
    let sent = unsafe {
        message.msg_controllen =
            libc::CMSG_SPACE(std::mem::size_of_val(&information) as u32) as usize;
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::IPPROTO_IP;
        (*header).cmsg_type = libc::IP_PKTINFO;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&information) as u32) as usize;
        std::ptr::write_unaligned(
            libc::CMSG_DATA(header).cast::<libc::in_pktinfo>(),
            information,
        );
        libc::sendmsg(
            socket.as_raw_fd(),
            &message,
            libc::MSG_DONTROUTE | libc::MSG_DONTWAIT,
        )
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sent as usize)
}
