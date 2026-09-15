// E2EE terminal socket — the client half of the bridge's keyed terminals.
//
// ONE socket per browser tab multiplexes every terminal (user shells + agent
// screens) by `term_id`, keeping PTY floods off the app RPC session (no
// head-of-line blocking of RPCs). It:
//   - routes what core/sessionRpc.js hands it — every frame that answers no
//     call of its own — to the terminal the frame names (`term.output` /
//     `term.reset` / `term.closed`, by `term_id`),
//   - applies a screen SNAPSHOT on every (re)attach, then live-tails — snapshot
//     resync, not byte replay — deduping output/reset on `cursor` PER term_id,
//   - RE-ATTACHES every registered terminal whenever a wire starts carrying,
//     reporting status so the UI can show a disconnected state.
//
// The relay socket under it — the handshake, the backoff reconnect, the session
// that outlives both — is core/relayLink.js, and which wire is riding is
// core/sessionSwitch.js. Nothing in this file opens a socket.
//
// Connecting no longer implies attaching: `start()` brings the socket up; each
// tab calls attachTerminal/attachAgent to register + attach its own term_id.
//
// The socket it handshakes on is this session's FIRST carrier, not its only
// one: `peer(carrier)` hands it the `term` DataChannel to ride instead, and
// every method above the wire is unchanged.

import { createRelayLink } from "../core/relayLink.js";
import { createSessionRpc } from "../core/sessionRpc.js";
import { createSessionSwitch } from "../core/sessionSwitch.js";

const textEncoder = new TextEncoder();
const b64encodeBytes = (u8) => btoa(String.fromCharCode(...u8));
const b64decodeBytes = (s) => Uint8Array.from(atob(s || ""), (c) => c.charCodeAt(0));
const noop = () => {};

/** How long the device has to accept the session this socket presents. A
 *  bridge that is still coming up fails fast and the link tries again. */
const HANDSHAKE_TIMEOUT_MS = 6000;

/** How many frames one not-yet-named agent screen may hold while its attach is
 *  in flight. A repainting TUI is a handful of coalesced frames in that window;
 *  the cap is what keeps a pathological flood from growing without bound. */
const ORPHAN_FRAME_LIMIT = 64;

/** How long a decrypted frame vouches for the connection.
 *
 *  Every frame for this client rides ONE FIFO (bridge → relay → browser), so a
 *  terminal flooding output queues the pong behind its bytes: pinging a busy
 *  stream measures the backlog, not the connection, and times out on a path
 *  that is plainly alive. Any frame we decrypted is itself proof the bridge is
 *  reachable, so within this window we skip the probe entirely — busy is not
 *  dead. Only real silence past it is worth a ping. */
const FRAME_PROOF_OF_LIFE_MS = 4000;

/** How long one terminal's cursor ack holds off the next.
 *
 *  The bridge streams blind: it cannot see this browser's receive queue, so
 *  without a report it keeps feeding a client that is not draining — and every
 *  frame for this tab rides ONE FIFO, so the pile it builds is what the
 *  liveness ping and every keystroke end up waiting behind. The report is the
 *  cursor we have applied, and one per quarter second per terminal is enough
 *  for a bridge whose budget is measured in megabytes, while a per-frame ack
 *  would put an RPC behind every flush of a flood. */
const TERM_ACK_THROTTLE_MS = 250;

/**
 * The socket was not there for a caller that needed it.
 *
 * `reason` is `disconnected` — the connection was lost and a reconnect is
 * already on its own backoff — or `closed`, the socket was shut for good.
 * Callers tell this apart from a bridge error about the terminal itself:
 * a lost socket is a state to wait out (attach again once it is back), not a
 * failure to report to the human as if the terminal were gone.
 */
export class TerminalSocketLost extends Error {
  constructor(reason) {
    super(`terminal socket ${reason}`);
    this.name = "TerminalSocketLost";
    this.reason = reason;
  }
}

/** Whether a rejection is "the socket was not there". */
export const isTerminalSocketLost = (error) => error instanceof TerminalSocketLost;

/** Terminal identity stops at the workspace. A selected directory is editor
 *  state and must never move or split the workspace's running PTYs. */
export function terminalScope(scope = {}) {
  return scope.workspace_id ? { workspace_id: scope.workspace_id } : { ...scope };
}

