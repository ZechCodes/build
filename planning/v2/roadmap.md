# Build v2 — Production Architecture & Roadmap

**Status:** Draft for build. Companion to `E2EE Platform Scope.md` and `UI Design Brief for E2EE Platform.md`.
**Context:** Full break from the chat-first v1. New model = planning/goals, review-first. Everything moves into one public (BSL) monorepo.

---

## 0. Decisions (locked)

- **Task intake stays goal-form + batched plan/diff comments. There is no chat
  UI.** The user states a goal, then reviews: comments on the plan and on the
  diff are batched into notes the agent receives at phase boundaries. Free-form
  back-and-forth chat is deliberately out — review is the product, and the
  terminal drawer is the escape hatch when direct interaction is truly needed.
- **Broker topology: one merged Rust broker, and it is a rendezvous.** The relay
  (`bridge/src/bin/relay.rs`) terminates both `/ws/device` (Ed25519 challenge
  auth) and `/ws/client` (api-minted gateway-token auth). The separate
  `gateway` tier and Redis frame bus described in §2 are **retired**; browsers
  connect straight to the relay, which validates against the api over
  `/internal/*` with `X-Internal-Secret`. Since 2026-09-15 it is not a data
  plane at all (`Strict P2P Transport Spec.md`, binding): it carries session
  setup and `rtc.*` signaling, the browser closes the socket once the
  DataChannels are open, and the bridge refuses any other frame offered to it
  over a relay carrier. Presence is the api's, from a device-signed heartbeat.
- **`api` language: Python on the Skrift framework** (`skriftapp/`), with
  Skrift's passkey auth in production. **relay: Rust** (shares the transport
  binding with the bridge).
- **Browser-session token:** short-lived (5-min) opaque gateway token minted by
  the api, validated by the relay via an internal api call (not a JWT).
- **Web client: Vite vanilla-ES-module SPA** (`spa/`, no framework), all
  dependencies self-hosted (zero CDN), built into the api's static dir. The
  React scaffold (`frontend/`) is deleted.

Sections below predate these decisions and are kept for context; where they
conflict (gateway tier, Redis, §6 open decisions), the list above wins.

---

## 1. Where we are

Already built and validated (this is the differentiated, higher-risk half):

