# Device pairing security checklist

**Status:** verified (16/16 controls)

**Scope:** what `build-bridge pair` prints and the approve link it prints,
#319. The bridge registers as pending with only the hash of a fresh pairing
code and a signature binding its keys to it, and a signed-in person approves
the code in Build (`bridge/src/pairing.rs`, `skriftapp/buildapp/devices_controller.py`).
That flow is unchanged here and not rescored. #372 additionally covers the
per-pairing browser hint used only for LAN discovery. #374 also covers bounded
credential-free conntrack probes during an encrypted peer negotiation. This checklist covers the link
`<web>/app/#/pair/<code>`, the screen it opens, and the terminal output around it.

## Threat model

The pairing code is the secret that pairs a device: whoever approves it binds
the device to their account. A link carrying the code is exactly as sensitive
as the code on screen, and no more. It must leak no further than the screen
does: no server log, no Referer, nothing that outlives the page. Its approve
screen must still make a person compare and press.

Anyone can make such a link. An attacker runs `build-bridge pair` on their own
machine and sends a signed-in victim `/app/#/pair/<their code>`. The victim
sees a device name the attacker chose and a fingerprint they have nothing to
compare with. Approving it gives the attacker's machine access to the
victim's account. So a sheet a link opened says it came from a link, and says
plainly what approving does and when to do it (control 9).

## Controls

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 1 | The code travels only in the URL fragment, which browsers never send in a request or a Referer: never in the path or query string. | [x] | `the_approve_link_opens_the_pairing_screen_with_the_code_in_the_fragment` (Rust); `pairLink.test.js` reads the code only from `#/pair/<code>`. |
| 2 | The SPA takes the code out of the address before the router or anything else reads the hash, replacing the history entry with `#/account/devices`, both on load and when the link is followed in an open tab. | [x] | `pairLink.test.js`: "takes the code the page opened on out of the address", "hands on a link followed while the app is already open, before the router reads it"; `main.js` installs `watchPairLinks` before `initRouter`. |
| 3 | The link never approves. It fills the code in and looks it up, and approving is still the person's press after comparing. | [x] | `addDeviceDom.test.js` "a code handed in by the approve link is filled in and looked up, never approved"; `gateOnboardingDom.test.js` "opens Add a device with the link's code". |
| 4 | Only a well-formed code is taken from the fragment: letters, digits and `-`, at most 32 characters. It is set as an input's value, never as markup. | [x] | `pairLink.test.js` "reads nothing from any other fragment, or from a code that could carry markup" (markup, encoded spaces, nested paths, 40 characters). |
| 5 | The lookup and the approval send the code in a POST body, never a URL. Both need a signed-in session, and the bridge never sends the raw code, only its hash. | [x] | `api.js` `lookupDevice`/`approveDevice` post JSON bodies; `/api/devices/lookup` and `/approve` carry `build_auth_guard`; `build_register_request_never_includes_raw_code`. |
| 6 | The person compares the same fingerprint the bridge printed. The approve screen shows the form the terminal shows (the first 32 hex digits of SHA-256 of the identity key, 128 bits, in fours), with the full fingerprint beneath it. | [x] | `the_short_fingerprint_is_its_first_thirty_two_hex_digits_in_fours`, `the_pairing_prompt_is_one_link_a_code_and_a_short_fingerprint` (Rust); `addDeviceDom.test.js` "shows the fingerprint in the short form the bridge printed, with the full one beneath". |
| 7 | Sign-in keeps the fragment without becoming a redirect. The fragment goes back only onto a same-origin redirect that has none of its own, and the result is that parsed URL's absolute href, so it can choose which page of this site opens and nothing else. | [x] | `passkey-signin.test.mjs`: "a signed-in visitor goes on with the fragment they arrived with", "a redirect with its own fragment, or to another origin, is left as the server said" (absolute and protocol-relative), "a same-origin path that starts with // stays on this site", "the page goes on with the fragment once the passkey is accepted". |
| 8 | The terminal output names no secret beyond what it did before. The retire notice names the directory the old identity was moved to, `~`-shortened under the home directory and in full outside it, and, for an api other than the default, the api. It prints no identity contents and no file names. | [x] | `a_retired_approval_says_so_in_two_short_lines`, `a_retired_approval_outside_home_names_the_full_directory`, `a_retired_approval_names_an_api_that_is_not_the_default`, `the_retire_message_names_the_api_and_where_the_old_identity_is`. |
| 9 | A sheet a link opened says so and warns before anything can be approved: only approve if you just ran the installer or `build-bridge pair` on a machine you own, because approving gives that machine access to your account. The typed-code subtitle is not shown for a link. | [x] | `addDeviceDom.test.js` "a sheet a link opened says so and warns before anything can be approved" (pins the copy), "a sheet opened by hand keeps its own subtitle and carries no link warning"; `gateOnboardingDom.test.js` "opens Add a device with the link's code, marked as opened by a link". |