export class TerminalSocket {
  constructor({ url, transport, WebSocketImpl, getToken, getPinnedDeviceKey, preferDeviceId = () => null }) {
    this.transport = transport;
    // This session's crypto and correlation: the pending calls, the frames and
    // the demux, over whichever carrier the switch has it riding. A fresh
    // session is a fresh one of these.
    this._rpc = null;
    this._onStatus = noop;
    this._closed = false;
    // termId → { kind, attachParams, termId, cols, rows, lastCursor, ackTimer,
    //            onOutput, onSnapshot, onClosed, onLive }
    this._terms = new Map();
    this._connected = false;
    this._connectWaiters = [];
    // Agent frames that arrived before their attach response named the wire id
    // they belong to: term_id → [frame]. Only ever non-empty while an agent
    // attach is in flight (see _bufferOrphanFrame).
    this._orphanFrames = new Map();
    this._agentAttachesInFlight = 0;
    this._agentAttachSeq = 0;
    // Which wire is riding is the switch's to say, and every method below asks
    // it rather than remembering.
    this._switch = createSessionSwitch({
      session: {
        rideOn: (carrier) => this._rpc?.rideOn(carrier),
        readFrom: (carrier) => this._rpc?.readFrom(carrier),
      },
      onActive: () => this._reattachAll(),
      onIdle: () => this._reportLost(),
    });
    // The relay socket, whole: the handshake, the presence pushes, and the
    // backoff reconnect that re-presents this session while the channel carries
    // it. Nothing below this line knows a socket exists.
    this._link = createRelayLink({
      relayUrl: url,
      transport,
      WebSocketImpl,
      fetchToken: getToken,
      getPinnedDeviceKey,
      // The terminals are on one machine at a time — the route's device, else
      // home — re-read on every connect; the shells moving machine is answered
      // by the next socket.
      preferDeviceId,
      acceptTimeoutMs: HANDSHAKE_TIMEOUT_MS,
      carrying: () => this._switch.active(),
      onConnecting: () => this._reportConnecting(),
      onSession: (opened) => this._openSession(opened),
      onRelay: (carrier) => this._relayChanged(carrier),
    });
  }

  /** The device this socket's session is with. */
  get deviceId() {
    return this._link.deviceId();
  }

  /** Ride the `term` DataChannel instead of the relay socket, or `null` to fall
   *  back to it. Settles once every open terminal is attached on the new wire. */
  peer(carrier) {
    return this._switch.peer(carrier);
  }

  onStatus(fn) { this._onStatus = fn; } // 'connecting'|'connected'|'disconnected'

  async start() {
    this._closed = false;
    await this._link.start();
  }

  /**
   * Settle once the socket is connected (immediately if it already is).
   *
   * Every wait ENDS: a connection resolves it, and a lost or closed socket
   * rejects it with a TerminalSocketLost. A wait that could only ever resolve
   * left every caller of a dead socket hanging — the surface that asked never
   * finished, and its retries piled up behind a socket that was not coming back
   * without one of them noticing.
   */
  whenConnected() {
    if (this._connected) return Promise.resolve();
    if (this._closed) return Promise.reject(new TerminalSocketLost("closed"));
    return new Promise((resolve, reject) => this._connectWaiters.push({ resolve, reject }));
  }

  /** End every wait on a connection that is not coming. */
  _failConnectWaiters(reason) {
    for (const { reject } of this._connectWaiters.splice(0)) reject(new TerminalSocketLost(reason));
  }

  // ---- terminal lifecycle (all ride this one socket) --------------------

  /** term.create — mint one of the user's shells in the given scope. Only a
   *  shell: a worktree's agent is Build's, lives in its Agent tab, and is
   *  started by a delivery rather than by opening a terminal. */
  async createTerminal(scope, cols, rows) {
    await this.whenConnected();
    return this._call("term.create", { ...terminalScope(scope), cols, rows });
  }

  /** term.list — the user's open shells for a scope (never the agent). */
  async listTerminals(scope) {
    await this.whenConnected();
    const r = await this._call("term.list", terminalScope(scope));
    return r.terminals || [];
  }

  /** term.close — kill the server PTY and deregister locally. */
  async closeTerminal(termId) {
    try {
      await this.whenConnected();
      await this._call("term.close", { term_id: termId });
    } finally {
      this._forgetTerm(termId);
    }
  }

