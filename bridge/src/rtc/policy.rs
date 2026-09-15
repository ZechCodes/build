//! What this bridge's ICE agent is allowed to do to reach a browser.
//!
//! One value, resolved once at startup, that every peer connection is built
//! from (strict P2P transport spec, rule 8): resolve the browser's mDNS host
//! candidates, gather UDP4 and (where the machine has one) UDP6 on every
//! non-loopback interface — or only on the ones an operator named — and make a
//! TURN pair wait before it may be
//! accepted, so a slower direct pair can still win.
//!
//! **Hides** which knobs of the webrtc crate say those things, and where a
//! wildcard turns into one socket per interface. **Boundary**: a policy is
//! read from the environment, asked what a peer may be configured with, and
//! asked where a peer gathers from. The future direct-network mode (rule 7)
//! is this same value with [`IceMode::DirectOnly`] and a rendezvous that is
//! not the relay; nothing else about it is new.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use rtc::ice::mdns::MulticastDnsMode;
use rtc::ice::network_type::NetworkType;
use rtc::shared::ifaces::ifaces;
use serde_json::Value;
use webrtc::peer_connection::SettingEngine;

use super::RtcError;

/// Which paths this bridge may be reached over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IceMode {
    /// Everything the browser offers, TURN included: the hosted default.
    #[default]
    All,
    /// Direct paths only — a LAN or Tailscale install, and what the future
    /// direct-network mode runs with. No TURN server reaches the agent and no
    /// relay candidate is paired with.
    DirectOnly,
}

/// How this bridge's ICE agent gathers and what it will pair on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcePolicy {
    pub mode: IceMode,
    /// How long a relay (TURN) pair must wait before the agent may accept it,
    /// so a direct pair that is still checking can win. [`Duration::ZERO`]
    /// turns the wait off.
    pub relay_min_wait: Duration,
    /// The interfaces host candidates are gathered on. `None` — the default —
    /// is every non-loopback interface the machine has at gather time.
    pub interfaces: Option<Vec<String>>,
}

/// `all` or `direct-only`.
pub const ICE_POLICY_ENV: &str = "BRIDGE_ICE_POLICY";

/// The relay acceptance wait, in milliseconds. `0` turns it off.
pub const ICE_RELAY_MIN_WAIT_ENV: &str = "BRIDGE_ICE_RELAY_MIN_WAIT_MS";

/// A comma-separated interface allow-list, e.g. `tailscale0,eth0`.
pub const ICE_INTERFACES_ENV: &str = "BRIDGE_ICE_INTERFACES";

/// Long enough for a host or server-reflexive pair on an ordinary home
/// network to finish its checks, short enough that a browser behind a
/// symmetric NAT is not left waiting (spec rule 8).
pub const DEFAULT_RELAY_MIN_WAIT_MS: u64 = 1500;

impl Default for IcePolicy {
    fn default() -> Self {
        IcePolicy {
            mode: IceMode::default(),
            relay_min_wait: Duration::from_millis(DEFAULT_RELAY_MIN_WAIT_MS),
            interfaces: None,
        }
    }
}

/// Where a peer gathers IPv4 host candidates from when no interface is named:
/// every non-loopback interface the machine has. The webrtc crate expands
/// this wildcard at bind time — one socket per interface address that exists
/// *then* — which is what makes an ICE restart after a network handover pick
/// up the interfaces the device has now.
pub(crate) const EVERY_IPV4_INTERFACE: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

/// The same for IPv6. A wildcard is expanded into the addresses of its **own**
/// family only (`driver.rs::expand_wildcard` skips every address whose family
/// differs), so this is the only way an IPv6 host candidate is ever gathered —
/// and rule 8's "gathers over UDP4/UDP6" needs it on any network whose shared
/// path is IPv6 (an IPv6-only LAN, a Tailnet).
pub(crate) const EVERY_IPV6_INTERFACE: SocketAddr =
    SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0);

