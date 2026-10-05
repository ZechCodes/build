//! Linux-only explicit-interface, gateway-free sends on an existing ICE socket.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::AsRawFd;

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