  /** Register a workspace's user terminal and attach. `scope` is retained on
   *  the registration so reconnects cannot accidentally reattach the id from
   *  another workspace. Legacy callers may omit it while old surfaces migrate. */
  async attachTerminal(termId, scope = {}, opts = undefined) {
    // Backward compatibility for the former (termId, opts) signature.
    if (opts === undefined) { opts = scope; scope = {}; }
    await this.whenConnected();
    const attachParams = terminalScope(scope);
    const entry = this._register(termId, "user", attachParams, opts || {});
    let r;
    try {
      r = await this._call("term.attach", { ...attachParams, term_id: termId, cols: entry.cols, rows: entry.rows });
    } catch (e) {
      this._deregisterFailedAttach(termId, entry);
      throw e;
    }
    this._applyAttachResult(entry, r);
    return r;
  }

  /**
   * Register a worktree's agent screen and attach — never errors on a dead
   * session. `target` is how the calling surface addresses that worktree:
   * `{ id }` for a run or plan, or the same scope shapes the shells use
   * (`{ run_id }`, `{ project_id, worktree_id }`, `{ project_id }`).
   *
   * An agent's wire id is `agent:<worktree_id>`, a hash of the canonical root
   * that no client can compute, so the registration starts under a provisional
   * key and is re-keyed to the id the bridge answers with. The caller reads
   * that id off the result: it is what term.input/term.resize address.
   */
  async attachAgent(target, opts = {}) {
    await this.whenConnected();
    const attachParams = { ...(target || {}) };
    const provisionalId = `agent:pending-${++this._agentAttachSeq}`;
    const entry = this._register(provisionalId, "agent", attachParams, opts);
    this._agentAttachesInFlight += 1;
    let r;
    try {
      r = await this._call("agent.attach", { ...attachParams, cols: entry.cols, rows: entry.rows });
    } catch (e) {
      this._deregisterFailedAttach(provisionalId, entry);
      this._agentAttachSettled();
      throw e;
    }
    this._rekeyAgent(provisionalId, entry, r.term_id);
    this._agentAttachSettled();
    this._applyAttachResult(entry, r);
    return r;
  }

  /** Move an agent registration onto the wire id the bridge just named, and
   *  hand it whatever frames arrived under that id while it was unknown. */
  _rekeyAgent(currentId, entry, wireId) {
    if (!wireId || wireId === currentId) {
      if (wireId) entry.onTermId(wireId);
      return;
    }
    if (this._terms.get(currentId) === entry) this._terms.delete(currentId);
    this._terms.set(wireId, entry);
    entry.termId = wireId; // a pending ack must name the id the bridge knows
    entry.awaitingBirth = false;
    const outran = this._orphanFrames.get(wireId);
    if (outran) {
      this._orphanFrames.delete(wireId);
      entry.preAttach.unshift(...outran);
    }
    // Keystrokes and resizes are addressed by the caller, from the id it was
    // told — so a screen that moved has to say so, or every key after the
    // agent's birth goes to an id the bridge no longer knows.
    entry.onTermId(wireId);
  }

  /**
   * The birth of an agent on a screen that was mounted before it existed.
   *
   * A worktree with no agent can only be addressed BY the worktree, so its
   * attach answers `agent:<worktree_id>` — a placeholder for the screen the
   * agent will be born onto. The newborn's frames carry `agent:<agent_id>`
   * instead, opening with the pump's start-of-session wipe: that reset is the
   * only thing that names the id, so it is what the waiting screen follows.
   *
   * Only a screen that is WAITING for a birth can claim one (its attach found
   * no agent at all: not live, no provider), and only when it is the single
   * one waiting — with two, nothing here can say which worktree the newborn
   * belongs to, and guessing would paint one panel with another's session.
   */
  _adoptAgentBirth(p) {
    if (p.type !== "term.reset" || !String(p.term_id || "").startsWith("agent:")) return null;
    const waiting = [...this._terms].filter(([, entry]) => entry.kind === "agent" && entry.awaitingBirth);
    if (waiting.length !== 1) return null;
    const [placeholderId, entry] = waiting[0];
    this._rekeyAgent(placeholderId, entry, p.term_id);
    return this._terms.get(p.term_id) === entry ? entry : null;
  }

  /** One agent attach finished (either way). With none left in flight, nothing
   *  can claim a buffered frame, so the buffer is dropped rather than grown. */
  _agentAttachSettled() {
    this._agentAttachesInFlight -= 1;
    if (this._agentAttachesInFlight <= 0) {
      this._agentAttachesInFlight = 0;
      this._orphanFrames.clear();
    }
  }

