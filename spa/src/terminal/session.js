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
// The RTC terminal session mints its own encrypted RPC on the followed
// device's terminal DataChannel.

import { assertAttachmentCurrent, withAttachmentCancellation } from "./attachmentLifetime.js";
import { decodeSnapshot, projectSourceScope, TerminalPaneSession, TerminalSocketLost, terminalCloseScope, terminalScope } from "./paneSession.js";
import { createSessionRpc } from "../core/sessionRpc.js";
import { createSessionSwitch } from "../core/sessionSwitch.js";
import { recordConnectionDiagnostic } from "../core/connectionDiagnostics.js";
import { FRAME_PROOF_OF_LIFE_MS, PING_TIMEOUT_MS, peerFrameAt, whatVouchesFor } from "../core/pathLiveness.js";

export { isTerminalSocketLost, TerminalSocketLost, terminalScope } from "./paneSession.js";

const textEncoder = new TextEncoder();
const noop = () => {};

/** Why a wire this socket closed was closed, for the owner of the link that
 *  hears it go: the path carried nothing at all, which is the one judgement
 *  worth the whole connection. */
export const LIVENESS_TIMEOUT = "liveness-timeout";

/** How long the probe waits for a pong before it judges — the shared rule
 *  (core/pathLiveness.js), re-exported here because this module is where it was
 *  first written and the terminals' callers name it. */
export { PING_TIMEOUT_MS };

export class TerminalSocket extends TerminalPaneSession {
  constructor({ transport, onTermUnresponsive = noop, userTerminalRpc, userTerminalDevice = null }) {
    super({ userTerminalRpc });
    this._userTerminalDevice = userTerminalDevice || (() => this.deviceId);
    this.transport = transport;
    /** What to do about a terminal session that stopped answering on a wire
     *  that is plainly still carrying: re-establish the terminals, and nothing
     *  else. The manager owns that move, because it owns which machine the
     *  shells are on. */
    this._onTermUnresponsive = onTermUnresponsive;
    // The session this socket is on, once a rendezvous has minted one for it.
    this._session = null;
    // Whether the session under this socket is being swapped for another one.
    this._replacing = false;
    // This session's crypto and correlation: the pending calls, the frames and
    // the demux, over whichever carrier the switch has it riding. A fresh
    // session is a fresh one of these.
    this._rpc = null;
    this._stopEpoch = noop;
    this._recovery = null;
    this._stopRecovery = noop;
    this._confirmNextCarrier = false;
    // Which wire is riding is the switch's to say, and every method below asks
    // it rather than remembering.
    this._switch = createSessionSwitch({
      session: {
        rideOn: (carrier) => this._rpc?.rideOn(carrier),
        readFrom: (carrier) => this._rpc?.readFrom(carrier),
      },
      onActive: () => this._nowCarrying(),
      onIdle: () => this._reportLost(),
    });
  }

  /** The device this socket's session is with, or null before it has one. */
  get deviceId() {
    return this._session?.deviceId ?? null;
  }

  /**
   * Type at this machine from now on, over this machine's wire.
   *
   * `session` is `{ sessionId, sessionKeyB64, deviceId }`, minted through that
   * device's rendezvous by whoever moved the shells, and `carrier` is that same
   * device's `term` channel — `null` while its peer link has none yet.
   *
   * The wire comes WITH the session because a session belongs to one machine
   * and so does every channel: a socket that re-took whatever it was already
   * riding would re-attach the new session's terminals over the machine they
   * just left, whose bridge has never heard of that session — and, with nothing
   * registered to fail, would report itself connected on it. A channel already
   * handed over is passed in again, and re-taken under the new session's key,
   * which is what re-attaches every open terminal on it.
   */
  adoptTerminalSession(session, carrier = null, { confirm = false, isCurrent = () => true, recovery = null } = {}) {
    this._closed = false;
    // A session being replaced is not one being lost: the old session's calls
    // end with it, and nobody is told the shells are gone when they are moving.
    this._replacing = true;
    this._switch.peer(null);
    this._openSession(session);
    this._confirmNextCarrier = confirm;
    this._replacing = false;
    // Not `_reportLost` when there is no wire: a machine whose channel has yet
    // to open is one the panes are waiting on, which is connecting.
    this._report("connecting");
    // A status observer can synchronously move the terminals again. Do not let
    // the superseded adoption install its old carrier after that newer move.
    if (!isCurrent()) return undefined;
    return carrier ? this.peer(carrier, { recovery }) : undefined;
  }

