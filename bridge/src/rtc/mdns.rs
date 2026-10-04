//! Resolve browser mDNS candidates on every eligible LAN, independently of
//! ICE negotiation. Interface discovery happens for each resolution round.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use rtc::mdns::{Mdns, MdnsConfig, MDNS_DEST_ADDR, MDNS_MULTICAST_IPV4, MDNS_PORT};
use rtc::sansio::Protocol;
#[cfg(test)]
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const ANSWER_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
struct LanInterface {
    name: String,
    address: Ipv4Addr,
    netmask: Ipv4Addr,
    index: u32,
}

struct ReportedInterface {
    name: String,
    address: SocketAddr,
    netmask: Option<Ipv4Addr>,
    index: u32,
    point_to_point: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AnswerRejection {
    OffSubnet,
    UnqueriedInterface,
    NonLocalAddress,
    MissingArrival,
}

impl AnswerRejection {
    fn code(self) -> &'static str {
        match self {
            Self::OffSubnet => "off-subnet",
            Self::UnqueriedInterface => "unqueried-interface",
            Self::NonLocalAddress => "nonlocal-address",
            Self::MissingArrival => "arrival-interface-unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveFailure {
    InvalidName,
    InterfacesUnavailable,
    NoLanInterfaces,
    SocketUnavailable,
    AnswerRejected,
    Unresolved,
}

impl ResolveFailure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidName => "mdns-invalid-name",
            Self::InterfacesUnavailable => "mdns-interfaces-unavailable",
            Self::NoLanInterfaces => "mdns-no-lan-interfaces",
            Self::SocketUnavailable => "mdns-socket-unavailable",
            Self::AnswerRejected => "mdns-answer-rejected",
            Self::Unresolved => "mdns-unresolved",
        }
    }
}

#[async_trait]
trait Transport: Send + Sync {
    fn join(&self, interface: &LanInterface) -> io::Result<()>;
    async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()>;
    async fn receive(&self) -> io::Result<Datagram>;
}

struct UdpTransport {
    socket: tokio::net::UdpSocket,
    destination: SocketAddr,
}

impl UdpTransport {
    fn bound(port: u16) -> io::Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_reuse_address(true)?;
        #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.set_multicast_ttl_v4(255)?;
        enable_packet_info(&socket)?;
        // Linux's upstream group-address bind misses unicast mDNS responses.
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
        Ok(Self {
            socket: tokio::net::UdpSocket::from_std(socket.into())?,
            destination: MDNS_DEST_ADDR,
        })
    }

    async fn send_on(
        &self,
        interface: &LanInterface,
        query: &[u8],
        destination: SocketAddr,
    ) -> io::Result<()> {
        socket2::SockRef::from(&self.socket).set_multicast_if_v4(&interface.address)?;
        self.socket.send_to(query, destination).await?;
        Ok(())
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn join(&self, interface: &LanInterface) -> io::Result<()> {
        self.socket
            .join_multicast_v4(MDNS_MULTICAST_IPV4, interface.address)
    }

    async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()> {
        self.send_on(interface, query, self.destination).await
    }

    async fn receive(&self) -> io::Result<Datagram> {
        receive_with_packet_info(&self.socket).await
    }
}

#[derive(Debug, Clone)]
struct Datagram {
    packet: Vec<u8>,
    source: SocketAddr,
    arrival: Option<u32>,
}
type Received = Result<Datagram, ()>;

/// Concurrent queries share one receiver: the OS delivers unicast UDP to one
/// socket, whereas each pending name needs to see and filter that response.
struct SharedSocket {
    transport: Arc<UdpTransport>,
    replies: tokio::sync::broadcast::Sender<Received>,
    memberships: Mutex<HashSet<Ipv4Addr>>,
    send_lock: tokio::sync::Mutex<()>,
    receiver_task: tokio::task::JoinHandle<()>,
}

impl SharedSocket {
    fn acquire() -> io::Result<Arc<Self>> {
        static ACTIVE: OnceLock<Mutex<Weak<SharedSocket>>> = OnceLock::new();
        let mut active = ACTIVE
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| io::Error::other("mDNS socket lock unavailable"))?;
        if let Some(shared) = active.upgrade() {
            return Ok(shared);
        }
        let shared = Self::new(UdpTransport::bound(MDNS_PORT)?);
        *active = Arc::downgrade(&shared);
        Ok(shared)
    }

    fn new(transport: UdpTransport) -> Arc<Self> {
        let transport = Arc::new(transport);
        let (replies, _) = tokio::sync::broadcast::channel(128);
        let receiver_task = tokio::spawn(fan_out(transport.clone(), replies.clone()));
        Arc::new(Self {
            transport,
            replies,
            memberships: Mutex::default(),
            send_lock: tokio::sync::Mutex::new(()),
            receiver_task,
        })
    }

    fn subscribe(self: &Arc<Self>) -> SharedTransport {
        SharedTransport {
            socket: self.clone(),
            replies: tokio::sync::Mutex::new(self.replies.subscribe()),
        }
    }
}

impl Drop for SharedSocket {
    fn drop(&mut self) {
        // The receiver owns only the raw socket, so it cannot keep this owner
        // alive after the final resolution round ends.
        self.receiver_task.abort();
    }
}

async fn fan_out(
    transport: Arc<impl Transport + 'static>,
    replies: tokio::sync::broadcast::Sender<Received>,
) {
    loop {
        let received = receive_datagram(transport.as_ref()).await.map_err(|_| ());
        let failed = received.is_err();
        let _ = replies.send(received);
        if failed {
            return;
        }
    }
}

struct SharedTransport {
    socket: Arc<SharedSocket>,
    replies: tokio::sync::Mutex<tokio::sync::broadcast::Receiver<Received>>,
}

#[async_trait]
impl Transport for SharedTransport {
    fn join(&self, interface: &LanInterface) -> io::Result<()> {
        let mut memberships = self
            .socket
            .memberships
            .lock()
            .map_err(|_| io::Error::other("mDNS membership lock unavailable"))?;
        if !memberships.contains(&interface.address) {
            self.socket.transport.join(interface)?;
            memberships.insert(interface.address);
        }
        Ok(())
    }

    async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()> {
        let _sending = self.socket.send_lock.lock().await;
        self.socket.transport.send(interface, query).await
    }

    async fn receive(&self) -> io::Result<Datagram> {
        loop {
            match self.replies.lock().await.recv().await {
                Ok(Ok(packet)) => return Ok(packet),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                _ => return Err(socket_gone_error()),
            }
        }
    }
}

async fn receive_datagram(transport: &impl Transport) -> io::Result<Datagram> {
    loop {
        match transport.receive().await {
            Err(error) if !socket_gone(&error) => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            received => return received,
        }
    }
}

fn socket_gone(error: &io::Error) -> bool {
    #[cfg(unix)]
    let codes = [libc::EBADF, libc::ENOTSOCK];
    #[cfg(not(unix))]
    let codes = [10038]; // WSAENOTSOCK
    error
        .raw_os_error()
        .is_some_and(|code| codes.contains(&code))
}