/// Where one peer connection binds, as `with_udp_addrs` takes it — **a
/// question, not an answer**.
///
/// The crate resolves what it is given on every bind, at startup and again on
/// every ICE-restart rebind, and that late resolution is what lets a rebind
/// follow a network change instead of asking for an address that has gone
/// away. A wildcard gets that for free; a literal address does not, it is
/// rebound verbatim. So an allow-list travels as the names an operator wrote
/// and is enumerated at each bind, exactly like the wildcard beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GatherAddr {
    /// Every non-loopback interface, as the family wildcards the crate expands.
    EveryInterface,
    /// Only the interfaces `BRIDGE_ICE_INTERFACES` named.
    Named(Vec<String>),
}

impl ToSocketAddrs for GatherAddr {
    type Iter = std::vec::IntoIter<SocketAddr>;

    /// Enumerated here rather than stored, because this runs again on every
    /// rebind. An enumeration that fails leaves the wildcard — the crate binds
    /// it as given and the connection can still come up over STUN or TURN —
    /// and leaves an allow-list with nothing, which is the only honest answer
    /// to "bind these interfaces" when the interfaces cannot be read.
    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        let reported = reported_interfaces().unwrap_or_default();
        let addrs = match self {
            GatherAddr::EveryInterface => every_interface(&reported),
            GatherAddr::Named(named) => gathered_on(named, &reported),
        };
        Ok(addrs.into_iter())
    }
}

impl IcePolicy {
    /// Read the policy from the environment, refusing anything it cannot
    /// read. Pure in its lookup so the default/override matrix is testable,
    /// and fallible because a mistyped policy must stop the daemon at startup
    /// rather than quietly gather the wrong way for a month.
    pub fn resolve(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        Ok(IcePolicy {
            mode: mode_of(lookup(ICE_POLICY_ENV))?,
            relay_min_wait: relay_min_wait_of(lookup(ICE_RELAY_MIN_WAIT_ENV))?,
            interfaces: interfaces_of(lookup(ICE_INTERFACES_ENV))?,
        })
    }

    /// The ICE servers a peer built under this policy may be configured with,
    /// out of the list the browser fetched from the api and offered.
    ///
    /// `direct-only` drops every `turn:` / `turns:` url — the egress somebody
    /// pays for — and keeps STUN, which is free and is how a peer on a
    /// Tailscale or LAN install learns the address to put in a host
    /// candidate. The transport policy itself stays `All` for the same
    /// reason: `Relay` would gather nothing BUT relay candidates, the exact
    /// opposite of what this mode is for.
    pub fn allowed_ice_servers(&self, offered: &[Value]) -> Vec<Value> {
        match self.mode {
            IceMode::All => offered.to_vec(),
            IceMode::DirectOnly => offered.iter().filter_map(without_turn).collect(),
        }
    }

    /// Whether one candidate the browser trickled may be paired with.
    ///
    /// `direct-only` says no to the browser's own relay candidates: our own
    /// TURN servers being stripped only stops the bridge from allocating one,
    /// and a pair on the browser's allocation is the same billed egress seen
    /// from the other end.
    ///
    /// **What this cannot reach**: the browser is the controlling agent and
    /// keeps its own TURN servers (the SPA passes the minted list through
    /// unchanged, and nothing on this end can stop a browser gathering). Its
    /// connectivity checks arrive here from its relay allocation, and a check
    /// from an address no candidate named is a *peer-reflexive* remote
    /// candidate the agent creates for itself — it never passes through this
    /// method, and nothing in it says `typ relay`. A browser behind a
    /// symmetric NAT can therefore still nominate a pair that rides its own
    /// TURN allocation under `direct-only`, and the ledger will read it as
    /// `direct` (the same `prflx` under-count the transport spec's open
    /// question 2 names). Closing it needs the browser's own view of its
    /// nominated pair, which is that question, not this one.
    pub fn allows_remote_candidate(&self, candidate: &Value) -> bool {
        match self.mode {
            IceMode::All => true,
            IceMode::DirectOnly => !is_relay_candidate(candidate),
        }
    }

