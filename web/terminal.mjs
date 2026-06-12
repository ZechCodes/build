// E2EE terminal session — the client half of the bridge's terminal.
//
// Runtime-agnostic: a real browser (native WebSocket + ghostty-web) and the Node
// verification harness (`ws`) both drive it. It:
//   - bootstraps an E2EE session through the gateway/relay,
//   - demuxes incoming frames into RPC responses (by id) and live `term.output`
//     pushes (server-initiated PTY bytes),
//   - applies a screen SNAPSHOT on every (re)attach, then live-tails — snapshot
//     resync, not byte replay,
//   - auto-reconnects with backoff and re-attaches, reporting status so the UI can
//     show a disconnected state.

const te = new TextEncoder();
const b64encodeBytes = (u8) => btoa(String.fromCharCode(...u8));
const b64decodeBytes = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const timeout = (ms, msg) => new Promise((_, rej) => setTimeout(() => rej(new Error(msg)), ms));

export class TerminalSession {
  constructor({ url, transport, WebSocketImpl, deviceId = "bridge" }) {
    this.url = url;
    this.transport = transport;
    this.WS = WebSocketImpl;
    this.deviceId = deviceId;
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
    await this._call("term.input", { data: b64encodeBytes(te.encode(data)) });
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
    this._onStatus("connecting");
    if (this.transport.ready) await this.transport.ready();

    const ws = new this.WS(`${this.url}/ws/client`);
    this._ws = ws;
    const inbox = [];
    const waiters = [];
    const deliver = (m) => (waiters.length ? waiters.shift()(m) : inbox.push(m));
    const recvRaw = () => new Promise((res) => (inbox.length ? res(inbox.shift()) : waiters.push(res)));

    ws.addEventListener("message", (e) => {
      try { deliver(JSON.parse(typeof e.data === "string" ? e.data : e.data.toString())); } catch { /* ignore */ }
    });
    ws.addEventListener("close", () => {
      waiters.splice(0).forEach((w) => w(null)); // unblock the demux loop
      this._onDisconnect();
    });
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", reject);
    });

    // E2EE bootstrap.
    const hello = await recvRaw();
    if (!hello || hello.type !== "device_key") throw new Error("expected device_key");
    const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
    const { sessionKeyB64, sessionInit } = await this.transport.createSessionInit({
      sessionId, deviceId: this.deviceId, deviceTransportPublicKeyB64: hello.transport_public_key,
    });
    this._sessionId = sessionId;
    this._key = sessionKeyB64;
    ws.send(JSON.stringify({ type: "session_init", session_id: sessionId, session_init: sessionInit }));
    const accept = await recvRaw();
    if (!accept || accept.type !== "session_accept") throw new Error("expected session_accept");
    await this.transport.openSessionAccept({ sessionKeyB64, envelope: accept.envelope });

    this._onStatus("connected");
    this._backoff = 400;
    this._demux(recvRaw); // fire-and-forget: routes responses + pushes

    // Attach → apply the current screen snapshot, then live output flows as pushes.
    this._lastCursor = 0;
    const r = await this._call("term.attach", { cols: this.cols, rows: this.rows });
    this._lastCursor = r.cursor || 0;
    this._onSnapshot(b64decodeBytes(r.snapshot));
  }

  async _demux(recvRaw) {
    for (;;) {
      const msg = await recvRaw();
      if (!msg) return; // socket closed
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
      }
    }
  }

  async _call(method, params = {}) {
    const id = "r" + ++this._reqId;
    const envelope = await this.transport.encryptFrame({
      sessionKeyB64: this._key,
      outerFields: { session_id: this._sessionId, route_to: `device:${this.deviceId}` },
      frameFields: { frame_type: "data", sender: "client", payload: { method, id, params } },
    });
    const result = new Promise((resolve, reject) => this._pending.set(id, { resolve, reject }));
    this._ws.send(JSON.stringify({ type: "e2ee_envelope", session_id: this._sessionId, envelope }));
    return Promise.race([result, timeout(12000, `rpc ${method} timeout`)]);
  }

  _onDisconnect() {
    if (this._closed) return;
    this._onStatus("disconnected");
    for (const { reject } of this._pending.values()) reject(new Error("disconnected"));
    this._pending.clear();
    const delay = this._backoff;
    this._backoff = Math.min(this._backoff * 2, 8000);
    setTimeout(() => {
      if (!this._closed) this._connect().catch(() => this._onDisconnect());
    }, delay);
  }
}
