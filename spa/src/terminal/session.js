// E2EE terminal socket — the client half of the bridge's keyed terminals.
//
// ONE socket per browser tab multiplexes every terminal (user shells + agent
// screens) by `term_id`, keeping PTY floods off the app RPC session (no
// head-of-line blocking of RPCs). It:
//   - bootstraps an E2EE session straight through the relay's /ws/client
//     (authenticate with a gateway token, then wait for the target device_key),
//   - demuxes incoming frames into RPC responses (by id) and live terminal
//     pushes (`term.output` / `term.reset` / `term.closed`) routed by `term_id`
//     to the registered terminal,
//   - applies a screen SNAPSHOT on every (re)attach, then live-tails — snapshot
//     resync, not byte replay — deduping output/reset on `cursor` PER term_id,
//   - auto-reconnects with backoff and RE-ATTACHES every registered terminal,
//     reporting status so the UI can show a disconnected state.
//
// Connecting no longer implies attaching: `start()` brings the socket up; each
// tab calls attachTerminal/attachAgent to register + attach its own term_id.

const textEncoder = new TextEncoder();
const b64encodeBytes = (u8) => btoa(String.fromCharCode(...u8));
const b64decodeBytes = (s) => Uint8Array.from(atob(s || ""), (c) => c.charCodeAt(0));
const timeout = (ms, msg) => new Promise((_, reject) => setTimeout(() => reject(new Error(msg)), ms));
const noop = () => {};

export class TerminalSocket {
  constructor({ url, transport, WebSocketImpl, getToken, getPinnedDeviceKey, preferDeviceId = () => null }) {
    if (typeof getPinnedDeviceKey !== "function") {
      throw new Error("getPinnedDeviceKey is required — refusing to trust relay-supplied device keys");
    }
    this.url = url;
    this.transport = transport;
    this.WS = WebSocketImpl;
    this.getToken = getToken; // async () => gateway token, for the relay handshake
    // async (deviceId) => the api-pinned transport key. The relay's device_key
    // push is a routing hint only — we never seal to a key the broker chose.
    this.getPinnedDeviceKey = getPinnedDeviceKey;
    this.preferDeviceId = preferDeviceId; // () => device id or null (any device)
    this.deviceId = null;
    this._pending = new Map();
    this._reqId = 0;
    this._onStatus = noop;
    this._closed = false;
    this._backoff = 400;
    // termId → { kind, taskId?, cols, rows, lastCursor, onOutput, onSnapshot, onClosed, onLive }
    this._terms = new Map();
    this._connected = false;
    this._connectWaiters = [];
  }

  onStatus(fn) { this._onStatus = fn; } // 'connecting'|'connected'|'disconnected'

  async start() {
    this._closed = false;
    await this._connect();
  }

  /** Resolve once the socket is connected (immediately if it already is). */
  whenConnected() {
    if (this._connected) return Promise.resolve();
    return new Promise((resolve) => this._connectWaiters.push(resolve));
  }

  // ---- terminal lifecycle (all ride this one socket) --------------------

  /** term.create — mint a user terminal in the given scope. */
  async createTerminal(scope, cols, rows) {
    await this.whenConnected();
    return this._call("term.create", { ...scope, cols, rows });
  }

  /** term.list — the open user terminals for a scope (never agent ids). */
  async listTerminals(scope) {
    await this.whenConnected();
    const r = await this._call("term.list", { ...scope });
    return r.terminals || [];
  }

  /** term.close — kill the server PTY and deregister locally. */
  async closeTerminal(termId) {
    try {
      await this.whenConnected();
      await this._call("term.close", { term_id: termId });
    } finally {
      this._terms.delete(termId);
    }
  }

  /** Register a user terminal and attach — the snapshot flows through opts.onSnapshot. */
  async attachTerminal(termId, opts = {}) {
    await this.whenConnected();
    const entry = this._register(termId, "user", null, opts);
    const r = await this._call("term.attach", { term_id: termId, cols: entry.cols, rows: entry.rows });
    entry.lastCursor = r.cursor || 0;
    entry.onSnapshot(b64decodeBytes(r.snapshot));
    return r;
  }

  /** Register a task's agent screen and attach — never errors on a dead session. */
  async attachAgent(taskId, opts = {}) {
    await this.whenConnected();
    const termId = `agent:${taskId}`;
    const entry = this._register(termId, "agent", taskId, opts);
    const r = await this._call("agent.attach", { task_id: taskId, cols: entry.cols, rows: entry.rows });
    entry.lastCursor = r.cursor || 0;
    entry.onSnapshot(b64decodeBytes(r.snapshot));
    entry.onLive(!!r.live);
    return r;
  }

  /** Deregister a terminal (tab unmounted) — the server PTY keeps running. */
  detach(termId) {
    this._terms.delete(termId);
  }

  /** Send keystrokes to a terminal's PTY. */
  async input(termId, data) {
    await this._call("term.input", { term_id: termId, data: b64encodeBytes(textEncoder.encode(data)) });
  }