  /**
   * Ride this device's `term` DataChannel, or `null` for "there is no wire".
   *
   * Settles once every open terminal is attached on it. There is no fallback
   * below it: with no channel the shells have nothing to type down, and every
   * caller waiting on one is told so rather than left hanging.
   */
  peer(carrier, { recovery = null } = {}) {
    const wasCarrying = Boolean(this._switch.active());
    this._bindRecovery(carrier ? recovery : null);
    const riding = this._switch.peer(carrier);
    // Losing a wire is the switch's own report. "There is no wire" told to a
    // socket that had none is not a change it can see, and the callers waiting
    // on a machine with no channel are owed the same answer.
    if (!wasCarrying && !this._switch.active()) this._reportLost();
    return riding;
  }

  _bindRecovery(recovery) {
    if (this._recovery === recovery) return;
    this._stopRecovery();
    this._stopRecovery = noop;
    this._recovery = recovery;
    this._liveness = null;
    if (!recovery) return;
    this._stopRecovery = recovery.subscribe((state) => {
      if (this._recovery !== recovery || this._closed) return;
      this._liveness = null;
      if (!state.recovering && this._switch.active() && this._connected) this._watchLiveness(true);
    });
  }

  // ---- terminal lifecycle (all ride this one socket) --------------------

  /** term.create — mint one of the user's shells in the given scope. Only a
   *  shell: a worktree's agent is Build's, lives in its Agent tab, and is
   *  started by a delivery rather than by opening a terminal. */
  async createTerminal(scope, cols, rows) {
    const deviceId = this._userTerminalDevice();
    const params = { ...terminalScope(scope), cols, rows };
    await this.whenConnected();
    return this._userCall("term.create", params, deviceId);
  }

  /** term.list — the user's open shells for a scope (never the agent). */
  async listTerminals(scope) {
    const deviceId = this._userTerminalDevice();
    await this.whenConnected();
    const result = await this._userCall("term.list", terminalScope(scope), deviceId);
    return result.terminals || [];
  }

  /** term.close — kill the server PTY and deregister locally. */
  async closeTerminal(termId, scope = this._terms.get(termId)?.attachParams || {}) {
    const deviceId = this._userTerminalDevice();
    const entry = this._terms.get(termId);
    const session = this._session;
    try {
      await this.whenConnected();
      await this._userCall("term.close", { ...terminalCloseScope(scope), term_id: termId }, deviceId);
    } finally {
      if (this._session === session && this._terms.get(termId) === entry) this._forgetTerm(termId);
    }
  }