fn socket_gone_error() -> io::Error {
    #[cfg(unix)]
    let code = libc::EBADF;
    #[cfg(not(unix))]
    let code = 10038;
    io::Error::from_raw_os_error(code)
}

#[cfg(target_os = "linux")]
fn enable_packet_info(socket: &socket2::Socket) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let enabled: libc::c_int = 1;
    // SAFETY: the socket is live and the option points to an initialized int
    // with its exact length. IP_PKTINFO supplies kernel arrival-interface data.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_PKTINFO,
            (&enabled as *const libc::c_int).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn enable_packet_info(_: &socket2::Socket) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
async fn receive_with_packet_info(socket: &tokio::net::UdpSocket) -> io::Result<Datagram> {
    socket
        .async_io(tokio::io::Interest::READABLE, || {
            receive_packet_info(socket)
        })
        .await
}

#[cfg(not(target_os = "linux"))]
async fn receive_with_packet_info(socket: &tokio::net::UdpSocket) -> io::Result<Datagram> {
    let mut packet = vec![0; 2048];
    let (length, source) = socket.recv_from(&mut packet).await?;
    packet.truncate(length);
    Ok(Datagram {
        packet,
        source,
        arrival: None,
    })
}

#[cfg(target_os = "linux")]
fn receive_packet_info(socket: &tokio::net::UdpSocket) -> io::Result<Datagram> {
    use std::os::fd::AsRawFd;
    let mut packet = vec![0; 2048];
    let mut control = [0usize; 8]; // aligned space for one cmsghdr + in_pktinfo
                                   // SAFETY: zero is valid for these C socket/message structs. All buffer
                                   // pointers remain live and exclusive during recvmsg; the socket is IPv4.
    let mut source: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    let mut buffer = libc::iovec {
        iov_base: packet.as_mut_ptr().cast(),
        iov_len: packet.len(),
    };
    message.msg_name = (&mut source as *mut libc::sockaddr_in).cast();
    message.msg_namelen = std::mem::size_of_val(&source) as libc::socklen_t;
    message.msg_iov = &mut buffer;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control);
    // SAFETY: the pointers and lengths above describe their initialized buffers.
    let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, 0) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    packet.truncate(length as usize);
    Ok(Datagram {
        packet,
        source: SocketAddr::from((
            Ipv4Addr::from(source.sin_addr.s_addr.to_ne_bytes()),
            u16::from_be(source.sin_port),
        )),
        arrival: arrival_index(&message),
    })
}

#[cfg(target_os = "linux")]
fn arrival_index(message: &libc::msghdr) -> Option<u32> {
    // SAFETY: recvmsg produced the ancillary data in the aligned live control
    // buffer. Check the type and complete payload length before reading it.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::IPPROTO_IP
                && (*header).cmsg_type == libc::IP_PKTINFO
                && (*header).cmsg_len
                    >= libc::CMSG_LEN(std::mem::size_of::<libc::in_pktinfo>() as u32) as usize
            {
                let info =
                    std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::in_pktinfo>());
                return (info.ipi_ifindex > 0).then_some(info.ipi_ifindex as u32);
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    None
}

#[cfg(unix)]
fn interface_index(name: &str) -> u32 {
    let Ok(name) = std::ffi::CString::new(name) else {
        return 0;
    };
    // SAFETY: name is a NUL-terminated OS interface name.
    unsafe { libc::if_nametoindex(name.as_ptr()) }
}

#[cfg(not(unix))]
fn interface_index(_: &str) -> u32 {
    0
}

#[cfg(unix)]
struct InterfaceList(*mut libc::ifaddrs);

