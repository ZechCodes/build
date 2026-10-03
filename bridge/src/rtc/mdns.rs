//! Resolve browser mDNS candidates on every eligible LAN, independently of
//! ICE negotiation. Interface discovery happens for each resolution round.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use rtc::mdns::{Mdns, MdnsConfig, MdnsEvent, MDNS_DEST_ADDR, MDNS_MULTICAST_IPV4, MDNS_PORT};
use rtc::sansio::Protocol;
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const ANSWER_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
struct LanInterface {
    name: String,
    address: Ipv4Addr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveFailure {
    InvalidName,
    InterfacesUnavailable,
    NoLanInterfaces,
    SocketUnavailable,
    Unresolved,
}

impl ResolveFailure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidName => "mdns-invalid-name",
            Self::InterfacesUnavailable => "mdns-interfaces-unavailable",
            Self::NoLanInterfaces => "mdns-no-lan-interfaces",
            Self::SocketUnavailable => "mdns-socket-unavailable",
            Self::Unresolved => "mdns-unresolved",
        }
    }
}

#[async_trait]
trait Transport: Send + Sync {
    fn join(&self, interface: &LanInterface) -> io::Result<()>;
    async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()>;
    async fn receive(&self) -> io::Result<(Vec<u8>, SocketAddr)>;
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

    async fn receive(&self) -> io::Result<(Vec<u8>, SocketAddr)> {
        let mut packet = vec![0; 2048];
        let (length, source) = self.socket.recv_from(&mut packet).await?;
        packet.truncate(length);
        Ok((packet, source))
    }
}

type Datagram = (Vec<u8>, SocketAddr);
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

async fn fan_out(transport: Arc<UdpTransport>, replies: tokio::sync::broadcast::Sender<Received>) {
    loop {
        let received = transport.receive().await.map_err(|_| ());
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
                _ => return Err(io::Error::other("mDNS receive unavailable")),
            }
        }
    }
}

pub(crate) async fn resolve(
    name: &str,
    allowlist: Option<&[String]>,
    session_id: &str,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let name = normalized_name(name)?;
    let reported = rtc::shared::ifaces::ifaces()
        .map_err(|_| ResolveFailure::InterfacesUnavailable)?
        .into_iter()
        .filter_map(|interface| Some((interface.name, interface.addr?)))
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
    reported: &[(String, SocketAddr)],
    allowlist: Option<&[String]>,
) -> Vec<LanInterface> {
    let mut selected: Vec<LanInterface> = Vec::new();
    for (name, address) in reported {
        let IpAddr::V4(address) = address.ip() else {
            continue;
        };
        if !eligible_interface(name, address, allowlist) {
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
        });
    }
    selected
}

fn eligible_interface(name: &str, address: Ipv4Addr, allowlist: Option<&[String]>) -> bool {
    if allowlist.is_some_and(|names| !names.iter().any(|allowed| allowed == name)) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    let excluded = ["docker", "veth", "loopback"]
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

fn normalized_name(name: &str) -> Result<String, ResolveFailure> {
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

fn answer_from(name: &str, packet: &[u8], source: SocketAddr) -> Option<IpAddr> {
    // A fresh protocol accepts the matching answer from each interface, even
    // after another interface has answered. Its parser handles compressed DNS.
    let mut protocol = Mdns::new(MdnsConfig::default());
    let query = protocol.query(name);
    protocol
        .handle_read(TaggedBytesMut {
            now: std::time::Instant::now(),
            transport: TransportContext {
                local_addr: SocketAddr::from((Ipv4Addr::UNSPECIFIED, MDNS_PORT)),
                peer_addr: source,
                transport_protocol: TransportProtocol::UDP,
                ecn: None,
            },
            message: packet.into(),
        })
        .ok()?;
    while let Some(event) = protocol.poll_event() {
        if let MdnsEvent::QueryAnswered(id, IpAddr::V4(address)) = event {
            if id == query && usable_ipv4(address) {
                return Some(address.into());
            }
        }
    }
    None
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
    super::diagnostic(
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
    super::diagnostic(
        session_id,
        &format!(
            "mdns-query sent_interfaces={successes} failed_sends={}",
            interfaces.len() - successes
        ),
    );
    if successes == 0 {
        return Err(ResolveFailure::SocketUnavailable);
    }
    receive_answers(name, interfaces, transport, &query, deadline).await
}

async fn receive_answers(
    name: &str,
    interfaces: &[LanInterface],
    transport: &impl Transport,
    query: &[u8],
    deadline: tokio::time::Instant,
) -> Result<Vec<IpAddr>, ResolveFailure> {
    let mut answers = Vec::new();
    let mut answer_deadline = None;
    let mut retry =
        tokio::time::interval_at(tokio::time::Instant::now() + RETRY_INTERVAL, RETRY_INTERVAL);
    loop {
        let wake = answer_deadline.unwrap_or(deadline).min(deadline);
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(wake) => return resolved_or(answers, ResolveFailure::Unresolved),
            _ = retry.tick() => { send_queries(interfaces, transport, query, deadline).await; }
            received = transport.receive() => {
                let Ok((packet, source)) = received else {
                    return resolved_or(answers, ResolveFailure::SocketUnavailable);
                };
                if let Some(address) = answer_from(name, &packet, source) {
                    if !answers.contains(&address) {
                        answers.push(address);
                    }
                    answer_deadline.get_or_insert_with(|| tokio::time::Instant::now() + ANSWER_GRACE);
                }
            }
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
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    const NAME: &str = "4c14a372-9db5-4faa-bbf1-d93583114e89.local";

    fn reported(entries: &[(&str, &str)]) -> Vec<(String, SocketAddr)> {
        entries
            .iter()
            .map(|(name, address)| (name.to_string(), address.parse().unwrap()))
            .collect()
    }

    fn interface(name: &str, address: &str) -> LanInterface {
        LanInterface {
            name: name.to_string(),
            address: address.parse().unwrap(),
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

    struct FakeTransport {
        joined: Mutex<Vec<Ipv4Addr>>,
        sent: Mutex<Vec<Ipv4Addr>>,
        failed_join: Option<Ipv4Addr>,
        failed_send: Option<Ipv4Addr>,
        reply_delay: Duration,
        replies: HashMap<Ipv4Addr, Vec<u8>>,
        sender: tokio::sync::mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>,
        receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>>,
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
                let response = (reply.clone(), "192.168.1.50:5353".parse().unwrap());
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

        async fn receive(&self) -> io::Result<(Vec<u8>, SocketAddr)> {
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
        let (packet, source) = transport.receive().await.unwrap();
        assert_eq!(packet, b"unicast reply");
        assert_eq!(source, sender.local_addr().unwrap());
    }

    #[tokio::test]
    async fn simultaneous_names_receive_their_unicast_answers_from_one_shared_socket() {
        const OTHER_NAME: &str = "8c14a372-9db5-4faa-bbf1-d93583114e89.local";
        let server_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut transport = UdpTransport::bound(0).unwrap();
        transport.destination = server_socket.local_addr().unwrap();
        let shared = SharedSocket::new(transport);
        let weak = std::sync::Arc::downgrade(&shared);
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
        let interfaces = vec![interface("test-lan", "127.0.0.1")];
        let (first_answer, second_answer) = tokio::join!(
            resolve_with(NAME, &interfaces, &first, RESOLVE_TIMEOUT, "test-one"),
            resolve_with(
                OTHER_NAME,
                &interfaces,
                &second,
                RESOLVE_TIMEOUT,
                "test-two"
            ),
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
    }
}