  /** Register a workspace's user terminal and attach. `scope` is retained on
   *  the registration so reconnects cannot accidentally reattach the id from
   *  another workspace. Legacy callers may omit it while old surfaces migrate. */
  async attachTerminal(termId, scope = {}, opts = undefined) {
    const deviceId = this._userTerminalDevice();
    // Backward compatibility for the former (termId, opts) signature.
    if (opts === undefined) { opts = scope; scope = {}; }
    opts ||= {};
    assertAttachmentCurrent(opts);
    await this.whenConnected(opts.signal);
    assertAttachmentCurrent(opts);
    const attachParams = terminalScope(scope);
    this._assertUserTerminalDevice(attachParams, deviceId);
    const entry = this._register(termId, "user", attachParams, opts);
    const current = () => this._ownsAttachment(entry);
    try {
      opts.onRegistered?.(this._paneDisposer(entry));
      const request = this._userCall("term.attach", { ...attachParams, term_id: termId, cols: entry.cols, rows: entry.rows }, deviceId, current);
      const result = await withAttachmentCancellation(request, opts.signal);
      assertAttachmentCurrent({ isCurrent: current });
      this._applyAttachResult(entry, result);
      return result;
    } catch (error) {
      this._deregisterFailedAttach(termId, entry);
      throw error;
    }
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
    const provisionalId = `agent:pending-${++this._agentAttachSeq}`;
    const attachParams = { ...(target || {}) };
    await this.whenConnected(opts.signal);
    assertAttachmentCurrent(opts);
    const entry = this._register(provisionalId, "agent", attachParams, opts);
    this._agentAttachesInFlight += 1;
    let r;
    try {
      opts.onRegistered?.(this._paneDisposer(entry));
      r = await withAttachmentCancellation(this._call("agent.attach", { ...attachParams, cols: entry.cols, rows: entry.rows }), opts.signal);
      assertAttachmentCurrent({ isCurrent: () => this._ownsAttachment(entry) });
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

  /** Apply an attach response's snapshot, then replay any pushes that outran it
   *  on the wire (the bridge registers the sender under its state lock but
   *  enqueues the response after the handler returns, so a pump flush can slip
   *  in between — those bytes are PAST the snapshot cursor and must survive). */
  _applyAttachResult(entry, r) {
    entry.lastCursor = r.cursor || 0;
    entry.onSnapshot(decodeSnapshot(r.snapshot));
    this._applyAgentAttachState(entry, r);
    entry.attached = true;
    for (const p of entry.preAttach.splice(0)) this._applyStreamFrame(entry, p);
  }

  /** Send keystrokes to a terminal's PTY. */
  async input(termId, data) {
    const bytes = textEncoder.encode(data);
    await this._call("term.input", { term_id: termId, data: btoa(String.fromCharCode(...bytes)) });
  }

  async resize(termId, cols, rows) {
    const entry = this._terms.get(termId);
    if (entry) { entry.cols = cols; entry.rows = rows; }
    const params = { term_id: termId, cols, rows };
    await this._call("term.resize", params);
  }

  // An HTTP Opening can take over the current device's existing panes.
  takePanesForDevice(deviceId) {
    this._failConnectWaiters("disconnected");
    const sameDevice = [];
    for (const termId of this._terms.keys()) {
      const entry = this._forgetTerm(termId);
      if (entry.deviceId === deviceId) sameDevice.push(entry);
    }
    return sameDevice;
  }

  /** Permanent close — no reconnect. */
  close() {
    this._closed = true;
    this._stopEpoch();
    this._bindRecovery(null);
    this._switch.close();
    this._rpc?.close(new TerminalSocketLost("closed"));
    for (const entry of this._terms.values()) this._cancelPendingAck(entry);
    // Nobody is waiting for a connection that will never be attempted again.
    this._failConnectWaiters("closed");
  }

  /** This socket's own crypto and correlation for the session it has just been
   *  given. A session is one key: a fresh one is a fresh rpc. */
  _detachForeignProjectTerminals(deviceId) {
    for (const [termId, entry] of this._terms) {
      if (projectSourceScope(entry.attachParams) && entry.deviceId !== deviceId) this._forgetTerm(termId);
    }
  }

  _openSession({ sessionId, sessionKeyB64, deviceId, bridgeEpoch }) {
    this._stopEpoch();
    this._rpc?.close(new TerminalSocketLost("disconnected"));
    this._detachForeignProjectTerminals(deviceId);
    this._session = { sessionId, sessionKeyB64, deviceId };
    this._rpc = createSessionRpc({
      transport: this.transport,
      sessionId,
      sessionKeyB64,
      deviceId,
      bridgeEpoch,
      noCarrier: () => new TerminalSocketLost(this._closed ? "closed" : "disconnected"),
    });
    // A push is a live terminal frame: everything a reply is not is routed to
    // the screen it names.
    this._rpc.onPush((payload) => this._applyPush(payload));
    const rpc = this._rpc;
    this._stopEpoch = rpc.bridgeEpoch.onChange((_current, previous) => {
      if (previous && !this._closed && this._rpc === rpc) this._restartEpoch();
    });
  }

  /** A new process has no old attachments or cursor acknowledgements. Retire
   * the opening before asking its owner to mint another one on this carrier. */
  _restartEpoch() {
    const session = this._session;
    this._stopEpoch();
    this._rpc?.close(new TerminalSocketLost("disconnected"));
    this._rpc = null;
    this._connected = false;
    this._bindRecovery(null);
    this._orphanFrames.clear();
    for (const entry of this._terms.values()) {
      this._cancelPendingAck(entry);
      entry.lastCursor = 0;
      entry.attached = false;
      entry.preAttach = [];
    }
    this._report("connecting");
    if (!this._closed && this._session === session) this._onTermUnresponsive();
  }

  /**
   * A wire has started carrying this session.
   *
   * Every registered terminal re-attaches on it before anyone is told the
   * socket is up: a caller that was waiting is owed a connection its own
   * terminal is already attached over, not one it has to race.
   */
  async _nowCarrying() {
    // A wire handed over before this socket has a session carries nothing: the
    // channel of a device whose mint is still in flight. Adopting that session
    // re-takes the wire, and that is the connection.
    if (!this._rpc) return;
    const rpc = this._rpc;
    const wire = this._switch.active();
    const diagnosticId = `${this.deviceId}:${this._session.sessionId}`;
    if (this._confirmNextCarrier) {
      this._confirmNextCarrier = false;
      try {
        await this._call("ping", {}, 3000);
        recordConnectionDiagnostic(diagnosticId, "terminal-session", { state: "confirmed" });
      } catch (error) {
        recordConnectionDiagnostic(diagnosticId, "terminal-session", { state: "confirmation-failed" });
        throw error;
      }
    }
    if (this._rpc !== rpc || this._switch.active() !== wire) return;
    await this._reattachAll(() => this._rpc === rpc && this._switch.active() === wire);
    if (this._rpc !== rpc || this._switch.active() !== wire) return;
    this._connected = true;
    this._report("connected");
    this._connectWaiters.splice(0).forEach(({ resolve }) => resolve());
    this._watchLiveness();
  }

  /// Re-attach every registered terminal after a (re)connect. User terminals go
  /// through term.attach; agent screens through agent.attach (refreshing onLive).
  /// An `unknown term_id`/`unknown id` rejection means the server reaped the
  /// terminal / dropped the run → onClosed("reaped") + deregister, never an
  /// eternal per-reconnect retry. Each snapshot resets that term's cursor to the
  /// response cursor first (snapshot resync, not byte replay).
  async _reattachAll(isCurrent = () => true) {
    for (const [termId, entry] of [...this._terms]) {
      if (!isCurrent()) return;
      await this._reattachEntry(termId, entry, isCurrent);
    }
  }

  async _reattachEntry(termId, entry, isCurrent) {
    if (!this._ownsAttachment(entry)) { this._deregisterFailedAttach(termId, entry); return; }
    entry.lastCursor = 0;
    entry.attached = false;
    entry.preAttach = [];
    // The cursor this reset discarded cannot be acknowledged on a new wire.
    this._cancelPendingAck(entry);
    try {
      const request = entry.kind === "agent"
        ? this._call("agent.attach", { ...entry.attachParams, cols: entry.cols, rows: entry.rows })
        : this._userCall("term.attach", { ...entry.attachParams, term_id: termId, cols: entry.cols, rows: entry.rows }, entry.deviceId, () => this._ownsAttachment(entry));
      const result = await withAttachmentCancellation(request, entry.signal);
      if (!isCurrent()) return;
      if (!this._ownsAttachment(entry)) { this._deregisterFailedAttach(termId, entry); return; }
      // A moved worktree or an agent born while away can answer with a new id.
      if (entry.kind === "agent") this._rekeyAgent(termId, entry, result.term_id);
      this._applyAttachResult(entry, result);
    } catch (error) {
      if (!isCurrent()) return;
      this._handleAttachFailure(error, termId, entry);
      // Other failures leave the entry registered for the next reconnect.
    }
  }

  _handleAttachFailure(error, termId, entry) {
    if (!this._ownsAttachment(entry)) { this._deregisterFailedAttach(termId, entry); return; }
    if (!/unknown (term_id|id)/.test(error.message || "")) return;
    this._deregisterFailedAttach(termId, entry);
    entry.onClosed("reaped");
  }

  _receiveStreamFrame(entry, frame) {
    if (entry.attached) this._applyStreamFrame(entry, frame);
    else entry.preAttach.push(frame);
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
        entry.onOutput(decodeSnapshot(p.data));
        this._markAgentLive(entry);
        this._scheduleAck(entry);
      }
    } else if ((p.cursor || 0) >= entry.lastCursor) {
      entry.lastCursor = p.cursor || 0;
      entry.onSnapshot(decodeSnapshot(p.data));
      this._markAgentLive(entry);
      this._scheduleAck(entry);
    }
  }

