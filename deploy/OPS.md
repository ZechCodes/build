# Ops checklist

Recurring checks on the live stack (namespace `8ly`). One-time deploy and
cutover steps live in [`k8s/CUTOVER.md`](k8s/CUTOVER.md); how to run the stack
locally is in [`README.md`](README.md).

## Diagnosing a dropped device connection

First check that the bridge process itself is stable. On macOS,
`launchctl print gui/$(id -u)/ing.getbuild.bridge` shows its PID, run count,
and last terminating signal. Repeated startup messages and a changing PID
point to process restarts, not necessarily a WebRTC failure.

Deployment helpers must be one-shot jobs, never `KeepAlive` services. A helper
that successfully restarts the bridge and exits will otherwise be launched
again indefinitely. After deployment, verify that the helper job is removed
and the bridge PID remains unchanged beyond the helper's delay. Keep
`KeepAlive` enabled for the bridge daemon itself.

Before refreshing a browser tab that lost its connection, run
`JSON.stringify(buildConnectionDiagnostics(), null, 2)` in its developer console.
This returns the last 100 connection events, including negotiation, state changes,
restart outcomes and deadlines. The history stays in that tab's memory and is
cleared by a reload. It contains timestamps and connection identifiers, not SDP,
candidate addresses, credentials or application content.

Match the session identifier and timestamp with the bridge's stderr. A macOS
LaunchAgent writes this to `~/.build/log/bridge.err.log`; a Linux service writes
to `journalctl --user -u build-bridge`. Diagnostic lines contain `timestamp_ms`
(Unix milliseconds) and the session identifier. They distinguish:

- Peer and ICE connection-state changes.
- DataChannel opening, closing, read termination and write failure.
- A session binding to a channel and the number of carriers remaining when one
  closes. Releasing the relay with a channel still bound should keep it alive.
- A client close frame, an explicit `rtc.close`, and the relay releasing a session.

Collect both sides for the same attempt: a successful ICE pair alone does not
prove the application session was attached to its DataChannel before signaling
closed. A restart is successful only once the peer is connected and both channels
are open again.

The app and terminal streams use separate encrypted sessions on the same peer.
After a bridge restart, both need fresh sessions. A terminal session must receive
an acknowledged channel request before its rendezvous lease is released, even
when no terminal panes are mounted. A terminal ping timeout can close the shared
peer without an earlier ICE failure; correlate terminal diagnostics as well as
peer state changes when investigating a five-second drop.

## Priority: the bridge, the user's apps, the agents

The bridge relays the user's live phone session, so it has to answer while the
agents it spawns saturate the machine, and the user's own apps have to stay
usable too. Under contention the order is **bridge > user's apps > agents**.

On Linux, with a user systemd:

| cgroup | CPUWeight | memory |
| --- | --- | --- |
| `build-bridge.service` (the bridge) | 500 | no limit |
| the user's apps (`app.slice` scopes and services) | 100, systemd's default | no limit |
| `app-build_agents.slice` (every agent the bridge spawns) | 20 | `MemoryHigh` = 75 % of RAM, whole MiB (`98304M` on 128 GiB) |

Weights only arbitrate between sibling cgroups. Every agent used to run inside
`build-bridge.service`'s own cgroup, so a weight on the bridge unit ranked the
bridge *plus its agents* against the user's apps and never protected the bridge
from its own children. With the agents in their own slice, the bridge, the
user's apps and the agents are siblings under `app.slice`. One catch: a slice's
name is its path, dash by dash: `app-build_agents.slice` is a direct child of
`app.slice`, the level the bridge's unit and the user's apps are ranked at. (A
name with a second dash would nest one level down and rank against nothing.)

### How every child gets there

Every child the bridge spawns goes the same way, whichever harness spawns it:
the terminal agents and the user's terminals (a PTY), the headless Claude Code
and Codex App Server agents (pipes). The child is started **once**, at a gate:
`/bin/sh` reading one line from a FIFO, after which it `exec`s the real
command. While it waits (forked, nothing run, nothing forked) the bridge places
its pid: one `busctl --user call … StartTransientUnit` puts the pid in a
transient scope of its own (`build-agent-p<bridge pid>-<n>.scope` under
`app-build_agents.slice`; `build-terminal-…` directly under `app.slice`), the
move is checked in `/proc/<pid>/cgroup`, the child is niced (agents +10;
terminals +5 only when there is no scope to rank them), and the line is
written. Everything the child ever runs descends from that pid, in its scope,
at its nice.