    /// The browser's offer as a peer under this policy may take it.
    ///
    /// Trickling is not the only way a candidate arrives: `a=candidate` lines
    /// carried inside the offer are extracted by `set_remote_description`
    /// itself (`rtc`'s `extract_ice_details`) and added without anything here
    /// being asked, so a browser that had already gathered its relay candidate
    /// when it offered would put it on the wire past
    /// [`allows_remote_candidate`](Self::allows_remote_candidate).
    /// `direct-only` takes those lines out; every other line, and every line
    /// ending, is left exactly as the browser wrote it, because what comes back
    /// out of this is parsed as SDP.
    pub fn allowed_offer<'sdp>(&self, offer_sdp: &'sdp str) -> Cow<'sdp, str> {
        match self.mode {
            IceMode::All => Cow::Borrowed(offer_sdp),
            IceMode::DirectOnly => Cow::Owned(
                offer_sdp
                    .split_inclusive('\n')
                    .filter(|line| !is_relay_candidate_line(line))
                    .collect(),
            ),
        }
    }

    /// Where a peer under this policy binds, as
    /// `PeerConnectionBuilder::with_udp_addrs` takes it — one
    /// [`GatherAddr`], resolved by the crate at every bind.
    ///
    /// This is where an interface allow-list is applied. The pinned webrtc
    /// crate (0.20.4) has no interface filter — `SettingEngine`'s is still a
    /// `TODO` in `rtc`'s source — but it resolves what it is given on every
    /// bind, so a value that enumerates by name does the filtering one step
    /// earlier and does it strictly: an interface that is not on the list is
    /// never bound at all, and one that changed address since the last bind is
    /// bound at the address it has now.
    ///
    /// Fallible for one reason: an allow-list that answers to no address on
    /// this machine right now. That bridge would bind nothing and could not be
    /// reached at all, so the offer is refused loudly (rule 3 blocks the
    /// device, naming the list) rather than gathered quietly from nowhere.
    pub(crate) fn gather_from(&self) -> Result<Vec<GatherAddr>, RtcError> {
        let Some(named) = &self.interfaces else {
            return Ok(vec![GatherAddr::EveryInterface]);
        };
        // Enumerated once here as well, and only to fail closed: a list that
        // answers to nothing now would bind nothing, and a bridge with no
        // socket has no path to a browser at all. What the peer is given is
        // still the names.
        let reported = reported_interfaces().map_err(|e| {
            RtcError::Refused(format!(
                "cannot enumerate the local interfaces {ICE_INTERFACES_ENV} names ({}): {e}",
                named.join(",")
            ))
        })?;
        if gathered_on(named, &reported).is_empty() {
            return Err(RtcError::Refused(format!(
                "{ICE_INTERFACES_ENV} names no interface this machine has an address on ({})",
                named.join(",")
            )));
        }
        Ok(vec![GatherAddr::Named(named.clone())])
    }

    /// The agent knobs rule 8 asks for, as the webrtc crate spells them.
    ///
    /// `multicast_dns` is the caller's because it is the one knob that can fail
    /// a build: `QueryOnly` decides whether a browser can be reached at all on
    /// a LAN — Chrome and Safari offer `<uuid>.local` host candidates and
    /// nothing else, and an agent with mDNS disabled discards every one of
    /// them — but the crate joins the multicast group while it binds, so a host
    /// that cannot join gets no peer connection at all. Rule 8 asks for
    /// `QueryOnly` and `rtc.rs::built_or_without_mdns` is what falls back. It
    /// is also the one knob here no test in this crate can prove: an in-process
    /// peer offers IP host candidates, never mDNS names, so resolution is
    /// never exercised. A real browser is what verifies it (stage 06's
    /// browser pass).
    pub(crate) fn setting_engine(&self, multicast_dns: MulticastDnsMode) -> SettingEngine {
        let mut engine = SettingEngine::default();
        engine.set_multicast_dns_mode(multicast_dns);
        engine.set_network_types(vec![NetworkType::Udp4, NetworkType::Udp6]);
        // Loopback is already excluded where the addresses are chosen (the
        // crate's wildcard expansion, and `gathered_on` for an allow-list);
        // said here too so a crate version that starts reading this flag keeps
        // the behaviour the addresses already have.
        engine.set_include_loopback_candidate(false);
        // Always `Some`: `None` would restore the crate's own 2 s default,
        // and `Duration::ZERO` is how this policy says "no wait".
        //
        // What it binds, honestly: rtc-ice consults the acceptance waits in
        // its CONTROLLING selector, and with a browser offering, the browser
        // controls and this agent is controlled — so today the wait governs
        // an ICE role conflict and the future direct mode (where this side
        // offers), while Chrome's own prioritisation is what usually keeps a
        // TURN pair from winning. Set here because it is this agent's half of
        // rule 8 and it is the half that becomes load-bearing the moment a
        // rendezvous that is not the relay has the bridge offer.
        engine.set_relay_acceptance_min_wait(Some(self.relay_min_wait));
        engine
    }
}