- **bridge/** (Rust) — task lifecycle, worktree-per-task, full-PTY harness (no SDK), single `done` MCP tool, git-diff watcher, orchestrator, **E2EE transport binding** (Rust port of `build-secure-transport`, interop-verified vs Python/JS), relay client, and the **E2EE app RPC** (`task.*`, `stream.*`, `term.*`).
- **Terminal over E2EE** — server-side `vt100` screen model, snapshot-resync (not byte replay), output coalescing. Renders Claude Code / Codex cleanly; idle keystroke RTT ~12 ms through the full path; survives client *and* relay/bridge reconnects.
- **Reliability** — snapshot+cursor resync per surface, liveness-ping disconnect detection, generation-guarded reconnect. Verified over the **real relay** (Postgres-backed Ed25519 auth, Redis fan-out).
- A **gateway shim** (Node) stood in for the web backend's relay-facing half — a prototype of the real `gateway`, not throwaway.

Not yet real:
- The **agent path is the QA scripted agent**, not a real CLI harness reporting `done` over MCP (the done-forward seam is unbuilt).
- No real **web backend** (auth/accounts/registry), no real **web app** (the React SPA), no **device pairing** flow.

---

## 2. Target architecture — deploy-isolated connection tiers

The core operational principle: **a connection tier you redeploy often must not hold the connections you can't afford to drop.** So we split by *deploy cadence + connection lifetime*, not just by function.

```
                         STABLE TIER (rare deploys, holds live connections)
  ┌────────────┐  e2ee  ┌──────────┐                 ┌──────────┐  e2ee  ┌────────────┐
  │  web (SPA) │◄──ws──►│ gateway  │◄────Redis──────►│  relay   │◄──ws──►│   bridge   │
  │  (browser) │        │ (browser │   ciphertext    │ (device  │        │ (user box) │
  └─────┬──────┘        │  broker) │                 │  broker) │        └────────────┘
        │ REST          └──────────┘                 └──────────┘
        ▼
  ┌────────────┐   CHURNY TIER (frequent deploys, no persistent connections)
  │    api     │ — auth/login · accounts · device registry/approval/pairing ·
  │ (app srv)  │   serves the SPA · settings (projects/harnesses/templates) · push
  └─────┬──────┘
        ▼  shared Postgres (users, devices, sessions)  ·  Redis (frame bus)
```

**The tiers:**

- **relay** — device connection broker. Terminates `/ws/device` (Ed25519 challenge auth vs the `devices` row), routes opaque ciphertext. *Stable; ~zero v2 changes* — it only routes envelopes; all v2 changes live in the inner frames it never sees. **Don't churn it.** (Keep Python for now; a Rust port is a later nicety, not blocking.)
- **gateway** — browser connection broker. Terminates the browser WebSocket, validates a session token minted by `api`, enforces device ownership, and bridges E2EE frames to/from the relay (via Redis). *Stable.* **This is the new tier that makes browser sessions survive `api` deploys** — promote the shim into a real service.
- **api** — the application server. Login/sessions, accounts, **device registration/approval/pairing**, serves the SPA, settings surfaces, notifications/push. *Churny — redeploy freely.* Holds no persistent E2EE connection, so a deploy drops only retryable request/response traffic.
- **bridge** — the Rust device daemon (built).
- **web** — the React SPA (greenfield; the review-first model).

**Why both stable brokers, separate:** devices and browsers have opposite scaling profiles (few long-lived vs many ephemeral), and keeping the proven device relay untouched honors "don't change the stable tier." They *could* be merged into one broker if you'd rather run fewer services; splitting later is cheap.

**Still fully E2EE.** Neither broker holds a session key — the client seals the session key to the *device's* transport key; brokers forward ciphertext only. Auth/accounts living in `api` doesn't weaken content confidentiality. Verified-device key-pinning is unaffected. Making the whole repo public (BSL) *strengthens* the trust pitch: local custody + E2EE you can audit.

---

## 3. Monorepo layout (one public repo, BSL)

```
build/
├── proto/        the versioned E2EE app-RPC contract — the seam everything codes to
├── transport/    E2EE protocol + bindings (Rust for relay?/gateway?/bridge, JS for web)
├── relay/        stable device broker (Python, ~as-is)
├── gateway/      stable browser broker (promote the shim)
├── api/          churny app server (auth, accounts, registry, SPA, settings, push)
├── bridge/       Rust device daemon (drops in)
└── web/          React SPA
```

A single repo ≠ a single deployable: each of relay/gateway/api/bridge/web ships independently.

---

## 4. The contract: `proto/`

The E2EE app RPC (`{method, id, params}` → `{id, ok, result|error}`, plus server-push frames like `term.output`) is the API between web ↔ bridge. It rides the brokers as opaque ciphertext. **Version it and treat it as the public contract.** It is what makes tandem (feature-by-feature) development safe: define a feature's request/response/push shapes once; build web + bridge to them in parallel. Existing methods to formalize: `task.dispatch/list/get/plan/diff/approve_plan/approve_merge/abandon`, `stream.start/events/state`, `term.attach/input/resize` (+ `term.output`/`term.reset` pushes).

---

## 5. Roadmap

### Phase 0 — Prerequisites (unblock everything)
1. Stand up the monorepo with the layout above; move `bridge/` + `transport/` in.
2. Write the `proto/` spec from the methods the bridge already implements.
3. Scaffold `gateway/` (promote the shim: browser WS + token auth + relay bridge) and `api/` (auth + device registry skeleton + serves the SPA). Pick the `api`/`gateway` language (see §6).

### Parallel track (bridge, contract-independent — start now)
- **MCP `done`-forward.** The one thing that turns the lifecycle from "QA scripted agent" into real: a per-task control socket so `build-bridge mcp --task <id>` (launched by the harness via `.mcp.json`) forwards the agent's real `done` to the daemon → `on_done`. The `pty` and `mcp` server pieces exist; this is the missing seam. Gates slices 3–4.

### Phase 1 — Vertical slices (tandem: define contract → build web + api + bridge together → ship)
1. **Auth + pairing + device-online** — login (api), device register→approve (api), the gateway token handshake, device status in the SPA. Blocking foundation.
2. **Terminal** — proven and de-risked. "Log in → drop into your agent from anywhere" is shippable on its own and exercises the whole real stack.
3. **Task dispatch + board** — needs the done-forward track landed.
4. **Plan review** → **Diff review** — the product headline (review is the product); batched notes/comments, changed-since markers.
5. **Notifications + settings** — push (encrypted payloads, client-rendered), devices & keys, projects, harnesses.

---

## 6. Open decisions

- **`api` / `gateway` language.** Rust (shares transport + types with the bridge; auditable; consistent) vs TS (web-framework velocity, shares the React language + JS binding). `api` is the churny web tier where TS velocity helps most; `gateway` is thin/stable and benefits from the Rust transport binding. Recommendation leans Rust for `gateway`, either for `api`.
- **relay:** keep Python (recommended — stable tier, don't churn) vs Rust port (consistency, later).
- **Broker topology:** separate relay + gateway (recommended — independent scaling, relay untouched) vs one merged broker (fewer services).
- **Browser-session token:** signed JWT (gateway validates with a shared public key, no api round-trip) vs Redis session lookup. JWT keeps the gateway independent of `api` uptime.
