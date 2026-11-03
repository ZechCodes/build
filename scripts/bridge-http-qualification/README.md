# REST/SSE client qualification

This runs the built SPA in Chromium and the real desktop HTTP module against a
cargo-built bridge and the real local Skrift application. Every invocation creates
a new disposable SQLite database, paired account/device identity, temporary HOME,
private credentials, and CA-signed localhost certificate under
`/home/zech/Projects/.agent-scratch/976/run-*`. It leaves logs and screenshots there
after bounded child shutdown. It never runs an installed bridge or changes system
certificate trust. All service addresses are loopback.

Before the client lifecycles, `discovery.py` obtains proofs from the actual bridge
route and submits them to the real hosted completion route. It proves valid
completion and refusal of a tampered signature, a mismatched pending origin,
an expired challenge, and a reused challenge. Each refused response must omit
credentials and leave the disposable token-row count unchanged. Only the expired
case changes a challenge's TTL in the throwaway SQLite DB; production TTLs and
authority records remain untouched. `discovery-evidence.json` records statuses and
row deltas without tokens, nonces or signed proofs.

Use Node 22.23.4 and npm 12.0.2 on PATH. From the repository root, install the
committed dependency locks and build the SPA:

```sh
nice -n 10 mkdir -p /home/zech/Projects/.agent-scratch/976/tmp
TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp \
  nice -n 10 npm ci --prefix transport/js --ignore-scripts
TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp \
  nice -n 10 npm ci --prefix spa --legacy-peer-deps
TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp VITE_BUILD_CHANNEL=dev \
  VITE_RELAY_URL=ws://127.0.0.1:18477 nice -n 10 npm --prefix spa run build
TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp \
  UV_PROJECT_ENVIRONMENT=/home/zech/Projects/.agent-scratch/976/venv \
  nice -n 10 uv sync --frozen --project skriftapp
```

Build the bridge only after #975's approved dependency SHA is available:

```sh
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/home/zech/Projects/.agent-scratch/976/target \
  TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp \
  nice -n 10 cargo build --manifest-path bridge/Cargo.toml --bin build-bridge
```

Run qualification with the explicit scratch binary. The runner refuses missing,
relative, installed, or symlink-escaped binary paths. It does not compile Rust.
Ports 18476 (issuer) and 19476 (bridge) must be free; `--issuer-port` and
`--bridge-port` select other unprivileged loopback ports.

```sh
BUILD_HTTP_QUALIFICATION_BINARY=/home/zech/Projects/.agent-scratch/976/target/debug/build-bridge \
  TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp \
  nice -n 10 /home/zech/Projects/.agent-scratch/976/venv/bin/python \
  scripts/bridge-http-qualification/run.py
```

The exact client commands inside that run are:

```sh
BUILD_HTTP_QUALIFICATION_FIXTURE=<run-dir>/runtime.json \
  NODE_EXTRA_CA_CERTS=<run-dir>/ca.pem nice -n 10 node \
  scripts/bridge-http-qualification/browser.mjs
BUILD_HTTP_QUALIFICATION_FIXTURE=<run-dir>/runtime.json \
  NODE_EXTRA_CA_CERTS=<run-dir>/ca.pem nice -n 10 node --test \
  desktop/test/bridge-http-qualification.test.mjs
```

These commands require the runner's disposable services to remain running.
The browser independently signs into the real dummy-auth form; the desktop test
uses a fixture token created by Skrift's real OAuth signer for the same account.
The fixture seeds current schema metadata, an already redeemed invite and an
approved device; it does not test migrations, manual invite redemption or pairing
UX. Runtime credentials and generated identity keys remain private in scratch and
must not be committed or attached as evidence. Attach the client logs and SPA
screenshot, which contain the assertions and observed events without bearer data.

The standalone discovery command uses the same fixture:

```sh
BUILD_HTTP_QUALIFICATION_FIXTURE=<run-dir>/runtime.json nice -n 10 \
  /home/zech/Projects/.agent-scratch/976/venv/bin/python \
  scripts/bridge-http-qualification/discovery.py
```

Runner boundary checks execute without a bridge:

```sh
TMPDIR=/home/zech/Projects/.agent-scratch/976/tmp nice -n 10 \
  /home/zech/Projects/.agent-scratch/976/venv/bin/python -m unittest discover \
  -s scripts/bridge-http-qualification -p test_runner.py
```