  async resize(termId, cols, rows) {
    const entry = this._terms.get(termId);
    if (entry) { entry.cols = cols; entry.rows = rows; }
    await this._call("term.resize", { term_id: termId, cols, rows });
  }

  _register(termId, kind, taskId, opts) {
    const entry = {
      kind, taskId,
      cols: opts.cols || 80,
      rows: opts.rows || 24,
      lastCursor: 0,
      onOutput: opts.onOutput || noop,
      onSnapshot: opts.onSnapshot || noop,
      onClosed: opts.onClosed || noop,
      onLive: opts.onLive || noop,
    };
    this._terms.set(termId, entry);
    return entry;
  }

  /** Permanent close — no reconnect. */
  close() {
    this._closed = true;
    try { this._ws && this._ws.close(); } catch { /* ignore */ }
  }

  /** Drop the socket but allow auto-reconnect (device retarget / reconnect test). */
  simulateDrop() {
    try { this._ws && this._ws.close(); } catch { /* ignore */ }
  }

  async _connect() {
    const gen = (this._gen = (this._gen || 0) + 1);
    this._connected = false;
    this._onStatus("connecting");
    if (this.transport.ready) await this.transport.ready();

    const ws = new this.WS(`${this.url}/ws/client`);
    this._ws = ws;
    const inbox = [];
    const waiters = [];
    const deliver = (m) => (waiters.length ? waiters.shift()(m) : inbox.push(m));
    const recvRaw = (ms) =>
      Promise.race([
        new Promise((resolve) => (inbox.length ? resolve(inbox.shift()) : waiters.push(resolve))),
        ms ? timeout(ms, "handshake timeout") : new Promise(() => {}),
      ]);

    ws.addEventListener("message", (e) => {
      try { deliver(JSON.parse(typeof e.data === "string" ? e.data : e.data.toString())); } catch { /* ignore */ }
    });
    ws.addEventListener("close", () => {
      waiters.splice(0).forEach((w) => w(null)); // unblock the demux loop
      this._onLost(gen);
    });

    try {
      await Promise.race([
        new Promise((resolve, reject) => {
          ws.addEventListener("open", resolve);
          ws.addEventListener("error", reject);
        }),
        timeout(8000, "open timeout"),
      ]);

      // Authenticate to the relay with a gateway token so it routes us only to
      // our own devices; it acks, then pushes device_key per online device.
      const token = await this.getToken();
      ws.send(JSON.stringify({ type: "authenticate", token }));
      // E2EE bootstrap (time-boxed so a still-down bridge fails fast → retry).
      // Skip control frames (authenticated, other devices' keys) until the
      // target device's key arrives.
      const wanted = this.preferDeviceId();
      const deadline = Date.now() + 8000;
      let hello;
      for (;;) {
        const remaining = deadline - Date.now();
        if (remaining <= 0) throw new Error("no device online");
        const msg = await recvRaw(remaining);
        if (!msg) throw new Error("connection closed");
        if (msg.type === "device_key" && (!wanted || msg.device_id === wanted)) {
          hello = msg;
          break;
        }
      }
      this.deviceId = hello.device_id;
      // Seal to the api-pinned key; a relay-pushed key that differs means the
      // broker is substituting keys — abort instead of handing it the session.
      const pinnedKeyB64 = await this.getPinnedDeviceKey(this.deviceId);
      if (!pinnedKeyB64) throw new Error(`no pinned transport key for device ${this.deviceId}`);
      if (hello.transport_public_key !== pinnedKeyB64) {
        throw new Error("relay-supplied device key does not match the api-pinned key — possible tampering");
      }
      const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
      const { sessionKeyB64, sessionInit } = await this.transport.createSessionInit({
        sessionId, deviceId: this.deviceId, deviceTransportPublicKeyB64: pinnedKeyB64,
      });
      this._sessionId = sessionId;
      this._key = sessionKeyB64;
      ws.send(JSON.stringify({ type: "session_init", session_id: sessionId, route_to: `device:${this.deviceId}`, session_init: sessionInit }));
      let accept;
      for (;;) {
        accept = await recvRaw(6000);
        if (!accept) throw new Error("connection closed");
        if (accept.type === "session_accept") break;
      }
      await this.transport.openSessionAccept({ sessionKeyB64, envelope: accept.envelope });

      this._demux(recvRaw, gen); // routes responses + pushes

      // Re-attach every terminal registered before this (re)connect. On the very
      // first connect this is empty; after a drop it restores every open tab.
      await this._reattachAll();

      this._onStatus("connected");
      this._backoff = 400;
      this._connected = true;
      this._connectWaiters.splice(0).forEach((resolve) => resolve());
      this._startLiveness(gen);
    } catch (e) {
      try { ws.close(); } catch { /* ignore */ }
      throw e;
    }
  }