## Paired LAN discovery hint (#372)

The optional hint is an unguessable client-held bearer value, not an
authenticated client identity. The SPA's UUIDv4 has 122 random bits, resides in
the paired browser's storage and travels only inside encrypted offers. Anyone
who can read it already has access to that browser's pairing. A holder of
another client's hint can cause one extra query to its already-validated LAN
address or replace the entry with their own freshly validated LAN address. It
cannot supply an address or bypass answer validation.

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 10 | `client_id` has the exact canonical 36-character UUID shape; malformed values are absent and never refuse the offer. The SPA sends it only to a bridge advertising `rtc.clientLanCache`. | [x] | `rtc::client_hint` parser regression; `rtcClientHint.test.js` and `peerLink.test.js` capability gating. |
| 11 | `crypto.randomUUID` creates a client-held UUIDv4 bearer hint with 122 random bits per paired bridge and pinned key. Only encrypted offers carry it; unpair and account replacement discard it. The first valid hint binds the encrypted session Opening through later offers, ICE restarts, peer recreation and carrier reattachment, so another cache entry requires another session and a validated LAN resolution. This is not authenticated client isolation. | [x] | `rtcClientHint.test.js`; pairing cleanup in `connection.js`; `rtc::client_hint` first-wins test; `closing_a_peer_does_not_let_its_session_rotate_or_forget_the_hint`; carrier reattachment/new-opening and concurrent-hint regressions. |
| 12 | The bridge cache is keyed solely by that bearer hint, memory-only, bounded to 64 LRU entries and one validated IPv4 address per hint, expiring one hour after validation even if used. A holder of another hint can query its already-validated address once or replace it with their own fully validated resolution; they cannot inject an address or skip validation. Hints and cached addresses never enter logs, pushes or diagnostic exports. | [x] | `rtc/mdns/cache.rs` bounds, LRU, expiry and separate-hint tests; mDNS validated-replacement regressions; review of hint plumbing and safe diagnostic sanitizer. |
| 13 | A hint permits at most one extra unicast query in the initial lookup, never on retry. All queries use QM, source port 5353 and the chosen LAN interface. | [x] | mDNS query wire-byte, initial-probe, retry and Linux source-egress regressions. |
| 14 | Cache hits never resolve a new name by themselves. Fresh replies still require matching name, valid arrival interface/subnet, private and not-self address, and live DNS TTL. Invalid answers cannot refresh the cache. | [x] | mDNS rotated-name, invalid-answer and stale-TTL regressions. |

## Unresolved-host conntrack sweep (#374)

The paired browser can trigger a bounded on-link sweep by delivering a valid
unresolved host candidate inside the encrypted negotiation. The sweep carries
no credentials or identity and never learns a candidate from a guessed address.
Only the normal authenticated ICE Binding Request can establish a peer-reflexive
endpoint. It opens outbound UDP conntrack state; it does not alter firewall rules.
Temporary scout sockets cause ARP discovery separately, so unresolved destinations
do not charge the ICE host socket. Neighbor hints authorize only a probe on an
already approved subnet; they do not authenticate a peer or resolve its mDNS name.

Chromium namespace proof on a mostly-empty /22 completed full scouting in about
15–24.4 seconds, depending on global neighbor-table pressure. Clustered discovery
took 369 ms after absence was proved (419 ms after candidate gathering) and selected direct with zero
restarts and no application traffic over TURN. Far-edge first-hit measurements
were 11.6–19.0 seconds (17.5 seconds in the final same-NAT run); a first hit can exceed Chromium's roughly 15-second
initial check window. The same encrypted session then upgrades through the one
existing optional restart, never a larger restart budget. Discovery retains its
25-second lifetime. The failed far-edge zero-restart run is retained on #374 as
evidence. Safari timing is inferred from source, not measured on iOS.