  /** Hold a frame for an agent id no registration answers to YET.
   *
   *  The bridge registers this client under its state lock and enqueues the
   *  attach response after the handler returns, so a pump flush can land in
   *  between — carrying the very wire id the response is about to reveal.
   *  Dropping those bytes loses them for good (the cursor only moves forward),
   *  so they wait here for the attach that is already in flight to claim them.
   *  Nothing is buffered outside that window. */
  _bufferOrphanFrame(p) {
    if (this._agentAttachesInFlight <= 0) return;
    if (!String(p.term_id).startsWith("agent:")) return;
    if (p.type !== "term.output" && p.type !== "term.reset") return;
    const held = this._orphanFrames.get(p.term_id) || [];
    if (held.length >= ORPHAN_FRAME_LIMIT) return;
    held.push(p);
    this._orphanFrames.set(p.term_id, held);
  }

  /** An attach that never took must not leave its registration behind — a dead
   *  id would swallow pushes and be retried on every reconnect forever. */
  _deregisterFailedAttach(termId, entry) {
    if (this._terms.get(termId) === entry) this._forgetTerm(termId);
  }

  /** Apply an attach response's snapshot, then replay any pushes that outran it
   *  on the wire (the bridge registers the sender under its state lock but
   *  enqueues the response after the handler returns, so a pump flush can slip
   *  in between — those bytes are PAST the snapshot cursor and must survive). */
  _applyAttachResult(entry, r) {
    entry.lastCursor = r.cursor || 0;
    entry.onSnapshot(b64decodeBytes(r.snapshot));
    if (entry.kind === "agent") {
      entry.live = !!r.live;
      // No session and no harness behind the screen means no agent has ever run
      // in this worktree: the id just handed back names the WORKTREE, and the
      // agent that is born here will push under its own (see _adoptAgentBirth).
      entry.awaitingBirth = !r.live && !r.provider;
      // The attach result rides along: a dead agent WITH a retained screen ran
      // and stopped, a dead agent with a blank one never ran, and only the
      // payload can tell those apart.
      entry.onLive(entry.live, r);
    }
    entry.attached = true;
    for (const p of entry.preAttach.splice(0)) this._applyStreamFrame(entry, p);
  }