  /// Re-attach every registered terminal after a (re)connect. User terminals go
  /// through term.attach; an `unknown term_id` rejection means the server reaped
  /// it (scope vanished) → onClosed("reaped") + deregister. Agent screens go
  /// through agent.attach and refresh onLive. Each snapshot resets that term's
  /// cursor to the response cursor first (snapshot resync, not byte replay).
  async _reattachAll() {
    for (const [termId, entry] of [...this._terms]) {
      entry.lastCursor = 0;
      try {
        if (entry.kind === "agent") {
          const r = await this._call("agent.attach", { task_id: entry.taskId, cols: entry.cols, rows: entry.rows });
          entry.lastCursor = r.cursor || 0;
          entry.onSnapshot(b64decodeBytes(r.snapshot));
          entry.onLive(!!r.live);
        } else {
          const r = await this._call("term.attach", { term_id: termId, cols: entry.cols, rows: entry.rows });
          entry.lastCursor = r.cursor || 0;
          entry.onSnapshot(b64decodeBytes(r.snapshot));
        }
      } catch (e) {
        if (entry.kind === "user" && /unknown term_id/.test(e.message || "")) {
          this._terms.delete(termId);
          entry.onClosed("reaped");
        }
        // Other failures leave the entry registered — the next reconnect retries.
      }
    }
  }

  async _demux(recvRaw, gen) {
    while (this._gen === gen) {
      const msg = await recvRaw();
      if (!msg) return; // socket closed
      if (msg.type === "device_offline" && msg.device_id === this.deviceId) {
        // Our device dropped — the socket stays up, so trigger the reconnect path.
        this._onLost(gen);
        return;
      }
      if (msg.type !== "e2ee_envelope") continue;
      let frame;
      try {
        frame = await this.transport.decryptEnvelope({ sessionKeyB64: this._key, envelope: msg.envelope });
      } catch { continue; }
      const p = frame.payload;
      if (p && p.id !== undefined && p.ok !== undefined) {
        const pend = this._pending.get(p.id);
        if (pend) {
          this._pending.delete(p.id);
          p.ok ? pend.resolve(p.result) : pend.reject(new Error(p.error));
        }
        continue;
      }
      if (!p || !p.term_id) continue;
      const entry = this._terms.get(p.term_id);
      if (!entry) continue; // a frame for a term we don't render — drop it
      if (p.type === "term.output") {
        if ((p.cursor || 0) > entry.lastCursor) {
          entry.lastCursor = p.cursor;
          entry.onOutput(b64decodeBytes(p.data));
        }
      } else if (p.type === "term.reset") {
        // A huge burst collapsed to a screen snapshot — reset + apply.
        if ((p.cursor || 0) > entry.lastCursor) {
          entry.lastCursor = p.cursor;
          entry.onSnapshot(b64decodeBytes(p.data));
        }
      } else if (p.type === "term.closed") {
        // An agent screen that merely ended its session is RETAINED (the tab keeps
        // its last screen for the next session); user terminals deregister.
        if (!(entry.kind === "agent" && p.reason === "agent_session_ended")) {
          this._terms.delete(p.term_id);
        }
        entry.onClosed(p.reason);
      }
    }
  }

  // Application-level liveness: a relay/bridge/network outage does NOT always
  // close our socket, so we actively ping. A failed ping means the path to the
  // bridge is down → show disconnected and reconnect.
  async _startLiveness(gen) {
    while (this._gen === gen && !this._closed) {
      await new Promise((resolve) => setTimeout(resolve, 2000));
      if (this._gen !== gen || this._closed) return;
      try {
        await this._call("ping", {}, 3000);
      } catch {
        if (this._gen === gen) this._onLost(gen);
        return;
      }
    }
  }

  async _call(method, params = {}, timeoutMs = 12000) {
    const id = "r" + ++this._reqId;
    const envelope = await this.transport.encryptFrame({
      sessionKeyB64: this._key,
      outerFields: { session_id: this._sessionId, route_to: `device:${this.deviceId}` },
      frameFields: { frame_type: "data", sender: "client", payload: { method, id, params } },
    });
    const result = new Promise((resolve, reject) => this._pending.set(id, { resolve, reject }));
    this._ws.send(JSON.stringify({ type: "e2ee_envelope", session_id: this._sessionId, envelope }));
    return Promise.race([result, timeout(timeoutMs, `rpc ${method} timeout`)]);
  }

  /// Connection `gen` was lost. Show disconnected, invalidate it, and reconnect
  /// with backoff. Stale generations are ignored (no double-reconnect).
  _onLost(gen) {
    if (this._closed || gen !== this._gen) return;
    this._gen++; // invalidate this connection so its demux/liveness stop
    this._connected = false;
    this._onStatus("disconnected");
    for (const { reject } of this._pending.values()) reject(new Error("disconnected"));
    this._pending.clear();
    const delay = this._backoff;
    this._backoff = Math.min(this._backoff * 2, 8000);
    setTimeout(() => {
      if (!this._closed) this._connect().catch(() => this._onLost(this._gen));
    }, delay);
  }
}