Each scope carries `BindsTo=`/`After=` on the bridge's own unit, so stopping or
restarting `build-bridge.service` stops every scope with it, as
`KillMode=control-group` did while the children shared the unit's cgroup; the
bridge also stops the scopes it started on its own way down
(`children: stopped N scopes` in `bridge.err.log`).

A scope that cannot be had (no `busctl`, a bus that does not answer, a pid that
never arrives) costs the child nothing but the scope: it is released niced, the
bridge logs one line (`children: …; from now on nice 10 only, no scope`) and
asks for no more scopes until it is restarted. The child is never started
twice, and nothing the manager says reaches the terminal. `BRIDGE_CHILD_SCOPE=0`
turns the scope off. The bridge probes the manager the same way at startup
(`children: transient scopes under app-build_agents.slice, bound to
build-bridge.service, nice 10; …`).

`build-bridge install-service` writes `CPUWeight=500` into the unit and runs
`systemctl --user set-property` on the agents' slice (a persistent drop-in under
`~/.config/systemd/user.control/`). A set-property that fails (an older systemd,
a machine without user slices) prints one stderr line, and the install carries
on: the bridge still works, but its agents keep the default priority.
`uninstall-service` leaves the slice settings in place, because a reinstalled
bridge's agents use them again.

There is no `MemoryMax` anywhere: nothing is killed. Above `MemoryHigh` the
kernel slows the agents' allocations and reclaims their memory, page cache
first, before it touches the bridge or the user's apps.

Where there is no user systemd (macOS, containers), the bridge falls back to
`nice +10` on each agent and `nice +5` on each terminal, set at the gate before
the child runs a thing, so the order bridge, then the user's shell, then the
agents still holds. On macOS the LaunchAgent is
`ProcessType=Interactive`. `Background` is launchd's throttled class, with CPU
and I/O deprioritised behind everything the user does. The bridge relays the
user's live session, so it gets the class launchd gives an app with a UI.

Check a machine:

```bash
systemctl --user show app-build_agents.slice -p CPUWeight,MemoryHigh   # 20, 75 % of RAM
systemctl --user show build-bridge.service -p CPUWeight,DropInPaths    # 500, no user.control drop-in
systemd-cgls --user   # agents under app.slice/app-build_agents.slice, not build-bridge.service
systemctl --user list-units 'build-agent-*' 'build-terminal-*'   # one scope per live child, bound to the unit
```

**Roll note.** `systemctl --user set-property build-bridge.service CPUWeight=…`
writes a drop-in, `~/.config/systemd/user.control/build-bridge.service.d/50-CPUWeight.conf`
(with `--runtime`, the same path under `/run/user/$UID/systemd/`), and a
drop-in overrides the unit file's `CPUWeight=500`. On a machine that ever ran
one, do this at roll time:

```bash
rm ~/.config/systemd/user.control/build-bridge.service.d/50-CPUWeight.conf
# or reset it instead: systemctl --user set-property build-bridge.service CPUWeight=500
systemctl --user daemon-reload
```

`DropInPaths` above lists every drop-in still in force.

## Monthly — Cloudflare TURN usage

Cloudflare TURN is free to 1000 GB of egress to clients per month and $0.05
per GB after that. Only relayed sessions bill: a peer connection that
settles on a direct (`host`, `srflx` or `prflx`) candidate pair costs
nothing, and a session that never upgrades off the relay costs nothing here
either.

1. Read the month's TURN egress in the Cloudflare dashboard (Realtime → TURN),
   or query the same numbers from Cloudflare's GraphQL analytics api. Traffic
   shows up within 30 seconds, so the figure is current, not a billing-cycle
   estimate.
2. Compare it against the 1000 GB included. Above it, budget $0.05 per further
   GB.
