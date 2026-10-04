//! Select the LAN source explicitly for multicast and remembered-peer probes.

use std::io;
use std::net::SocketAddr;

use super::LanInterface;

#[cfg(target_os = "linux")]
pub(super) async fn send_on(
    socket: &tokio::net::UdpSocket,
    interface: &LanInterface,
    query: &[u8],
    destination: SocketAddr,
) -> io::Result<()> {
    socket
        .async_io(tokio::io::Interest::WRITABLE, || {
            send_packet_info(socket, interface, query, destination)
        })
        .await
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn send_on(
    socket: &tokio::net::UdpSocket,
    _: &LanInterface,
    query: &[u8],
    destination: SocketAddr,
) -> io::Result<()> {
    socket.send_to(query, destination).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn send_packet_info(
    socket: &tokio::net::UdpSocket,
    interface: &LanInterface,
    query: &[u8],
    destination: SocketAddr,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let destination = socket2::SockAddr::from(destination);
    let mut control = [0usize; 8];
    let mut buffer = libc::iovec {
        iov_base: query.as_ptr().cast_mut().cast(),
        iov_len: query.len(),
    };
    // SAFETY: zero initializes a valid msghdr; all referenced buffers live for
    // the sendmsg call, which reads rather than mutates the query bytes.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = destination.as_ptr().cast_mut().cast();
    message.msg_namelen = destination.len();
    message.msg_iov = &mut buffer;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    let information = libc::in_pktinfo {
        ipi_ifindex: interface.index as libc::c_int,
        ipi_spec_dst: libc::in_addr {
            s_addr: u32::from_ne_bytes(interface.address.octets()),
        },
        ipi_addr: libc::in_addr { s_addr: 0 },
    };
    // SAFETY: the usize-aligned control buffer has room for one cmsghdr and
    // in_pktinfo. The initialized ancillary message specifies egress and source.
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
        libc::sendmsg(socket.as_raw_fd(), &message, 0)
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    if sent as usize != query.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "incomplete mDNS datagram",
        ));
    }
    Ok(())
}