fn mode_of(configured: Option<String>) -> Result<IceMode, String> {
    match configured.as_deref().map(str::trim) {
        None | Some("all") => Ok(IceMode::All),
        Some("direct-only") => Ok(IceMode::DirectOnly),
        Some(other) => Err(format!(
            "{ICE_POLICY_ENV} must be `all` or `direct-only`, not `{other}`"
        )),
    }
}

fn relay_min_wait_of(configured: Option<String>) -> Result<Duration, String> {
    let Some(configured) = configured else {
        return Ok(Duration::from_millis(DEFAULT_RELAY_MIN_WAIT_MS));
    };
    configured
        .trim()
        .parse::<u64>()
        .map(Duration::from_millis)
        .map_err(|e| {
            format!(
                "{ICE_RELAY_MIN_WAIT_ENV} must be whole milliseconds (`0` for no wait), not `{}`: {e}",
                configured.trim()
            )
        })
}

fn interfaces_of(configured: Option<String>) -> Result<Option<Vec<String>>, String> {
    let Some(configured) = configured else {
        return Ok(None);
    };
    let named: Vec<String> = configured
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if named.is_empty() {
        return Err(format!(
            "{ICE_INTERFACES_ENV} is set but names no interface; unset it to gather on every one"
        ));
    }
    Ok(Some(named))
}

/// Every address the OS reports an interface on, named. The interfaces with no
/// address at all (a down link, a tunnel with nothing on it) are not addresses
/// anything could bind.
fn reported_interfaces() -> Result<Vec<(String, SocketAddr)>, String> {
    Ok(ifaces()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|interface| Some((interface.name, interface.addr?)))
        .collect())
}

/// The wildcards that mean "every non-loopback interface" on this machine.
///
/// The IPv4 one is always handed over: with no address to expand into, the
/// crate binds it verbatim and the connection can still come up over STUN or
/// TURN, which is the failure this bridge already had. The IPv6 one is added
/// only where there is an address to expand into, because that same verbatim
/// fallback would bind `[::]` and put `::` — an address no peer can dial — in
/// a host candidate, and point a STUN query at a socket that cannot reach the
/// server.
fn every_interface(reported: &[(String, SocketAddr)]) -> Vec<SocketAddr> {
    let mut wildcards = vec![EVERY_IPV4_INTERFACE];
    let has_ipv6 = reported
        .iter()
        .any(|(_, addr)| addr.is_ipv6() && is_reachable(&addr.ip()));
    if has_ipv6 {
        wildcards.push(EVERY_IPV6_INTERFACE);
    }
    wildcards
}

/// The bind addresses an allow-list picks out of what the OS reported, in the
/// order the names were given. Loopback, unspecified and link-local addresses
/// are skipped for the same reason the crate skips them when it expands a
/// wildcard: no peer can use one.
fn gathered_on(named: &[String], reported: &[(String, SocketAddr)]) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for name in named {
        for (interface, addr) in reported {
            if interface == name && is_reachable(&addr.ip()) {
                let addr = SocketAddr::new(addr.ip(), 0);
                if !addrs.contains(&addr) {
                    addrs.push(addr);
                }
            }
        }
    }
    addrs
}