  /** Deregister a terminal (tab unmounted) — the server PTY keeps running. */
  detach(termId) {
    this._forgetTerm(termId);
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

  _register(termId, kind, attachParams, opts) {
    const entry = {
      kind, attachParams,
      // The wire id this entry currently answers to — what an ack names. An
      // agent's is provisional until the bridge answers with the real one.
      termId,
      cols: opts.cols || 80,
      rows: opts.rows || 24,
      lastCursor: 0,
      ackTimer: null, // pending throttled ack, if any
      attached: false, // pushes buffer in preAttach until the attach response applies
      preAttach: [],
      live: false, // agent kind: last reported session liveness
      // agent kind: this screen is a worktree's placeholder, waiting for the
      // agent that will be born onto it to name itself.
      awaitingBirth: false,
      onOutput: opts.onOutput || noop,
      onSnapshot: opts.onSnapshot || noop,
      onClosed: opts.onClosed || noop,
      onLive: opts.onLive || noop,
      onTermId: opts.onTermId || noop,
    };
    this._terms.set(termId, entry);
    return entry;
  }

  /** Permanent close — no reconnect. */
  close() {
    this._closed = true;
    this._switch.close();
    this._rpc?.rideOn(null); // nothing is asked or answered on this session again
    for (const entry of this._terms.values()) this._cancelPendingAck(entry);
    // Nobody is waiting for a connection that will never be attempted again.
    this._failConnectWaiters("closed");
    this._link.close();
  }

  /** Drop the socket but allow auto-reconnect (device retarget / reconnect test). */
  simulateDrop() {
    this._link.dropSocket();
  }

  /** A fresh socket is on its way. A live channel is still a connection: only
   *  a session with nothing carrying it is one the panes should be shown
   *  reconnecting. */
  _reportConnecting() {
    if (this._switch.active()) return;
    this._connected = false;
    this._onStatus("connecting");
  }

  /** The link minted a session: whatever was riding the one before it is not
   *  riding this one, and this one gets its own crypto and correlation. */
  _openSession({ sessionId, sessionKeyB64, deviceId }) {
    this._switch.peer(null);
    this._rpc = createSessionRpc({
      transport: this.transport,
      sessionId,
      sessionKeyB64,
      deviceId,
      noCarrier: () => new TerminalSocketLost(this._closed ? "closed" : "disconnected"),
    });
    // A push is a live terminal frame: everything a reply is not is routed to
    // the screen it names.
    this._rpc.onPush((payload) => this._applyPush(payload));
  }

  /**
   * The relay slot changed: a carrier while a socket carries this session,
   * `null` the moment none does.
   *
   * Taking the slot re-attaches every registered terminal when the socket is
   * what carries the session; when a channel already carries it, the session
   * did not change and neither did its terminals. Losing it is a disconnect
   * only if nothing else was carrying — which is the switch's call, not this
   * method's, except for the connect that failed before anything carried at
   * all: it has no transition to make, and its waiters are owed the same
   * answer.
   */
  async _relayChanged(carrier) {
    if (!carrier) {
      // Every pending ack was read on the connection that just died: the bridge
      // it would report to is gone, and the re-attach rebases each cursor anyway.
      for (const entry of this._terms.values()) this._cancelPendingAck(entry);
      const wasCarrying = this._switch.active();
      this._switch.relay(null);
      if (!wasCarrying) this._reportLost();
      return;
    }
    await this._switch.relay(carrier);
    this._onStatus("connected");
    this._connected = true;
    this._connectWaiters.splice(0).forEach(({ resolve }) => resolve());
    this._watchLiveness();
  }

  /// Re-attach every registered terminal after a (re)connect. User terminals go
  /// through term.attach; agent screens through agent.attach (refreshing onLive).
  /// An `unknown term_id`/`unknown id` rejection means the server reaped the
  /// terminal / dropped the run → onClosed("reaped") + deregister, never an
  /// eternal per-reconnect retry. Each snapshot resets that term's cursor to the
  /// response cursor first (snapshot resync, not byte replay).
  async _reattachAll() {
    for (const [termId, entry] of [...this._terms]) {
      entry.lastCursor = 0;
      entry.attached = false;
      entry.preAttach = [];
      // The cursor this reset just discarded is what a pending ack would have
      // reported — on a connection where it means nothing.
      this._cancelPendingAck(entry);
      try {
        const r = entry.kind === "agent"
          ? await this._call("agent.attach", { ...entry.attachParams, cols: entry.cols, rows: entry.rows })
          : await this._call("term.attach", { ...entry.attachParams, term_id: termId, cols: entry.cols, rows: entry.rows });
        // A worktree that moved (or an agent that opened while we were away)
        // answers with a different wire id: follow it rather than stream into
        // an id nothing pushes to.
        if (entry.kind === "agent") this._rekeyAgent(termId, entry, r.term_id);
        this._applyAttachResult(entry, r);
      } catch (e) {
        if (/unknown (term_id|id)/.test(e.message || "")) {
          this._forgetTerm(termId);
          entry.onClosed("reaped");
        }
        // Other failures leave the entry registered — the next reconnect retries.
      }
    }
  }

  /** One live frame off whichever carrier brought it, routed to the terminal
   *  it names. */
  _applyPush(p) {
    if (!p.term_id) return;
    // An id nothing answers to is either the agent this client is waiting to
    // be born (follow it) or a screen we don't render.
    const entry = this._terms.get(p.term_id) || this._adoptAgentBirth(p);
    if (!entry) {
      // Either a term we don't render (drop it) or an agent whose wire id an
      // in-flight attach is about to reveal (hold it for that attach).
      this._bufferOrphanFrame(p);
      return;
    }
    if (p.type === "term.output" || p.type === "term.reset") {
      if (entry.attached) this._applyStreamFrame(entry, p);
      else entry.preAttach.push(p); // outran the attach response — replay after it
    } else if (p.type === "term.closed") {
      // An agent screen that merely ended its session is RETAINED (the tab keeps
      // its last screen for the next session); user terminals deregister.
      if (entry.kind === "agent" && p.reason === "agent_session_ended") {
        entry.live = false; // the next session's frames re-report live
      } else {
        this._forgetTerm(p.term_id);
      }
      entry.onClosed(p.reason);
    }
  }

  /** Apply one live push to a terminal's screen, deduping on cursor. Output
   *  applies strictly past the cursor; term.reset applies at cursor >= — the
   *  agent pump's start-of-session wipe is pushed at cursor == total (resets
   *  never advance it), which EQUALS the attach cursor the client just stored,
   *  and dropping it garbles session transitions (reset application is a full
   *  screen snapshot, so an equal-cursor replay is idempotent). */
  _applyStreamFrame(entry, p) {
    if (p.type === "term.output") {
      if ((p.cursor || 0) > entry.lastCursor) {
        entry.lastCursor = p.cursor;
        entry.onOutput(b64decodeBytes(p.data));
        this._markAgentLive(entry);
        this._scheduleAck(entry);
      }
    } else if ((p.cursor || 0) >= entry.lastCursor) {
      entry.lastCursor = p.cursor || 0;
      entry.onSnapshot(b64decodeBytes(p.data));
      this._markAgentLive(entry);
      this._scheduleAck(entry);
    }
  }

  /** Report this terminal's applied cursor to the bridge, at most once per
   *  TERM_ACK_THROTTLE_MS. The bridge pauses a client that falls too far past
   *  its last ack and resyncs it with a snapshot when it catches up, so the
   *  report is what keeps a busy terminal streaming rather than stalling
   *  everything behind it.
   *
   *  A window already open is left alone — the cursor is read when the ack is
   *  SENT, so a burst of frames costs one call and reports the last of them.
   *  The call is advisory: a failure is swallowed, because losing an ack costs
   *  one snapshot resync while surfacing it would tear the socket down. */
  _scheduleAck(entry) {
    if (entry.ackTimer) return;
    entry.ackTimer = setTimeout(() => {
      entry.ackTimer = null;
      this._call("term.ack", { term_id: entry.termId, cursor: entry.lastCursor }).catch(noop);
    }, TERM_ACK_THROTTLE_MS);
  }

  /** Drop a terminal's pending ack. A cursor means something only for the
   *  terminal and the connection it was read on: an ack that outlives either
   *  reports a position on a stream that no longer exists. */
  _cancelPendingAck(entry) {
    if (!entry || !entry.ackTimer) return;
    clearTimeout(entry.ackTimer);
    entry.ackTimer = null;
  }

  /** Deregister a terminal and drop what it still owes the bridge. */
  _forgetTerm(termId) {
    const entry = this._terms.get(termId);
    this._cancelPendingAck(entry);
    this._terms.delete(termId);
    return entry;
  }

  /** Frames only stream while a session is pumping, so an applied frame on an
   *  idle agent screen means a new session started — report it live (once) so
   *  the "no active agent session" chip clears without a re-attach. */
  _markAgentLive(entry) {
    if (entry.kind === "agent" && !entry.live) {
      entry.live = true;
      entry.onLive(true);
    }
  }

  // Application-level liveness: a relay/bridge/network outage does NOT always
  // close our socket, so we actively ping WHEN NOTHING ELSE IS ARRIVING. A
  // recently decrypted frame already proves the path, so it suppresses the
  // probe (see FRAME_PROOF_OF_LIFE_MS); only silence is probed, and a failed
  // ping means the path to the bridge is down → show disconnected and reconnect.
  async _watchLiveness() {
    const mine = (this._liveness = {}); // one watch per connection, the newest
    while (this._liveness === mine && !this._closed) {
      await new Promise((resolve) => setTimeout(resolve, 2000));
      if (this._liveness !== mine || this._closed) return;
      if (Date.now() - this._rpc.lastFrameAt() < FRAME_PROOF_OF_LIFE_MS) continue;
      const wire = this._switch.active();
      if (!wire) return;
      try {
        await this._call("ping", {}, 3000);
      } catch {
        // The wire that did not answer is the one that goes: closing a carrier
        // is how either kind reports itself gone, and the switch decides what
        // that costs — a channel falls back to the relay, a relay socket
        // reconnects.
        if (this._liveness === mine) wire.close();
        return;
      }
    }
  }

  _call(method, params = {}, timeoutMs) {
    const rpc = this._rpc;
    if (!rpc) return Promise.reject(new TerminalSocketLost(this._closed ? "closed" : "disconnected"));
    return rpc.call(method, params, { timeoutMs });
  }

  /// Nothing is carrying this session any more: say so, and end every wait that
  /// was on a wire that is gone. The next connection is already on its way; a
  /// caller that wants it says so by asking again, rather than by holding a
  /// promise nothing will settle.
  _reportLost() {
    this._connected = false;
    this._onStatus("disconnected");
    this._rpc?.fail(new TerminalSocketLost("disconnected"));
    this._failConnectWaiters("disconnected");
  }
}