  _scheduleAck(entry) {
    super._scheduleAck(entry, () => this._call("term.ack", {
      term_id: entry.termId, cursor: entry.lastCursor,
    }).catch(noop));
  }

  // Application-level liveness: a bridge or network outage does NOT always close
  // the channel, so we actively ping WHEN NOTHING ELSE IS ARRIVING. A recently
  // decrypted frame already proves the path, so it suppresses the probe (see
  // FRAME_PROOF_OF_LIFE_MS); only silence is probed, and a failed ping means the
  // path to the bridge is down → the wire goes and the shells are lost.
  async _watchLiveness(immediate = false) {
    const mine = (this._liveness = {}); // one watch per connection, the newest
    const diagnosticId = `${this.deviceId}:${this._session?.sessionId || "terminal"}`;
    while (this._liveness === mine && !this._closed) {
      await this._livenessDelay(immediate);
      immediate = false;
      if (!await this._probeLiveness(mine, diagnosticId)) return;
    }
  }

  _livenessDelay(immediate) {
    return immediate ? Promise.resolve() : new Promise((resolve) => setTimeout(resolve, 2000));
  }

  /** When this peer last carried anything — the terminal session's own frames
   *  and the app session's alike, because the two channels are one path and a
   *  frame on either is proof it is up. */
  _peerFrameAt(wire) {
    return peerFrameAt(this._rpc, wire);
  }

