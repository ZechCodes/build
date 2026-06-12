# Build web client (E2EE)

The browser side of Build. `client.mjs` holds the runtime-agnostic E2EE session
logic; it runs unchanged in a real browser (`index.html`, native WebSocket) and
in the Node end-to-end harness (`e2e.mjs`, the `ws` package). The crypto comes
from the audited [`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport)
JS binding — the browser never reimplements it.

It depends on that binding as a sibling checkout:

```
<parent>/
  Build/                     ← this repo (web/ lives here)
  build-secure-transport/    ← the audited E2EE binding (js/)
```

## End-to-end demo: browser client → relay → bridge → relay → browser

Proves the whole transport path with the real components — the JS client, a
minimal dev relay, and the real Rust bridge — fully E2E encrypted, relay-blind.

```bash
# 1. Install (the binding resolves to ../../build-secure-transport/js)
cd web && npm install
#    and install the binding's own deps once:
( cd ../../build-secure-transport/js && npm install )

# 2. Start the dev relay + in-process bridge device (echo handler)
( cd ../bridge && cargo run --example dev_relay )   # prints DEV_RELAY_LISTENING

# 3. Run the browser-client logic against it
npm run e2e        # → "E2E PASS: browser client ↔ relay ↔ bridge round-trip succeeded"
```

To drive it from a real browser instead of Node, serve this directory
(`python3 -m http.server`) and open `index.html` while the dev relay runs.

The dev relay (`bridge/examples/dev_relay.rs`) is a minimal stand-in for the
production `build-relay`: it forwards opaque envelopes between the device and the
client and never decrypts. Against the deployed relay, the same `client.mjs`
connects to the real client endpoint over `wss://`.