/// Whether an address of ours is one a browser could send to.
fn is_reachable(ip: &IpAddr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return false;
    }
    match ip {
        // An IPv6 link-local address is meaningless to the peer without its
        // scope id, which does not survive into a candidate.
        IpAddr::V4(v4) => !v4.is_link_local(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 != 0xfe80,
    }
}

/// One offered server with its TURN urls removed, or nothing if TURN was all
/// it was.
fn without_turn(offered: &Value) -> Option<Value> {
    let kept: Vec<Value> = offered_urls(offered)
        .into_iter()
        .filter(|url| !is_turn(url))
        .map(Value::from)
        .collect();
    let object = offered.as_object()?;
    if kept.is_empty() {
        return None;
    }
    let mut stripped = object.clone();
    stripped.insert("urls".to_string(), Value::Array(kept));
    Some(Value::Object(stripped))
}

/// The urls of one offered ICE server. Tolerant on purpose: Cloudflare answers
/// a list and a device with no TURN key configured answers a bare string, and
/// both are the same server to everything above.
pub(crate) fn offered_urls(offered: &Value) -> Vec<String> {
    match offered.get("urls") {
        Some(Value::String(url)) => vec![url.clone()],
        Some(Value::Array(urls)) => urls
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn is_turn(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("turn:") || url.starts_with("turns:")
}

/// Whether a trickled candidate is one end of a TURN allocation. Read off the
/// candidate attribute itself (`... typ relay ...`), the one place the type is
/// stated, so a candidate shape this bridge does not otherwise parse is still
/// classified.
fn is_relay_candidate(candidate: &Value) -> bool {
    candidate
        .get("candidate")
        .and_then(Value::as_str)
        .is_some_and(is_relay_attribute)
}

/// The same question of one line of an SDP, which states the candidate the same
/// way with `a=candidate:` in front of it.
fn is_relay_candidate_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("a=candidate:") && is_relay_attribute(line)
}

/// `... typ relay ...` in a candidate attribute, wherever it was written.
fn is_relay_attribute(attribute: &str) -> bool {
    attribute
        .split_whitespace()
        .skip_while(|word| *word != "typ")
        .nth(1)
        == Some("relay")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// One variable set, everything else unset.
    fn env_of(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |key: &str| {
            set.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    /// A hosted bridge sets none of this and still prefers direct: every
    /// interface, and a TURN pair that waits for one.
    #[test]
    fn a_bridge_with_no_environment_prefers_direct_over_every_interface() {
        let policy = IcePolicy::resolve(no_env).expect("no environment is the default policy");

        assert_eq!(policy.mode, IceMode::All);
        assert_eq!(policy.relay_min_wait, Duration::from_millis(1500));
        assert_eq!(policy.interfaces, None);
        assert_eq!(policy, IcePolicy::default());
    }

    #[test]
    fn the_only_two_modes_are_all_and_direct_only() {
        let all = IcePolicy::resolve(env_of(&[(ICE_POLICY_ENV, "all")])).expect("all is a mode");
        assert_eq!(all.mode, IceMode::All);

        let direct = IcePolicy::resolve(env_of(&[(ICE_POLICY_ENV, "direct-only")]))
            .expect("direct-only is a mode");
        assert_eq!(direct.mode, IceMode::DirectOnly);

        let refused = IcePolicy::resolve(env_of(&[(ICE_POLICY_ENV, "direct")]))
            .expect_err("a near miss is not a mode");
        assert!(refused.contains(ICE_POLICY_ENV), "{refused}");
        assert!(refused.contains("direct-only"), "{refused}");
    }

    /// The wait is milliseconds because that is the scale it is tuned on, and
    /// `0` is the way to say "accept a relay pair as soon as it checks out".
    #[test]
    fn the_relay_wait_is_milliseconds_and_zero_turns_it_off() {
        let waited = IcePolicy::resolve(env_of(&[(ICE_RELAY_MIN_WAIT_ENV, "250")]))
            .expect("a number of milliseconds");
        assert_eq!(waited.relay_min_wait, Duration::from_millis(250));

        let none = IcePolicy::resolve(env_of(&[(ICE_RELAY_MIN_WAIT_ENV, "0")]))
            .expect("zero is a valid wait");
        assert_eq!(none.relay_min_wait, Duration::ZERO);

        for bad in ["soon", "-1", "1.5", ""] {
            let refused = IcePolicy::resolve(env_of(&[(ICE_RELAY_MIN_WAIT_ENV, bad)]))
                .expect_err("only whole milliseconds are a wait");
            assert!(refused.contains(ICE_RELAY_MIN_WAIT_ENV), "{refused}");
        }
    }

    #[test]
    fn the_interface_allow_list_is_a_comma_list_and_an_empty_one_is_a_mistake() {
        let named = IcePolicy::resolve(env_of(&[(ICE_INTERFACES_ENV, " tailscale0 , eth0 ")]))
            .expect("a comma list of interface names");
        assert_eq!(
            named.interfaces,
            Some(vec!["tailscale0".to_string(), "eth0".to_string()])
        );

        for empty in ["", " ", ",", " , "] {
            let refused = IcePolicy::resolve(env_of(&[(ICE_INTERFACES_ENV, empty)]))
                .expect_err("an allow-list that names nothing would gather nothing");
            assert!(refused.contains(ICE_INTERFACES_ENV), "{refused}");
        }
    }

    /// Cloudflare's array as the api mints it: one credentialed TURN entry and
    /// one bare STUN url. `direct-only` keeps what is free and drops what
    /// somebody pays for; the hosted default touches neither.
    #[test]
    fn direct_only_drops_every_turn_url_and_keeps_stun() {
        let minted = vec![
            json!({
                "urls": ["stun:stun.cloudflare.com:3478", "turn:turn.cloudflare.com:3478?transport=udp"],
                "username": "user-1",
                "credential": "secret-1",
            }),
            json!({ "urls": "turns:turn.cloudflare.com:5349?transport=tcp" }),
        ];

        let direct_only = IcePolicy {
            mode: IceMode::DirectOnly,
            ..IcePolicy::default()
        };
        let allowed = direct_only.allowed_ice_servers(&minted);

        assert_eq!(
            allowed,
            vec![json!({
                "urls": ["stun:stun.cloudflare.com:3478"],
                "username": "user-1",
                "credential": "secret-1",
            })],
            "a server with nothing but TURN urls left is not a server"
        );
        assert_eq!(
            IcePolicy::default().allowed_ice_servers(&minted),
            minted,
            "the hosted default passes the minted list through unchanged"
        );
    }

    /// Stripping our own TURN servers is only half of rule 8: the browser
    /// trickles the candidates of the TURN server IT allocated, and pairing
    /// with one of those is the same billed egress from the other end.
    #[test]
    fn direct_only_refuses_the_relay_candidates_the_browser_trickles() {
        let relayed = json!({
            "candidate": "candidate:1 1 udp 41885439 198.51.100.7 51234 typ relay raddr 0.0.0.0 rport 0",
            "sdpMid": "0",
        });
        let host = json!({
            "candidate": "candidate:2 1 udp 2130706431 192.168.1.9 51235 typ host",
            "sdpMid": "0",
        });

        let direct_only = IcePolicy {
            mode: IceMode::DirectOnly,
            ..IcePolicy::default()
        };
        assert!(!direct_only.allows_remote_candidate(&relayed));
        assert!(direct_only.allows_remote_candidate(&host));

        assert!(
            IcePolicy::default().allows_remote_candidate(&relayed),
            "the hosted default pairs with whatever reaches it"
        );
    }

    /// Stripping the browser's TURN servers and refusing its trickled relay
    /// candidates leaves one way in: a candidate carried **inside** the offer.
    /// `set_remote_description` extracts every `a=candidate` line and adds it
    /// (`rtc`'s `extract_ice_details`), where nothing consults
    /// `allows_remote_candidate`, so an offer from a browser that had already
    /// gathered its relay candidate would pair on billed TURN egress under the
    /// one mode that exists to prevent it.
    #[test]
    fn direct_only_takes_the_relay_candidates_out_of_the_offer() {
        let offer = "v=0\r\n\
                     m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
                     a=mid:0\r\n\
                     a=candidate:1 1 udp 2130706431 192.168.1.9 51235 typ host\r\n\
                     a=candidate:2 1 udp 41885439 198.51.100.7 51234 typ relay raddr 0.0.0.0 rport 0\r\n\
                     a=candidate:3 1 udp 1694498815 203.0.113.9 51236 typ srflx raddr 192.168.1.9 rport 51235\r\n\
                     a=end-of-candidates\r\n";

        let direct_only = IcePolicy {
            mode: IceMode::DirectOnly,
            ..IcePolicy::default()
        };
        let allowed = direct_only.allowed_offer(offer);

        assert!(!allowed.contains("typ relay"), "{allowed}");
        assert!(allowed.contains("typ host"), "a direct candidate stays");
        assert!(allowed.contains("typ srflx"), "so does a free one");
        assert_eq!(
            allowed.lines().count(),
            offer.lines().count() - 1,
            "one line out, every other line and its ending untouched: {allowed}"
        );
        assert_eq!(
            IcePolicy::default().allowed_offer(offer),
            offer,
            "the hosted default takes the offer as it was sent"
        );
        assert_eq!(
            direct_only.allowed_offer("v=0\r\na=mid:0\r\n"),
            "v=0\r\na=mid:0\r\n",
            "an offer with no candidate in it is not rewritten"
        );
    }

    /// The allow-list is applied where a wildcard would otherwise be expanded:
    /// one bind address per named interface, loopback and link-local skipped
    /// exactly as the crate skips them, and an interface the machine does not
    /// have contributes nothing.
    #[test]
    fn an_allow_list_binds_the_addresses_of_the_interfaces_it_names() {
        let reported = [
            ("lo", "127.0.0.1:0"),
            ("tailscale0", "100.101.102.103:0"),
            ("eth0", "192.168.1.9:0"),
            ("eth0", "[fe80::1]:0"),
            ("docker0", "172.17.0.1:0"),
        ];
        let reported: Vec<(String, SocketAddr)> = reported
            .iter()
            .map(|(name, addr)| (name.to_string(), addr.parse().expect("a bind address")))
            .collect();

        let bind = |addr: &str| -> SocketAddr { addr.parse().expect("a bind address") };
        assert_eq!(
            gathered_on(&["tailscale0".into(), "eth0".into()], &reported),
            vec![bind("100.101.102.103:0"), bind("192.168.1.9:0")],
            "the link-local address of a named interface is no use to a peer"
        );
        assert_eq!(
            gathered_on(&["lo".into()], &reported),
            Vec::<SocketAddr>::new(),
            "loopback is not a path to a browser"
        );
        assert_eq!(
            gathered_on(&["wg0".into()], &reported),
            Vec::<SocketAddr>::new(),
            "an interface this machine does not have"
        );
    }

    /// Without an allow-list the bind list is wildcards: the crate's own
    /// "every interface that exists at bind time", which is what makes an ICE
    /// restart follow a network handover.
    #[test]
    fn no_allow_list_gathers_from_the_wildcard() {
        let gathering = IcePolicy::default().gather_from().expect("the wildcard");

        assert_eq!(gathering, vec![GatherAddr::EveryInterface]);
        let bound: Vec<SocketAddr> = gathering[0]
            .to_socket_addrs()
            .expect("the wildcards")
            .collect();
        assert_eq!(bound[0], EVERY_IPV4_INTERFACE);
        assert!(bound.iter().all(|addr| addr.ip().is_unspecified()));
    }

    /// The crate expands a wildcard into the interface addresses of its **own
    /// family** and skips every other one, so the IPv4 wildcard alone gathers
    /// no IPv6 host candidate at all — and `set_network_types` promising UDP6
    /// would be a promise nothing keeps. A machine with no IPv6 address to
    /// expand into is not handed the IPv6 wildcard: the crate would bind `[::]`
    /// verbatim and every candidate off that socket names `::`, which no peer
    /// can dial.
    #[test]
    fn every_interface_adds_the_ipv6_wildcard_only_where_ipv6_exists() {
        let reported = |addrs: &[(&str, &str)]| -> Vec<(String, SocketAddr)> {
            addrs
                .iter()
                .map(|(name, addr)| (name.to_string(), addr.parse().expect("a bind address")))
                .collect()
        };

        assert_eq!(
            every_interface(&reported(&[("eth0", "192.168.1.9:0")])),
            vec![EVERY_IPV4_INTERFACE],
            "an IPv4-only machine"
        );
        assert_eq!(
            every_interface(&reported(&[
                ("eth0", "192.168.1.9:0"),
                ("eth0", "[2001:db8::5]:0"),
            ])),
            vec![EVERY_IPV4_INTERFACE, EVERY_IPV6_INTERFACE],
            "a dual-stack machine gathers on both families"
        );
        assert_eq!(
            every_interface(&reported(&[("lo", "[::1]:0"), ("eth0", "[fe80::1]:0")])),
            vec![EVERY_IPV4_INTERFACE],
            "loopback and link-local are addresses the crate would skip anyway"
        );
        assert_eq!(
            every_interface(&reported(&[("tailscale0", "[fd7a:115c::1]:0")])),
            vec![EVERY_IPV4_INTERFACE, EVERY_IPV6_INTERFACE],
            "a Tailnet's IPv6 is a path even with no IPv4 beside it"
        );
    }

    /// The allow-list is handed to the crate as the **names**, not the
    /// addresses they have right now.
    ///
    /// `resolve_bind_addrs` runs again on every ICE-restart rebind, but it only
    /// re-enumerates wildcards: a literal address is rebound verbatim. So an
    /// allow-list resolved once at build time would, after the named interface
    /// changed address (a DHCP lease, a Tailnet re-address), rebind an address
    /// that no longer exists — every bind skipped, the restart unable to
    /// recover, and only a fresh peer connection able to. Resolving at bind
    /// time is the property the wildcard already had; this is the allow-list
    /// keeping it.
    #[test]
    fn an_allow_list_is_bound_by_name_so_every_rebind_enumerates_again() {
        let reported = reported_interfaces().expect("this machine reports its interfaces");
        let Some((name, addr)) = reported
            .iter()
            .find(|(_, addr)| is_reachable(&addr.ip()))
            .cloned()
        else {
            return; // a machine with no usable interface proves nothing here
        };
        let policy = IcePolicy {
            interfaces: Some(vec![name.clone()]),
            ..IcePolicy::default()
        };

        let gathering = policy
            .gather_from()
            .expect("a name this machine answers to");

        assert_eq!(gathering, vec![GatherAddr::Named(vec![name])]);
        assert!(
            gathering[0]
                .to_socket_addrs()
                .expect("the names resolve")
                .any(|bind| bind.ip() == addr.ip()),
            "and resolving it — which the crate does at every bind — gives the \
             address that interface has now"
        );
    }

    /// An allow-list nothing answers is a misconfiguration, not a quiet
    /// fallback to every interface: a bridge that binds nothing has no path to
    /// a browser at all, and the offer it refuses says which name was wrong.
    #[test]
    fn an_allow_list_that_matches_nothing_is_an_error_that_names_it() {
        let policy = IcePolicy {
            interfaces: Some(vec!["bridge-nope0".to_string()]),
            ..IcePolicy::default()
        };

        let refused = policy.gather_from().expect_err("nothing to bind");

        assert!(refused.to_string().contains("bridge-nope0"), "{refused}");
    }
}