  async _probeLiveness(liveness, diagnosticId) {
    if (this._liveness !== liveness || this._closed || this._recovery?.snapshot().recovering) return false;
    const wire = this._switch.active();
    if (!wire) return false;
    if (Date.now() - this._peerFrameAt(wire) < FRAME_PROOF_OF_LIFE_MS) return true;
    const asked = Date.now();
    try {
      await this._call("ping", {}, PING_TIMEOUT_MS);
      return this._liveness === liveness;
    } catch (error) {
      return this._judgeSilence(liveness, wire, diagnosticId, { asked, error });
    }
  }

  /**
   * The ping did not answer. Which of the two things that means?
   *
   * A path nothing vouches for is down, and the wire goes: closing a carrier
   * is how it reports itself gone, and its owner decides what that costs.
   *
   * Two things vouch for it. A frame that arrived on either channel while the
   * ping was out is one. The other, and the stronger, is the browser's own
   * ICE: it runs consent checks over the path every few seconds and takes the
   * connection off `connected` when they stop being answered, which is a
   * direct measurement of the path where an application ping measures the path
   * AND the daemon behind it. A bridge with eleven agents on it can take three
   * seconds to say "pong" over a phone's relayed path while the path itself is
   * perfectly healthy — and a browser that read that as death tore its own
   * connection down every six seconds, all day, which is the bug this is.
   *
   * So a path ICE is still holding costs the terminals their session and
   * nothing more: they re-establish on the wire that is still there, and the
   * app session and the peer are left alone.
   */
  _judgeSilence(liveness, wire, diagnosticId, { asked, error } = {}) {
    if (this._liveness !== liveness) return false;
    const vouched = this._whatVouchesFor(wire);
    recordConnectionDiagnostic(diagnosticId, "terminal-session", {
      state: "liveness-timeout",
      channel: vouched === "nothing" ? "peer" : "term",
      // Which evidence saved the path, or that there was none.
      vouched,
      // How the ping ended, and how long it took to end that way: a refusal
      // that comes back in a millisecond is a wire that is not there, not a
      // bridge that took too long, and the two were indistinguishable in the
      // record a phone sent back.
      waitedMs: asked ? Date.now() - asked : null,
      refusal: error?.message || null,
    });
    if (vouched === "nothing") {
      wire.close(LIVENESS_TIMEOUT);
      return false;
    }
    this._onTermUnresponsive?.();
    return false;
  }

  /** What says this path is still there, in the order the evidence is worth
   *  anything: a frame that arrived on either channel, then ICE's own word.
   *  The terminals' verdict on that evidence is below; the reading of it is the
   *  shared rule (core/pathLiveness.js). */
  _whatVouchesFor(wire) {
    return whatVouchesFor(this._rpc, wire);
  }

  _assertUserTerminalDevice(scope, deviceId) {
    if (projectSourceScope(scope) && deviceId && this.deviceId !== deviceId) {
      throw new Error("The terminal device changed before this request could be sent.");
    }
  }

  _userCall(method, params, deviceId = this.deviceId, isCurrent = () => true) {
    assertAttachmentCurrent({ isCurrent });
    this._assertUserTerminalDevice(params, deviceId);
    const session = this._session;
    return this._userTerminalRpc(this.deviceId, method, params, (verb, scoped) => {
      if (this._session !== session) throw new TerminalSocketLost("disconnected");
      assertAttachmentCurrent({ isCurrent });
      return this._call(verb, scoped, undefined, () => assertAttachmentCurrent({ isCurrent }));
    });
  }

  _call(method, params = {}, timeoutMs, assertCurrent) {
    const rpc = this._rpc;
    if (!rpc) return Promise.reject(new TerminalSocketLost(this._closed ? "closed" : "disconnected"));
    return rpc.call(method, params, { timeoutMs, assertCurrent });
  }

  /// Nothing is carrying this session any more: say so, and end every wait that
  /// was on a wire that is gone. The next connection is already on its way; a
  /// caller that wants it says so by asking again, rather than by holding a
  /// promise nothing will settle.
  _reportLost() {
    this._bindRecovery(null);
    this._liveness = null;
    // Every pending ack was read on the wire that just went: the bridge it would
    // report to cannot be reached, and the next attach rebases each cursor.
    for (const entry of this._terms.values()) this._cancelPendingAck(entry);
    this._connected = false;
    this._rpc?.fail(new TerminalSocketLost("disconnected"));
    if (this._replacing) return;
    this._failConnectWaiters("disconnected");
    this._report("disconnected");
  }
}
