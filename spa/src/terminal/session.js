// E2EE terminal session — the client half of the bridge's terminal.
//
// Runtime-agnostic: a real browser (native WebSocket + ghostty-web) and the Node
// verification harness (`ws`) both drive it. It:
//   - bootstraps an E2EE session straight through the relay's /ws/client
//     (authenticate with a gateway token, then wait for the target device_key),
//   - demuxes incoming frames into RPC responses (by id) and live `term.output`
//     pushes (server-initiated PTY bytes),
//   - applies a screen SNAPSHOT on every (re)attach, then live-tails — snapshot
//     resync, not byte replay,
//   - auto-reconnects with backoff and re-attaches, reporting status so the UI can
//     show a disconnected state.

const textEncoder = new TextEncoder();
const b64encodeBytes = (u8) => btoa(String.fromCharCode(...u8));
const b64decodeBytes = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const timeout = (ms, msg) => new Promise((_, reject) => setTimeout(() => reject(new Error(msg)), ms));

export class TerminalSession {
  constructor({ url, transport, WebSocketImpl, getToken, preferDeviceId = () => null }) {
    this.url = url;
    this.transport = transport;
    this.WS = WebSocketImpl;
    this.getToken = getToken; // async () => gateway token, for the relay handshake
    this.preferDeviceId = preferDeviceId; // () => device id or null (any device)
    this.deviceId = null;
    this._pending = new Map();
    this._reqId = 0;
    this._onOutput = () => {};
    this._onSnapshot = () => {};
    this._onStatus = () => {};
    this._closed = false;
    this._lastCursor = 0;
    this._backoff = 400;
  }

  onOutput(fn) { this._onOutput = fn; }      // (Uint8Array) live PTY bytes
  onSnapshot(fn) { this._onSnapshot = fn; }  // (Uint8Array) full screen on (re)attach
  onStatus(fn) { this._onStatus = fn; }      // 'connecting'|'connected'|'disconnected'

  async start(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    this._closed = false;
    await this._connect();
  }

  /** Send keystrokes to the PTY. */
  async input(data) {
    await this._call("term.input", { data: b64encodeBytes(textEncoder.encode(data)) });
  }

  async resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    await this._call("term.resize", { cols, rows });
  }

  /** Permanent close — no reconnect. */
  close() {
    this._closed = true;
    try { this._ws && this._ws.close(); } catch { /* ignore */ }
  }

  /** Drop the socket but allow auto-reconnect (used to exercise reconnect). */
  simulateDrop() {
    try { this._ws && this._ws.close(); } catch { /* ignore */ }
  }

  async _connect() {
    const gen = (this._gen = (this._gen || 0) + 1);
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
      const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
      const { sessionKeyB64, sessionInit } = await this.transport.createSessionInit({
        sessionId, deviceId: this.deviceId, deviceTransportPublicKeyB64: hello.transport_public_key,
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

      // Attach → apply the current screen snapshot, then live output flows.
      this._lastCursor = 0;
      const r = await this._call("term.attach", { cols: this.cols, rows: this.rows });
      this._lastCursor = r.cursor || 0;
      this._onStatus("connected");
      this._backoff = 400;
      this._onSnapshot(b64decodeBytes(r.snapshot));
      this._startLiveness(gen);
    } catch (e) {
      try { ws.close(); } catch { /* ignore */ }
      throw e;
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
      } else if (p && p.type === "term.output") {
        if ((p.cursor || 0) > this._lastCursor) {
          this._lastCursor = p.cursor;
          this._onOutput(b64decodeBytes(p.data));
        }
      } else if (p && p.type === "term.reset") {
        // The bridge collapsed a huge burst to a screen snapshot — reset + apply.
        if ((p.cursor || 0) > this._lastCursor) {
          this._lastCursor = p.cursor;
          this._onSnapshot(b64decodeBytes(p.data));
        }
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