3. If egress is climbing, ask how many sessions are actually being relayed.
   **The admin transport page** (`/admin/transport`, administrator permission)
   answers first: sessions in the window bucketed as Direct WebRTC / TURN /
   Never connected / Unstable, per device, with the newest sessions' trails. Every
   bridge reports each session's transport events to the api
   (`POST /api/transport/report`, device-signed, content-free), and the page
   counts a session as TURN when any of its carries was relayed. The bridge's
   own stderr is the device-side cross-check: one line per event, and one
   per path each time its peer connection carries — the first time, and
   again after every ICE restart:

   ```
   transport: session <session_id> minted over the relay
   rtc: session <session_id> carrying over host/relay candidates (TURN, billed)
   transport: session <session_id> lost its last channel
   transport: session <session_id> ended
   ```

   The pair is named at both ends, device first, browser second. Count the
   billed carries on a device with:

   ```bash
   grep -c 'TURN, billed' bridge.err.log
   ```

   `host`, `srflx` and `prflx` are the direct, free paths; `relay` at
   **either** end is the billed one — and the usual billed shape is
   `host/relay`: a device on a home box pairing its own host candidate with a
   browser that could only reach it through TURN. The bridge states the bill
   itself so the count needs no rule about which side to read. `unknown` means
   the bridge found no nominated pair in its stats report — never billed, but
   a rise in `unknown` is a bridge bug, not TURN usage. A rise in billed lines
   without a rise in users means more clients are failing to hole-punch, not
   that each client is moving more bytes.

   One under-count to know about: a browser end that reads `prflx` is a
   candidate the bridge's ICE agent discovered from the browser's connectivity
   check before the browser's own trickled candidate arrived — the usual
   order with Chrome. The bridge cannot tell whether that address is the
   browser's host or a TURN allocation, so a browser relaying through TURN
   can log as `host/prflx`, unbilled. Cloudflare's own egress figure (step 1)
   is the authority; the bridge count is the lower bound. Closing the gap
   needs the browser to report its own nominated candidate type over the
   session (tracked in the transport spec's open questions).

   Before trusting the count after a bridge change, prove the billed path
   once from a machine with the TURN key: mint a list and run the relay-only
   peer test, which crosses Cloudflare TURN for real and asserts the pair
   read both ways:

   ```bash
   export BUILD_ICE_SERVERS_JSON="$(cd skriftapp && uv run --frozen python -c \
     'import os,json; from buildapp.ice_servers import ice_servers; \
      print(json.dumps(ice_servers(os.environ["CF_TURN_KEY_ID"], os.environ["CF_TURN_KEY_API_TOKEN"])))')"
   (cd bridge && nice -n 10 cargo test --test rtc_peer a_browser_that_can_only_relay -- --nocapture)
   ```

   Without the variable the test skips, which is how CI runs it.

## Monthly — the TURN key still works

Egress falling to zero while relay traffic holds is not everyone hole-punching:
it is what a dead key looks like. Cloudflare rejecting `CF_TURN_KEY_ID` /
`CF_TURN_KEY_API_TOKEN` makes `POST /api/rtc/ice-servers` answer 502, which
makes the browser's upgrade fail and leaves every session on the relay —
working, slower, and silent about why. Zero egress plus doubled relay load is
that failure, not good news.

1. Ask the api for a list with the key the pod actually holds:

   ```bash
   kubectl --context do-nyc1-production-hosting -n 8ly exec deploy/build-app -- \
     python -c "import os; from buildapp.ice_servers import ice_servers; \
       print(ice_servers(os.environ.get('CF_TURN_KEY_ID', ''), os.environ.get('CF_TURN_KEY_API_TOKEN', '')))"
   ```

   A TURN entry with `username` / `credential` is a live key. An
   `IceServersUnavailable` traceback naming Cloudflare's status is a dead one.
   The bare `stun:stun.cloudflare.com:3478` list means no key is configured at
   all, which is a supported deployment (see [`README.md`](README.md)) but not
   what a production pod should print.
2. Replace a dead key. `k8s/bootstrap-secrets.sh` only adds the keys when they
   are missing, so it will not overwrite one, and the Deployment reads the
   Secret at start:

   ```bash
   kubectl --context do-nyc1-production-hosting -n 8ly patch secret build-app --type merge \
     -p '{"data":{"CF_TURN_KEY_ID":"<base64>","CF_TURN_KEY_API_TOKEN":"<base64>"}}'
   kubectl --context do-nyc1-production-hosting -n 8ly rollout restart deploy/build-app
   ```

   Then re-run step 1 against the new pod.