#[cfg(unix)]
impl Drop for InterfaceList {
    fn drop(&mut self) {
        // SAFETY: this pointer came from getifaddrs and is freed exactly once.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

#[cfg(unix)]
fn point_to_point_names() -> io::Result<HashSet<String>> {
    let mut head = std::ptr::null_mut();
    // SAFETY: getifaddrs initializes head; the RAII guard owns its allocation.
    if unsafe { libc::getifaddrs(&mut head) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let list = InterfaceList(head);
    let mut current = list.0;
    let mut names = HashSet::new();
    while !current.is_null() {
        // SAFETY: this entry and its name are live until the guard frees them.
        let entry = unsafe { &*current };
        if entry.ifa_flags & libc::IFF_POINTOPOINT as u32 != 0 && !entry.ifa_name.is_null() {
            names.insert(
                unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        current = entry.ifa_next;
    }
    Ok(names)
}

#[cfg(not(unix))]
fn point_to_point_names() -> io::Result<HashSet<String>> {
    Ok(HashSet::new())
}

pub(crate) async fn resolve(
    name: &str,
    allowlist: Option<&[String]>,
    session_id: &str,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let name = normalized_name(name)?;
    let point_to_point =
        point_to_point_names().map_err(|_| ResolveFailure::InterfacesUnavailable)?;
    let reported = rtc::shared::ifaces::ifaces()
        .map_err(|_| ResolveFailure::InterfacesUnavailable)?
        .into_iter()
        .filter_map(|interface| {
            let index = interface_index(&interface.name);
            let point_to_point = point_to_point.contains(&interface.name)
                || matches!(
                    interface.hop,
                    Some(rtc::shared::ifaces::NextHop::Destination(_))
                );
            Some(ReportedInterface {
                name: interface.name,
                address: interface.addr?,
                netmask: interface.mask.and_then(|mask| match mask.ip() {
                    IpAddr::V4(address) => Some(address),
                    _ => None,
                }),
                index,
                point_to_point,
            })
        })
        .collect::<Vec<_>>();
    let interfaces = selected_interfaces(&reported, allowlist);
    if interfaces.is_empty() {
        return Err(ResolveFailure::NoLanInterfaces);
    }
    let shared = SharedSocket::acquire().map_err(|_| ResolveFailure::SocketUnavailable)?;
    let transport = shared.subscribe();
    resolve_with(&name, &interfaces, &transport, RESOLVE_TIMEOUT, session_id).await
}

fn selected_interfaces(
    reported: &[ReportedInterface],
    allowlist: Option<&[String]>,
) -> Vec<LanInterface> {
    let mut selected: Vec<LanInterface> = Vec::new();
    for reported in reported {
        let name = &reported.name;
        let Some(netmask) = reported.netmask.filter(|mask| valid_netmask(*mask)) else {
            continue;
        };
        let IpAddr::V4(address) = reported.address.ip() else {
            continue;
        };
        if reported.point_to_point || !eligible_interface(name, address, allowlist) {
            continue;
        }
        if selected
            .iter()
            .any(|interface| interface.address == address)
        {
            continue;
        }
        selected.push(LanInterface {
            name: name.clone(),
            address,
            netmask,
            index: reported.index,
        });
    }
    selected
}

fn eligible_interface(name: &str, address: Ipv4Addr, allowlist: Option<&[String]>) -> bool {
    if allowlist.is_some_and(|names| !names.iter().any(|allowed| allowed == name)) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    let excluded = [
        "docker",
        "veth",
        "loopback",
        "tun",
        "utun",
        "wg",
        "tailscale",
        "zt",
        "cni-",
        "podman",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));
    let loopback = lower == "lo" || lower.starts_with("lo0");
    !excluded && !docker_bridge(&lower) && !loopback && usable_ipv4(address)
}

fn docker_bridge(name: &str) -> bool {
    name.strip_prefix("br-").is_some_and(|network_id| {
        network_id.len() == 12 && network_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn usable_ipv4(address: Ipv4Addr) -> bool {
    !address.is_loopback() && address.octets()[0] != 0 && address.octets()[0] < 224
}

fn valid_netmask(netmask: Ipv4Addr) -> bool {
    let mask = u32::from(netmask);
    let host_bits = !mask;
    mask != 0 && host_bits & host_bits.wrapping_add(1) == 0
}

impl LanInterface {
    fn contains(&self, address: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        let host_bits = !mask;
        let host = u32::from(address) & host_bits;
        valid_netmask(self.netmask)
            && u32::from(address) & mask == u32::from(self.address) & mask
            && (host_bits <= 1 || (host != 0 && host != host_bits))
    }
}

fn local_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    address.is_private()
        || address.is_link_local()
        || (octets[0] == 100 && (64..128).contains(&octets[1]))
}

pub(super) fn normalized_name(name: &str) -> Result<String, ResolveFailure> {
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let label = name
        .strip_suffix(".local")
        .ok_or(ResolveFailure::InvalidName)?;
    let uuid = uuid::Uuid::parse_str(label).map_err(|_| ResolveFailure::InvalidName)?;
    if label != uuid.to_string() {
        return Err(ResolveFailure::InvalidName);
    }
    Ok(name)
}

fn query_bytes(name: &str) -> Vec<u8> {
    let mut protocol = Mdns::new(MdnsConfig::default());
    protocol.query(name);
    protocol
        .poll_write()
        .expect("validated UUID.local query")
        .message
        .to_vec()
}

#[cfg(test)]
fn answer_from(name: &str, packet: &[u8], source: SocketAddr) -> Option<IpAddr> {
    decoded_answer(name, packet, source)
        .filter(|address| matches!(address, IpAddr::V4(address) if usable_ipv4(*address)))
}

fn decoded_answer(name: &str, packet: &[u8], _: SocketAddr) -> Option<IpAddr> {
    // The upstream private decoder reports an AAAA record as its IPv4 sender.
    // Read only IN/A RDATA here; a sender address is never an advertised answer.
    let mut cursor = DnsCursor { packet, offset: 0 };
    let header = cursor.take(12)?;
    let flags = u16::from_be_bytes([header[2], header[3]]);
    if flags & 0xf80f != 0x8000 {
        return None; // response, standard opcode, successful result
    }
    let questions = u16::from_be_bytes([header[4], header[5]]);
    let answers = u16::from_be_bytes([header[6], header[7]]);
    for _ in 0..questions {
        cursor.name()?;
        cursor.take(4)?;
    }
    for _ in 0..answers {
        let answer = cursor.answer()?;
        if answer.kind == 1
            && answer.class & 0x7fff == 1
            && answer.name.eq_ignore_ascii_case(name.as_bytes())
        {
            if let Ok(octets) = <[u8; 4]>::try_from(answer.data) {
                return Some(Ipv4Addr::from(octets).into());
            }
        }
    }
    None
}

struct DnsCursor<'a> {
    packet: &'a [u8],
    offset: usize,
}

struct DnsAnswer<'a> {
    name: Vec<u8>,
    kind: u16,
    class: u16,
    data: &'a [u8],
}

impl<'a> DnsCursor<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(length)?;
        let bytes = self.packet.get(self.offset..end)?;
        self.offset = end;
        Some(bytes)
    }

    fn word(&mut self) -> Option<u16> {
        let bytes = self.take(2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn name(&mut self) -> Option<Vec<u8>> {
        let (name, end) = dns_name(self.packet, self.offset)?;
        self.offset = end;
        Some(name)
    }

    fn answer(&mut self) -> Option<DnsAnswer<'a>> {
        let name = self.name()?;
        let kind = self.word()?;
        let class = self.word()?;
        self.take(4)?; // TTL
        let length = usize::from(self.word()?);
        let data = self.take(length)?;
        Some(DnsAnswer {
            name,
            kind,
            class,
            data,
        })
    }
}

enum NamePart<'a> {
    Label(&'a [u8], usize),
    Pointer(usize, usize),
    End(usize),
}

fn name_part(packet: &[u8], offset: usize) -> Option<NamePart<'_>> {
    let first = *packet.get(offset)?;
    match first {
        0 => Some(NamePart::End(offset + 1)),
        1..=63 => {
            let end = offset.checked_add(1 + usize::from(first))?;
            Some(NamePart::Label(packet.get(offset + 1..end)?, end))
        }
        192..=255 => {
            let second = *packet.get(offset + 1)?;
            let target = usize::from(u16::from_be_bytes([first & 0x3f, second]));
            (target < offset && target >= 12).then_some(NamePart::Pointer(target, offset + 2))
        }
        _ => None,
    }
}

fn dns_name(packet: &[u8], mut offset: usize) -> Option<(Vec<u8>, usize)> {
    let mut name = Vec::new();
    let mut end = None;
    // Both decoded name length and compression traversal are bounded.
    for _ in 0..128 {
        match name_part(packet, offset)? {
            NamePart::Label(label, next) => {
                if label.contains(&b'.') {
                    return None; // a dot inside one label cannot match UUID.local
                }
                if !name.is_empty() {
                    name.push(b'.');
                }
                name.extend_from_slice(label);
                if name.len() > 253 {
                    return None;
                }
                offset = next;
            }
            NamePart::Pointer(target, next) => {
                end.get_or_insert(next);
                offset = target;
            }
            NamePart::End(next) => return Some((name, end.unwrap_or(next))),
        }
    }
    None
}

fn validated_answer(
    name: &str,
    packet: &[u8],
    source: SocketAddr,
    arrival: Option<u32>,
    interfaces: &[LanInterface],
) -> Result<Option<IpAddr>, AnswerRejection> {
    validated_answer_for_platform(
        name,
        packet,
        source,
        arrival,
        interfaces,
        cfg!(target_os = "linux"),
    )
}

fn validated_answer_for_platform(
    name: &str,
    packet: &[u8],
    source: SocketAddr,
    arrival: Option<u32>,
    interfaces: &[LanInterface],
    require_arrival: bool,
) -> Result<Option<IpAddr>, AnswerRejection> {
    let Some(IpAddr::V4(address)) = decoded_answer(name, packet, source) else {
        return Ok(None);
    };
    if require_arrival && arrival.is_none() {
        return Err(AnswerRejection::MissingArrival);
    }
    if !usable_ipv4(address) || (arrival.is_none() && !local_ipv4(address)) {
        return Err(AnswerRejection::NonLocalAddress);
    }
    let receiving = interfaces
        .iter()
        .filter(|interface| arrival.is_none_or(|index| interface.index == index))
        .collect::<Vec<_>>();
    if receiving.is_empty() {
        return Err(AnswerRejection::UnqueriedInterface);
    }
    let on_lan = receiving.iter().any(|interface| {
        matches!(source.ip(), IpAddr::V4(source) if interface.contains(source) && interface.contains(address))
    });
    if !on_lan {
        return Err(AnswerRejection::OffSubnet);
    }
    Ok(Some(address.into()))
}

async fn resolve_with(
    name: &str,
    interfaces: &[LanInterface],
    transport: &impl Transport,
    timeout: Duration,
    session_id: &str,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    if interfaces.is_empty() {
        return Err(ResolveFailure::NoLanInterfaces);
    }
    let joined = interfaces
        .iter()
        .filter(|interface| transport.join(interface).is_ok())
        .cloned()
        .collect::<Vec<_>>();
    super::diagnostic_debug(
        session_id,
        &format!(
            "mdns-query eligible_interfaces={} joined_interfaces={} failed_interfaces={}",
            interfaces.len(),
            joined.len(),
            interfaces.len() - joined.len()
        ),
    );
    if joined.is_empty() {
        return Err(ResolveFailure::SocketUnavailable);
    }
    query_round(name, &joined, transport, session_id, deadline).await
}

async fn send_queries(
    interfaces: &[LanInterface],
    transport: &impl Transport,
    query: &[u8],
    deadline: tokio::time::Instant,
) -> usize {
    let mut successes = 0;
    for interface in interfaces {
        if matches!(
            tokio::time::timeout_at(deadline, transport.send(interface, query)).await,
            Ok(Ok(()))
        ) {
            successes += 1;
        }
    }
    successes
}

async fn query_round(
    name: &str,
    interfaces: &[LanInterface],
    transport: &impl Transport,
    session_id: &str,
    deadline: tokio::time::Instant,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let query = query_bytes(name);
    let successes = send_queries(interfaces, transport, &query, deadline).await;
    super::diagnostic_debug(
        session_id,
        &format!(
            "mdns-query sent_interfaces={successes} failed_sends={}",
            interfaces.len() - successes
        ),
    );
    if successes == 0 {
        return Err(ResolveFailure::SocketUnavailable);
    }
    receive_answers(name, interfaces, transport, &query, deadline, session_id).await
}

async fn receive_answers(
    name: &str,
    interfaces: &[LanInterface],
    transport: &impl Transport,
    query: &[u8],
    deadline: tokio::time::Instant,
    session_id: &str,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let mut answers = Vec::new();
    let mut answer_deadline = None;
    let mut rejected = HashSet::new();
    let mut retry =
        tokio::time::interval_at(tokio::time::Instant::now() + RETRY_INTERVAL, RETRY_INTERVAL);
    loop {
        let wake = answer_deadline.unwrap_or(deadline).min(deadline);
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(wake) => return resolved_or(answers, rejection_failure(&rejected)),
            _ = retry.tick() => { send_queries(interfaces, transport, query, deadline).await; }
            received = receive_datagram(transport) => {
                let Ok(datagram) = received else {
                    return resolved_or(answers, ResolveFailure::SocketUnavailable);
                };
                if let Some(address) = accept_answer(name, &datagram, interfaces, &mut rejected, session_id) {
                    if !answers.contains(&address) {
                        answers.push(address);
                    }
                    answer_deadline.get_or_insert_with(|| tokio::time::Instant::now() + ANSWER_GRACE);
                }
            }
        }
    }
}

fn rejection_failure(rejected: &HashSet<AnswerRejection>) -> ResolveFailure {
    if rejected.is_empty() {
        ResolveFailure::Unresolved
    } else {
        ResolveFailure::AnswerRejected
    }
}

fn accept_answer(
    name: &str,
    datagram: &Datagram,
    interfaces: &[LanInterface],
    rejected: &mut HashSet<AnswerRejection>,
    session_id: &str,
) -> Option<IpAddr> {
    match validated_answer(
        name,
        &datagram.packet,
        datagram.source,
        datagram.arrival,
        interfaces,
    ) {
        Ok(address) => address,
        Err(reason) => {
            if rejected.insert(reason) {
                super::diagnostic(
                    session_id,
                    &format!("mdns-answer-rejected reason={}", reason.code()),
                );
            }
            None
        }
    }
}

fn resolved_or(
    answers: Vec<IpAddr>,
    failure: ResolveFailure,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    if answers.is_empty() {
        Err(failure)
    } else {
        Ok(answers)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use super::*;

    const NAME: &str = "4c14a372-9db5-4faa-bbf1-d93583114e89.local";

    fn reported(entries: &[(&str, &str)]) -> Vec<ReportedInterface> {
        entries
            .iter()
            .map(|(name, address)| ReportedInterface {
                name: name.to_string(),
                address: address.parse().unwrap(),
                netmask: Some(mask_for(address)),
                index: index_for(name),
                point_to_point: false,
            })
            .collect()
    }

    fn interface(name: &str, address: &str) -> LanInterface {
        LanInterface {
            name: name.to_string(),
            address: address.parse().unwrap(),
            netmask: mask_for(address),
            index: index_for(name),
        }
    }

    fn mask_for(address: &str) -> Ipv4Addr {
        if address.starts_with("169.254.") {
            Ipv4Addr::new(255, 255, 0, 0)
        } else {
            Ipv4Addr::new(255, 255, 255, 0)
        }
    }

    fn index_for(name: &str) -> u32 {
        if name == "wlan0" {
            2
        } else {
            1
        }
    }

    #[test]
    fn mdns_answers_must_belong_to_the_subnet_of_the_receiving_interface() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let source = "192.168.1.50:5353".parse().unwrap();
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.1.60"),
                source,
                Some(1),
                &interfaces
            ),
            Ok(Some("192.168.1.60".parse().unwrap()))
        );
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.68.50"),
                source,
                Some(1),
                &interfaces
            ),
            Err(AnswerRejection::OffSubnet)
        );
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "8.8.8.8"),
                source,
                Some(1),
                &interfaces
            ),
            Err(AnswerRejection::OffSubnet)
        );
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.1.60"),
                source,
                Some(2),
                &interfaces
            ),
            Err(AnswerRejection::OffSubnet)
        );
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.1.60"),
                source,
                Some(99),
                &interfaces
            ),
            Err(AnswerRejection::UnqueriedInterface)
        );
    }

    #[test]
    fn mdns_without_arrival_metadata_requires_a_local_address_in_the_source_lan() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let source = "192.168.1.50:5353".parse().unwrap();
        assert_eq!(
            validated_answer_for_platform(
                NAME,
                &response(NAME, "192.168.1.60"),
                source,
                None,
                &interfaces,
                false
            ),
            Ok(Some("192.168.1.60".parse().unwrap()))
        );
        assert_eq!(
            validated_answer_for_platform(
                NAME,
                &response(NAME, "192.168.68.50"),
                source,
                None,
                &interfaces,
                false
            ),
            Err(AnswerRejection::OffSubnet)
        );
        let public = vec![interface("eth0", "8.8.8.10")];
        assert_eq!(
            validated_answer_for_platform(
                NAME,
                &response(NAME, "8.8.8.8"),
                "8.8.8.50:5353".parse().unwrap(),
                None,
                &public,
                false
            ),
            Err(AnswerRejection::NonLocalAddress)
        );
        assert_eq!(
            validated_answer_for_platform(
                NAME,
                &response(NAME, "8.8.8.8"),
                "8.8.8.50:5353".parse().unwrap(),
                Some(1),
                &public,
                false
            ),
            Ok(Some("8.8.8.8".parse().unwrap()))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rejects_missing_arrival_metadata_even_for_an_on_subnet_answer() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.1.60"),
                "192.168.1.50:5353".parse().unwrap(),
                None,
                &interfaces
            ),
            Err(AnswerRejection::MissingArrival)
        );
    }

    #[test]
    fn fallback_accepts_private_link_local_and_cgnat_hosts_on_the_source_lan() {
        for (local, peer) in [
            ("10.2.3.10", "10.2.3.20"),
            ("172.16.3.10", "172.16.3.20"),
            ("192.168.3.10", "192.168.3.20"),
            ("169.254.3.10", "169.254.3.20"),
            ("100.64.3.10", "100.64.3.20"),
        ] {
            let interfaces = vec![interface("eth0", local)];
            let source = SocketAddr::new(peer.parse().unwrap(), MDNS_PORT);
            assert_eq!(
                validated_answer_for_platform(
                    NAME,
                    &response(NAME, peer),
                    source,
                    None,
                    &interfaces,
                    false
                ),
                Ok(Some(peer.parse().unwrap()))
            );
        }
    }

    #[test]
    fn invalid_and_directed_broadcast_answers_cannot_enter_a_lan_pair() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let source = "192.168.1.50:5353".parse().unwrap();
        for address in ["127.0.0.1", "0.0.0.0", "224.0.0.251", "255.255.255.255"] {
            assert_eq!(
                validated_answer(NAME, &response(NAME, address), source, Some(1), &interfaces),
                Err(AnswerRejection::NonLocalAddress)
            );
        }
        for address in ["192.168.1.0", "192.168.1.255"] {
            assert_eq!(
                validated_answer(NAME, &response(NAME, address), source, Some(1), &interfaces),
                Err(AnswerRejection::OffSubnet)
            );
        }
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.1.60"),
                "192.168.68.50:5353".parse().unwrap(),
                Some(1),
                &interfaces
            ),
            Err(AnswerRejection::OffSubnet)
        );
    }

    #[test]
    fn mdns_subnet_boundaries_use_the_actual_netmask() {
        let mut lan = interface("eth0", "192.168.68.10");
        lan.netmask = "255.255.252.0".parse().unwrap();
        let source = "192.168.70.50:5353".parse().unwrap();
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.71.60"),
                source,
                Some(1),
                &[lan.clone()]
            ),
            Ok(Some("192.168.71.60".parse().unwrap()))
        );
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "192.168.72.60"),
                source,
                Some(1),
                &[lan]
            ),
            Err(AnswerRejection::OffSubnet)
        );
    }

    #[test]
    fn tunnel_container_and_point_to_point_interfaces_are_not_lans() {
        for name in [
            "tun0",
            "utun2",
            "wg0",
            "tailscale0",
            "ztabcdef",
            "cni-pod0",
            "podman0",
        ] {
            assert!(
                selected_interfaces(&reported(&[(name, "192.168.1.10:0")]), None).is_empty(),
                "{name}"
            );
        }
        let mut entries = reported(&[("eth0", "192.168.1.10:0")]);
        entries[0].point_to_point = true;
        assert!(selected_interfaces(&entries, None).is_empty());
        entries[0].point_to_point = false;
        entries[0].netmask = None;
        assert!(selected_interfaces(&entries, None).is_empty());
    }

    #[test]
    fn missing_default_and_noncontiguous_masks_are_not_usable_lans() {
        for mask in [
            None,
            Some(Ipv4Addr::UNSPECIFIED),
            Some(Ipv4Addr::new(255, 0, 255, 0)),
        ] {
            let mut entries = reported(&[("eth0", "192.168.1.10:0")]);
            entries[0].netmask = mask;
            assert!(selected_interfaces(&entries, None).is_empty(), "{mask:?}");
        }
    }

    fn tagged(packet: &[u8], source: SocketAddr) -> TaggedBytesMut {
        TaggedBytesMut {
            now: std::time::Instant::now(),
            transport: TransportContext {
                local_addr: MDNS_DEST_ADDR,
                peer_addr: source,
                transport_protocol: TransportProtocol::UDP,
                ecn: None,
            },
            message: packet.into(),
        }
    }

    /// Generate fixtures through the same upstream DNS encoder used in ICE.
    fn response(name: &str, address: &str) -> Vec<u8> {
        if address.parse::<IpAddr>().unwrap().is_ipv6() {
            return response_records(&[(name, address)]);
        }
        let mut server = Mdns::new(
            MdnsConfig::default()
                .with_local_names(vec![name.to_string()])
                .with_local_ip(address.parse().unwrap()),
        );
        let mut client = Mdns::new(MdnsConfig::default());
        client.query(name);
        let packet = client.poll_write().unwrap();
        server
            .handle_read(tagged(&packet.message, "192.168.1.1:5353".parse().unwrap()))
            .unwrap();
        server.poll_write().unwrap().message.to_vec()
    }

    fn response_records(records: &[(&str, &str)]) -> Vec<u8> {
        let mut packet = vec![0, 0, 0x84, 0, 0, 0];
        packet.extend_from_slice(&(records.len() as u16).to_be_bytes());
        packet.extend_from_slice(&[0; 4]);
        for (index, (name, address)) in records.iter().enumerate() {
            if index > 0 && *name == records[0].0 {
                packet.extend_from_slice(&[0xc0, 0x0c]); // first answer's name
            } else {
                for label in name.split('.') {
                    packet.push(label.len() as u8);
                    packet.extend_from_slice(label.as_bytes());
                }
                packet.push(0);
            }
            let (kind, data): (u16, Vec<u8>) = match address.parse::<IpAddr>().unwrap() {
                IpAddr::V4(address) => (1, address.octets().to_vec()),
                IpAddr::V6(address) => (28, address.octets().to_vec()),
            };
            packet.extend_from_slice(&kind.to_be_bytes());
            packet.extend_from_slice(&1u16.to_be_bytes());
            packet.extend_from_slice(&120u32.to_be_bytes());
            packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
            packet.extend_from_slice(&data);
        }
        packet
    }

    #[test]
    fn an_aaaa_answer_does_not_invent_an_ipv4_candidate_from_its_sender() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        assert_eq!(
            validated_answer(
                NAME,
                &response(NAME, "fd00::123"),
                "192.168.1.50:5353".parse().unwrap(),
                Some(1),
                &interfaces
            ),
            Ok(None)
        );
    }

    #[test]
    fn a_compressed_a_answer_after_an_aaaa_answer_uses_its_actual_rdata() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let packet = response_records(&[(NAME, "fd00::123"), (NAME, "192.168.1.60")]);
        assert_eq!(
            validated_answer(
                NAME,
                &packet,
                "192.168.1.50:5353".parse().unwrap(),
                Some(1),
                &interfaces
            ),
            Ok(Some("192.168.1.60".parse().unwrap()))
        );
    }

    #[test]
    fn truncated_or_malformed_a_records_do_not_produce_candidates() {
        let packet = response_records(&[(NAME, "192.168.1.60")]);
        let source = "192.168.1.50:5353".parse().unwrap();
        for length in 0..packet.len() {
            assert_eq!(
                decoded_answer(NAME, &packet[..length], source),
                None,
                "length={length}"
            );
        }
        let record = 12 + NAME.len() + 2;
        for (length, data) in [(3u16, vec![192, 168, 1]), (5, vec![192, 168, 1, 60, 0])] {
            let mut invalid = packet[..record + 8].to_vec();
            invalid.extend_from_slice(&length.to_be_bytes());
            invalid.extend_from_slice(&data);
            assert_eq!(decoded_answer(NAME, &invalid, source), None);
        }
        for (offset, value) in [
            (record, 2u16),
            (record, 28),
            (record + 2, 3),
            (2, 0x8800),
            (2, 0x8403),
        ] {
            let mut invalid = packet.clone();
            invalid[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
            assert_eq!(
                decoded_answer(NAME, &invalid, source),
                None,
                "offset={offset} value={value}"
            );
        }
        let mut cache_flush = packet.clone();
        cache_flush[record + 2..record + 4].copy_from_slice(&0x8001u16.to_be_bytes());
        assert_eq!(
            decoded_answer(NAME, &cache_flush, source),
            Some("192.168.1.60".parse().unwrap())
        );
        assert_eq!(decoded_answer(NAME, &query_bytes(NAME), source), None);
        assert_eq!(
            decoded_answer(
                NAME,
                &response_records(&[("other.local", "192.168.1.60")]),
                source
            ),
            None
        );
    }

    #[test]
    fn malformed_compression_pointers_do_not_produce_candidates() {
        let packet = response_records(&[(NAME, "192.168.1.60")]);
        let record = 12 + NAME.len() + 2;
        let source = "192.168.1.50:5353".parse().unwrap();
        for encoded_name in [
            vec![0xc0, 0x0c],          // points to itself
            vec![0xc0, 0xff],          // outside packet
            vec![0xc0, 0x20],          // forward pointer
            vec![1, b'a', 0xc0, 0x0c], // label and backwards pointer cycle
            vec![0xc0],                // truncated pointer
        ] {
            let mut invalid = packet[..12].to_vec();
            invalid.extend_from_slice(&encoded_name);
            invalid.extend_from_slice(&packet[record..]);
            assert_eq!(
                decoded_answer(NAME, &invalid, source),
                None,
                "name={encoded_name:?}"
            );
        }
        assert_eq!(dns_name(&[0; 12], usize::MAX), None);
        assert_eq!(dns_name(&[0; 12], 12), None);
    }

    #[test]
    fn one_dns_label_cannot_impersonate_the_two_browser_name_labels() {
        let packet = response_records(&[(NAME, "192.168.1.60")]);
        let record = 12 + NAME.len() + 2;
        let mut invalid = packet[..12].to_vec();
        invalid.push(NAME.len() as u8);
        invalid.extend_from_slice(NAME.as_bytes());
        invalid.push(0);
        invalid.extend_from_slice(&packet[record..]);
        assert_eq!(
            decoded_answer(NAME, &invalid, "192.168.1.50:5353".parse().unwrap()),
            None
        );
    }

    struct FakeTransport {
        joined: Mutex<Vec<Ipv4Addr>>,
        sent: Mutex<Vec<Ipv4Addr>>,
        failed_join: Option<Ipv4Addr>,
        failed_send: Option<Ipv4Addr>,
        reply_delay: Duration,
        replies: HashMap<Ipv4Addr, Vec<u8>>,
        receive_errors: Mutex<VecDeque<io::Error>>,
        reply_sources: HashMap<Ipv4Addr, SocketAddr>,
        sender: tokio::sync::mpsc::UnboundedSender<Datagram>,
        receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Datagram>>,
    }

    #[tokio::test(start_paused = true)]
    async fn an_aaaa_only_round_never_reports_resolution_to_its_ipv4_sender() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let mut transport = FakeTransport::new(HashMap::from([(
            interfaces[0].address,
            response(NAME, "fd00::123"),
        )]));
        transport
            .reply_sources
            .insert(interfaces[0].address, "192.168.1.50:5353".parse().unwrap());
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test").await,
            Err(ResolveFailure::Unresolved)
        );
    }

    impl FakeTransport {
        fn new(replies: HashMap<Ipv4Addr, Vec<u8>>) -> Self {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            Self {
                joined: Mutex::new(Vec::new()),
                sent: Mutex::new(Vec::new()),
                failed_join: None,
                failed_send: None,
                reply_delay: Duration::ZERO,
                replies,
                receive_errors: Mutex::default(),
                reply_sources: HashMap::new(),
                sender,
                receiver: tokio::sync::Mutex::new(receiver),
            }
        }
    }

    #[async_trait]
    impl Transport for FakeTransport {
        fn join(&self, interface: &LanInterface) -> io::Result<()> {
            self.joined.lock().unwrap().push(interface.address);
            if self.failed_join == Some(interface.address) {
                return Err(io::Error::other("synthetic membership failure"));
            }
            Ok(())
        }

        async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()> {
            self.sent.lock().unwrap().push(interface.address);
            assert_eq!(query, query_bytes(NAME));
            if self.failed_send == Some(interface.address) {
                return Err(io::Error::other("synthetic egress failure"));
            }
            if let Some(reply) = self.replies.get(&interface.address) {
                let response = Datagram {
                    packet: reply.clone(),
                    source: self
                        .reply_sources
                        .get(&interface.address)
                        .copied()
                        .unwrap_or_else(|| {
                            SocketAddr::new(
                                answer_from(NAME, reply, "192.168.1.50:5353".parse().unwrap())
                                    .unwrap(),
                                MDNS_PORT,
                            )
                        }),
                    arrival: Some(interface.index),
                };
                if self.reply_delay.is_zero() {
                    self.sender.send(response).unwrap();
                } else {
                    let sender = self.sender.clone();
                    let delay = self.reply_delay;
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = sender.send(response);
                    });
                }
            }
            Ok(())
        }

        async fn receive(&self) -> io::Result<Datagram> {
            if let Some(error) = self.receive_errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            self.receiver
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| io::Error::other("synthetic receiver closed"))
        }
    }

    #[test]
    fn real_lan_addresses_skip_container_loopback_and_invalid_addresses() {
        let interfaces = selected_interfaces(
            &reported(&[
                ("docker0", "172.17.0.1:0"),
                ("docker-test", "172.18.0.1:0"),
                ("br-7964352a61a0", "172.19.0.1:0"),
                ("veth9c", "172.20.0.1:0"),
                ("lo", "127.0.0.1:0"),
                ("lo0", "192.168.1.99:0"),
                ("loopback", "192.168.1.98:0"),
                ("eth0", "0.0.0.0:0"),
                ("eth0", "224.0.0.4:0"),
                ("eth0", "255.255.255.255:0"),
                ("eth0", "169.254.1.10:0"),
                ("eth0", "192.168.1.10:0"),
                ("wlan0", "192.168.68.2:0"),
                ("en0", "[fd00::1]:0"),
            ]),
            None,
        );
        assert_eq!(
            interfaces,
            vec![
                interface("eth0", "169.254.1.10"),
                interface("eth0", "192.168.1.10"),
                interface("wlan0", "192.168.68.2")
            ]
        );
    }

    #[test]
    fn addresses_are_deduplicated_and_allowlists_do_not_expand_to_other_interfaces() {
        let addresses = reported(&[
            ("eth0", "192.168.1.10:0"),
            ("eth0", "192.168.1.10:123"),
            ("eth1", "192.168.1.10:0"),
            ("wlan0", "192.168.68.2:0"),
        ]);
        assert_eq!(selected_interfaces(&addresses, None).len(), 2);
        let allowed = vec!["wlan0".to_string()];
        assert_eq!(
            selected_interfaces(&addresses, Some(&allowed)),
            vec![interface("wlan0", "192.168.68.2")]
        );
        assert!(selected_interfaces(&addresses, Some(&[])).is_empty());
    }

    #[test]
    fn physical_bridges_are_queried_but_docker_bridge_ids_are_skipped() {
        assert_eq!(
            selected_interfaces(
                &reported(&[
                    ("br-7964352a61a0", "172.19.0.1:0"),
                    ("br-lan", "192.168.68.2:0"),
                    ("br-eth0", "192.168.1.10:0"),
                ]),
                None
            ),
            vec![
                interface("br-lan", "192.168.68.2"),
                interface("br-eth0", "192.168.1.10")
            ]
        );
    }

    #[test]
    fn an_interface_address_change_is_seen_by_the_next_selection() {
        let allowlist = vec!["eth0".to_string()];
        assert_eq!(
            selected_interfaces(&reported(&[("eth0", "192.168.1.10:0")]), Some(&allowlist)),
            vec![interface("eth0", "192.168.1.10")]
        );
        assert_eq!(
            selected_interfaces(&reported(&[("eth0", "192.168.68.2:0")]), Some(&allowlist)),
            vec![interface("eth0", "192.168.68.2")]
        );
    }

    #[test]
    fn browser_names_are_uuid_local_only_and_dns_case_is_normalized() {
        assert_eq!(normalized_name(&NAME.to_uppercase()).unwrap(), NAME);
        assert_eq!(normalized_name(&format!("{NAME}.")).unwrap(), NAME);
        for invalid in [
            "printer.local",
            "4c14a372-9db5-4faa-bbf1-d93583114e89.example.com",
            "4c14a372-9db5-4faa-bbf1-d93583114e89.extra.local",
            "../../secret.local",
            "4c14a372-9db5-4faa-bbf1-d93583114e89.local..",
        ] {
            assert_eq!(normalized_name(invalid), Err(ResolveFailure::InvalidName));
        }
    }

    #[test]
    fn only_the_queried_name_and_a_usable_address_are_accepted() {
        let source = "192.168.1.50:5353".parse().unwrap();
        assert_eq!(
            answer_from(NAME, &response(NAME, "192.168.1.50"), source),
            Some("192.168.1.50".parse().unwrap())
        );
        assert_eq!(
            answer_from(NAME, &response("other.local", "192.168.1.50"), source),
            None
        );
        assert_eq!(answer_from(NAME, &[0, 1, 2], source), None);
        for invalid in ["0.0.0.0", "127.0.0.1", "224.0.0.251", "255.255.255.255"] {
            assert_eq!(answer_from(NAME, &response(NAME, invalid), source), None);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_interface_receives_the_query_and_answers_are_deduplicated() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let transport = FakeTransport::new(HashMap::from([
            (interfaces[0].address, response(NAME, "192.168.1.50")),
            (interfaces[1].address, response(NAME, "192.168.1.50")),
        ]));
        let resolved = resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
            .await
            .unwrap();
        assert_eq!(resolved, vec!["192.168.1.50".parse::<IpAddr>().unwrap()]);
        assert_eq!(
            *transport.joined.lock().unwrap(),
            vec![interfaces[0].address, interfaces[1].address]
        );
        assert_eq!(
            *transport.sent.lock().unwrap(),
            vec![interfaces[0].address, interfaces[1].address]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn answers_from_different_lans_are_collected() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let transport = FakeTransport::new(HashMap::from([
            (interfaces[0].address, response(NAME, "192.168.1.50")),
            (interfaces[1].address, response(NAME, "192.168.68.50")),
        ]));
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .unwrap(),
            vec![
                "192.168.1.50".parse::<IpAddr>().unwrap(),
                "192.168.68.50".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_first_membership_does_not_hide_the_second_lan() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let mut transport = FakeTransport::new(HashMap::from([(
            interfaces[1].address,
            response(NAME, "192.168.68.50"),
        )]));
        transport.failed_join = Some(interfaces[0].address);
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .unwrap(),
            vec!["192.168.68.50".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(*transport.sent.lock().unwrap(), vec![interfaces[1].address]);
    }

    #[tokio::test(start_paused = true)]
    async fn link_local_lans_and_answers_remain_available_without_dhcp() {
        let interfaces = selected_interfaces(&reported(&[("en0", "169.254.10.2:0")]), None);
        assert_eq!(interfaces, vec![interface("en0", "169.254.10.2")]);
        let transport = FakeTransport::new(HashMap::from([(
            interfaces[0].address,
            response(NAME, "169.254.10.3"),
        )]));
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .unwrap(),
            vec!["169.254.10.3".parse::<IpAddr>().unwrap()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_first_send_does_not_hide_the_second_lan() {
        let interfaces = vec![
            interface("eth0", "192.168.1.10"),
            interface("wlan0", "192.168.68.2"),
        ];
        let mut transport = FakeTransport::new(HashMap::from([(
            interfaces[1].address,
            response(NAME, "192.168.68.50"),
        )]));
        transport.failed_send = Some(interfaces[0].address);
        assert!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .is_ok()
        );
        assert_eq!(
            *transport.sent.lock().unwrap(),
            vec![interfaces[0].address, interfaces[1].address]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_receive_error_does_not_end_the_resolution_round() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let transport = FakeTransport::new(HashMap::from([(
            interfaces[0].address,
            response(NAME, "192.168.1.50"),
        )]));
        transport
            .receive_errors
            .lock()
            .unwrap()
            .push_back(io::Error::from(io::ErrorKind::ConnectionReset));
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .unwrap(),
            vec!["192.168.1.50".parse::<IpAddr>().unwrap()]
        );
    }

    struct ScriptedReceiver(tokio::sync::Mutex<VecDeque<io::Result<Datagram>>>);

    #[async_trait]
    impl Transport for ScriptedReceiver {
        fn join(&self, _: &LanInterface) -> io::Result<()> {
            Ok(())
        }
        async fn send(&self, _: &LanInterface, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        async fn receive(&self) -> io::Result<Datagram> {
            self.0
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Err(socket_gone_error()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_shared_receiver_recovers_after_connection_reset_and_stops_on_a_gone_socket() {
        let transport = Arc::new(ScriptedReceiver(tokio::sync::Mutex::new(VecDeque::from([
            Err(io::Error::from(io::ErrorKind::ConnectionReset)),
            Ok(Datagram {
                packet: b"answer".to_vec(),
                source: "192.168.1.50:5353".parse().unwrap(),
                arrival: Some(1),
            }),
            Err(socket_gone_error()),
        ]))));
        let (sender, mut receiver) = tokio::sync::broadcast::channel(8);
        let task = tokio::spawn(fan_out(transport, sender));
        assert_eq!(receiver.recv().await.unwrap().unwrap().packet, b"answer");
        assert!(receiver.recv().await.unwrap().is_err());
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_unresolved_name_has_a_bounded_round_with_retries() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let transport = FakeTransport::new(HashMap::new());
        let started = tokio::time::Instant::now();
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test").await,
            Err(ResolveFailure::Unresolved)
        );
        assert_eq!(started.elapsed(), RESOLVE_TIMEOUT);
        assert!(transport.sent.lock().unwrap().len() >= 4);
    }

    #[tokio::test(start_paused = true)]
    async fn off_subnet_answers_report_the_fixed_rejection_code() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let mut transport = FakeTransport::new(HashMap::from([(
            interfaces[0].address,
            response(NAME, "8.8.8.8"),
        )]));
        transport
            .reply_sources
            .insert(interfaces[0].address, "192.168.1.50:5353".parse().unwrap());
        let failure = resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
            .await
            .unwrap_err();
        assert_eq!(failure, ResolveFailure::AnswerRejected);
        assert_eq!(failure.code(), "mdns-answer-rejected");
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_just_before_the_deadline_survives_the_collection_grace() {
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        let mut transport = FakeTransport::new(HashMap::from([(
            interfaces[0].address,
            response(NAME, "192.168.1.50"),
        )]));
        transport.reply_delay = RESOLVE_TIMEOUT - Duration::from_millis(50);
        let started = tokio::time::Instant::now();
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test")
                .await
                .unwrap(),
            vec!["192.168.1.50".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(started.elapsed(), RESOLVE_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn no_interfaces_and_failed_memberships_report_distinct_reasons() {
        let mut transport = FakeTransport::new(HashMap::new());
        assert_eq!(
            resolve_with(NAME, &[], &transport, RESOLVE_TIMEOUT, "test").await,
            Err(ResolveFailure::NoLanInterfaces)
        );
        let interfaces = vec![interface("eth0", "192.168.1.10")];
        transport.failed_join = Some(interfaces[0].address);
        assert_eq!(
            resolve_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, "test").await,
            Err(ResolveFailure::SocketUnavailable)
        );
        assert!(transport.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn wildcard_socket_preserves_unicast_replies_and_explicit_egress() {
        // Ephemeral loopback sockets only; no mDNS multicast is sent by this test.
        let transport = UdpTransport::bound(0).unwrap();
        assert_eq!(
            transport.socket.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        let socket = socket2::SockRef::from(&transport.socket);
        assert!(socket.reuse_address().unwrap());
        assert_eq!(socket.multicast_ttl_v4().unwrap(), 255);
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        transport
            .send_on(
                &interface("test-lan", "127.0.0.1"),
                b"query",
                sender.local_addr().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(socket.multicast_if_v4().unwrap(), Ipv4Addr::LOCALHOST);
        let mut query = [0; 32];
        assert_eq!(sender.recv_from(&mut query).await.unwrap().0, 5);
        sender
            .send_to(
                b"unicast reply",
                (
                    Ipv4Addr::LOCALHOST,
                    transport.socket.local_addr().unwrap().port(),
                ),
            )
            .await
            .unwrap();
        let received = transport.receive().await.unwrap();
        assert_eq!(received.packet, b"unicast reply");
        assert_eq!(received.source, sender.local_addr().unwrap());
        #[cfg(target_os = "linux")]
        assert!(
            received.arrival.is_some(),
            "Linux carries the kernel arrival interface"
        );
    }

    #[tokio::test]
    async fn simultaneous_names_receive_their_unicast_answers_from_one_shared_socket() {
        const OTHER_NAME: &str = "8c14a372-9db5-4faa-bbf1-d93583114e89.local";
        let server_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut transport = UdpTransport::bound(0).unwrap();
        transport.destination = server_socket.local_addr().unwrap();
        let shared = SharedSocket::new(transport);
        let weak = std::sync::Arc::downgrade(&shared);
        let weak_transport = Arc::downgrade(&shared.transport);
        let first = shared.subscribe();
        let second = shared.subscribe();
        let server = tokio::spawn(async move {
            let mut protocol = Mdns::new(
                MdnsConfig::default()
                    .with_local_names(vec![NAME.to_string(), OTHER_NAME.to_string()])
                    .with_local_ip("192.168.1.50".parse().unwrap()),
            );
            let mut packet = vec![0; 2048];
            for _ in 0..2 {
                let (length, source) = server_socket.recv_from(&mut packet).await.unwrap();
                protocol
                    .handle_read(tagged(&packet[..length], source))
                    .unwrap();
                while let Some(reply) = protocol.poll_write() {
                    server_socket.send_to(&reply.message, source).await.unwrap();
                }
            }
        });
        let local = interface("test-lan", "127.0.0.1");
        first.join(&local).unwrap();
        second.join(&local).unwrap();
        let (first_answer, second_answer) = tokio::join!(
            unicast_answer(NAME, &local, &first),
            unicast_answer(OTHER_NAME, &local, &second),
        );
        assert_eq!(
            first_answer.unwrap(),
            vec!["192.168.1.50".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            second_answer.unwrap(),
            vec!["192.168.1.50".parse::<IpAddr>().unwrap()]
        );
        server.await.unwrap();
        drop(first);
        drop(second);
        drop(shared);
        assert!(
            weak.upgrade().is_none(),
            "receiver task does not retain its owner"
        );
        tokio::task::yield_now().await;
        assert!(
            weak_transport.upgrade().is_none(),
            "aborted receiver releases its socket"
        );
    }

    async fn unicast_answer(
        name: &str,
        interface: &LanInterface,
        transport: &impl Transport,
    ) -> io::Result<Vec<IpAddr>> {
        transport.send(interface, &query_bytes(name)).await?;
        tokio::time::timeout(RESOLVE_TIMEOUT, async {
            loop {
                let received = transport.receive().await?;
                // Loopback exercises dispatch and name matching. Synthetic LAN
                // tests separately enforce the subnet trust boundary.
                if let Some(answer) = answer_from(name, &received.packet, received.source) {
                    return Ok(vec![answer]);
                }
            }
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }
}