Matching current operational IPv4 srflx addresses are required before either
scout or sweep traffic. Missing evidence waits only within the existing lifetime;
unequal or never-arriving evidence emits no probes. This is a same-NAT load hint,
not peer authentication or proof of LAN membership. A process-wide interface key
and first successful scout-enqueue timestamp permit one discovery pass per
interface per 60 seconds,
including in-progress or canceled work. Later generations and sessions use only
fresh usable kernel neighbors. The timestamp registry caps unexpired interface
entries at 64 and admits no new interface when full. A fully sparse /22 therefore costs about 3,000 ARP
requests including default retries at most once per window; a different-NAT
session costs zero scout/sweep packets.

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 15 | The opt-in sweep uses the advertised host socket and its actual interface, only wholly RFC 1918 or IPv4 link-local on-link subnets of at most 1024 addresses. It excludes all local addresses, network and broadcast, shares a 200-packet/s ceiling, caps generation ports/attempts and permits only one repeat within 25 seconds while unresolved on relay. Send-queue gating reserves at least three quarters of the host socket buffer for ordinary ICE writes; probes yield without advancing under pressure. A bounded read-only snapshot prioritizes usable neighbors on the owning interface. Resolution, direct selection, close, new credentials and expiry erase destination state; no neighbor cache is retained. The 28-byte STUN indication contains only FINGERPRINT and a fresh transaction ID, without credentials, ufrags or session identifiers. Sweep logs/pushes contain no candidate names, addresses, ports or credentials. Guessed destinations are never registered as candidates. | [x] | Vendored subnet/payload/rate/cancellation and neighbor-parser tests, same-host-socket source-port and immediate ordinary-write regressions under ARP pressure, bridge stale-generation/privacy tests, Chromium namespace red/green with silent mDNS and inbound DROP, independent early/no-TURN-app-data and exactly-one-restart late cases, mostly-empty /22 timing and too-large-subnet skip. |
| 16 | Neighbor scouts use the same approved interface/subnet bounds, explicit source/interface and no-gateway routing, with at most five ephemeral sockets across the process. Their payload is one zero byte to UDP port 9, without credentials or identity. Scouts share the packet ceiling and quarter-buffer gate. Fresh incomplete observations plus unobserved admissions are capped at min(256, actual gc_thresh2 / 2); a separate global ARP-table guard includes failed entries and other namespaces. Missing, malformed or stale pressure observations admit nothing. Real ICE probes target only freshly usable neighbors. Both scout and sweep require intersecting current operational UDP/component-1 IPv4 srflx address sets; missing evidence waits within the unchanged lifetime, and unequal or absent evidence emits no probes or upgrade eligibility. Local evidence must belong to a current advertised live base socket. A bounded process-wide interface-key/timestamp reservation permits one scout pass per 60 seconds from its first successful enqueue, across peers, ports, aliases and generations; later work probes only fresh usable kernel neighbors. Cancellation closes sockets and erases addresses without resetting that timestamp, while numeric pressure reservations remain conservative until fresh observation. Cluster ordering changes only the order of authorized destinations. | [x] | Shared socket/admission/rate and lifecycle regressions, global-table parser and conflicting-state tests, exact destination-set ordering tests, srflx missing/mismatch/current-generation tests, two-session cooldown and known-neighbor reuse tests, genuine different-NAT zero-packet namespace proof, genuinely unknown far-edge and clustered /22 Chromium proofs, separate full-scout coverage control with actual ARP rate/pressure measurements, and immediate ordinary ICE writes under scout load. |

## Accepted

- The fingerprint a person compares is truncated to 128 bits (32 hex digits of
  the 256-bit SHA-256). Matching a substituted key's prefix while a code is
  pending would take a 2^128 second-preimage search. The full fingerprint is
  on the approve screen beneath it.
- The link warning is the defence against an attacker-crafted link. A person
  who ignores it and approves an attacker's code gives that machine access, as
  they would by typing the attacker's code.

- A signed-out visitor's sign-in page keeps `/auth/login?next=/app/#/pair/<code>`
  as its own history entry. That entry is on the same machine and in the same
  browser that showed the code, and the code stops pairing anything once it is
  approved.
