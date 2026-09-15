# Transport Telemetry Spec

Status: **built** (2026-09-05; stages 1–5 below). Companion to `WebRTC Transport Spec.md`.

Amended 2026-09-15 by `Strict P2P Transport Spec.md`, which is binding where they
differ: the relay is no longer a data plane, so a session that never carries on a
peer connection is a session that never reached the device. The event `fell_back`
is `channels_lost` and the bucket "Relay only" is **Never connected**; the storage
column `fell_back_count` keeps its name (no migration) and counts the same thing.
`buildapp/transport_admin.py` cites §Classification below.

## The question this answers

"How many sessions are reliably on WebRTC, how many had to use TURN, and how many
never got a DataChannel at all?" — asked by the operator, answered nowhere (2026-09-05).
The bridge writes one stderr line per session naming the first path it carried on; the
relay logs sockets, not sessions; Cloudflare reports egress, not sessions; the SPA
`console.warn`ed "staying on the relay" where no one read it. A session that never
upgrades, one that loses its channels, and one that re-negotiates after an ICE restart
are all invisible.

## Who knows what

| Party | Knows | Does not know |
|---|---|---|
| Bridge | Every session it accepts, every peer it answers, the nominated pair at every (re)connect, every lost channel, every end | The browser's own view of its pair (see the `prflx` under-count in the transport spec) |
| Relay | Sockets and which sessions rode them | Whether a session also rides a DataChannel, or on what |
| Browser | Its own nominated pair, exactly | Nothing about other sessions |
| Api | Devices, owners | Nothing about sessions |

The bridge is the one party that sees all three outcomes for every session, so it is
the reporter. The browser's exact view is a later refinement (transport spec open
question 2) that would ride the same report.

## Design

**One row per session, kept by the api, written by the bridge, read by an admin page.**

### Events (bridge → api)

The bridge writes each event to stderr (the ledger the ops checklist greps — this is
"item 1") and POSTs the same event to the api. Both are content-free: ids, a path word,
a timestamp. No goal, no task, no repo, no frame.

| Event | When | Fields |
|---|---|---|
| `minted` | A `session_init` the registry admits as a new session (not a carrier re-attach) | — |
| `carrying` | The peer connection reaches `Connected` — the first time **and after every ICE restart** | `path`: `direct` \| `turn` |
| `channels_lost` | The session's last DataChannel carrier closed while the session lives — an ICE restart is under way, or the session is ending. Nothing carries for it meanwhile; there is no relay underneath. Was `fell_back`, which a bridge one release behind still sends | — |
| `ended` | The registry ends the session (client `close`, last carrier gone) | — |

`path` is the bridge's `NegotiatedPath`: `turn` when either end is a relay candidate,
else `direct`. The stderr line stays as it is (`carrying over host/relay candidates
(TURN, billed)`); the api receives the word.

### Wire (bridge → api)

`POST /api/transport/report`, a sibling of `/api/push/notify` with the same
authentication: an Ed25519 signature by the device identity key over a timestamped
challenge that binds every field, a freshness window, and a replay guard on
`(device_id, timestamp, signature)`.

```json
{"device_id": "…", "session_id": "sess-…", "event": "carrying", "path": "turn",
 "timestamp": 1757088000, "signature_b64": "…"}
```

Challenge: `transport.{device_id}.{session_id}.{event}.{path_or_dash}.{timestamp}`.
`path` is `-` when the event carries none. Unknown `event`/`path` words are refused.

Delivery is best effort: a report that fails is logged and dropped (the stderr ledger
is authoritative on the device); the bridge never blocks a session on the api. Reports
are sent from a task, in order per session, one in flight per bridge.

### Storage (api)

Table `transport_sessions`, one row per `(device_id, session_id)`:

| Column | Meaning |
|---|---|
| `session_id`, `device_id`, `owner_user_id` | Identity; owner copied from the device at mint so a later device deletion does not orphan the count |
| `minted_at` | `minted` |
| `first_carrying_at`, `first_path` | The first `carrying` |
| `current_path` | `relay` \| `direct` \| `turn` — last `carrying`, reset to `relay` by `channels_lost` (`relay` reads "not carrying"; the word is the column's, not a path any traffic takes) |
| `carrying_count`, `turn_count`, `fell_back_count` | Counters over the session's life. `fell_back_count` counts `channels_lost`; the column name is kept to avoid a migration and is commented in `models.py` |
| `ended_at` | `ended` |

Out-of-order or duplicate events are absorbed: a `carrying` before `minted` creates the
row; a second `minted` is a no-op; an event after `ended` is recorded but does not
unset `ended_at`.

### Classification (admin page)

Over a window (24 h / 7 d / 30 d, by `minted_at`, default 7 d):

| Bucket | Rule |
|---|---|
| **Direct WebRTC** | `first_path = direct`, `turn_count = 0`, `fell_back_count = 0` — "reliably using WebRTC" |
| **TURN** | `turn_count > 0` — had to relay through Cloudflare at least once (billed) |
| **Never connected** | never `carrying` — the browser never reached this device over a DataChannel, so nothing carried for it at all |
| **Unstable** | upgraded but lost its channels at least once (`fell_back_count > 0`, and no TURN) — shown beside Direct as its caveat |

Plus: sessions total, per-device rows (device name, owner, the four buckets), and the
last 50 sessions with their event trail. The page is `/admin/transport`, guarded like
the other admin pages (`administrator`), in the admin nav.

### Privacy and threat model

The api learns, per session: which device, which owner (already known), when, and which
of three transport words. It already learns device presence (a signed heartbeat since
`Strict P2P Transport Spec.md` rule 6; from the relay before it) and content-free notify
kinds from the bridge; this adds no content and no addresses — the
candidate types are words, never IPs. A hostile client cannot forge a report: it is
signed by the device identity key the api pinned at pairing. A hostile bridge can only
lie about its own sessions.

### Known limits

- The `prflx` under-count from the transport spec applies to `turn` vs `direct`: a
  browser on TURN whose connectivity check beat its trickle reads `direct` here. The
  admin page says so under the TURN figure, and Cloudflare's egress stays the billing
  authority. The browser reporting its own pair closes this (transport spec open
  question 2).
- Reports are best effort; the bridge's stderr ledger is the record of truth on the
  device. A bridge that cannot reach the api reports nothing for that span.

## Stages

Sequential, TDD, each green (tests, clippy/fmt or pytest, semgrep, gitleaks) before the
next.

1. **bridge-ledger** — `transport_ledger.rs`: the event enum, a `TransportLedger` sink
   trait with the stderr implementation, and the four hook points (registry mint/end,
   peer connect on every `Connected`, the last DataChannel closing). The `rtc:` line moves
   onto the ledger unchanged. Unit tests on the sink; `rtc_peer` asserts the trail.
2. **bridge-reporter** — `transport_report.rs`: the signed request (sibling of
   `notify.rs`), the ordered best-effort sender, wired as a second ledger sink.
   wiremock tests: signature/challenge bytes, ordering, failure is dropped not retried
   forever.
3. **api-endpoint** — model + migration, `/api/transport/report` with the notify
   authentication mirrored, upsert semantics per the storage rules. pytest against a
   fake device (the push notify tests' shape).
4. **admin-page** — `/admin/transport`: classification query, window selector, per-device
   table, recent sessions. pytest on the classification with fixture rows; a template
   test that the page renders the four figures.
5. **docs-and-ops** — `OPS.md` points the monthly TURN check at the page and keeps the
   grep as the device-side cross-check; transport spec threat model amended; README
   architecture note.
