// Pane identity and callbacks are independent of the wire.
import { withAttachmentCancellation } from "./attachmentLifetime.js";

const noop = () => {};
export const ORPHAN_FRAME_LIMIT = 64;
const TERM_ACK_THROTTLE_MS = 250;

export const decodeSnapshot = (value) => Uint8Array.from(atob(value || ""), (byte) => byte.charCodeAt(0));

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

export function projectSourceScope(scope) {
  return Boolean(scope.project_id && (scope.source_id || scope.source_path));
}

export function terminalCloseScope(scope) {
  return projectSourceScope(scope) ? terminalScope(scope) : {};
}

function paneCallbacks(opts) {
  return {
    onOutput: opts.onOutput || noop,
    onSnapshot: opts.onSnapshot || noop,
    onClosed: opts.onClosed || noop,
    onLive: opts.onLive || noop,
    onTermId: opts.onTermId || noop,
    onFailure: opts.onFailure || noop,
  };
}

export class TerminalPaneSession {
  constructor({ userTerminalRpc = (_deviceId, method, params, call) => call(method, params) } = {}) {
    this._userTerminalRpc = userTerminalRpc;
    this._terms = new Map();
    this._orphanFrames = new Map();
    this._agentAttachesInFlight = 0;
    this._agentAttachSeq = 0;
    this._connected = false;
    this._closed = false;
    this._connectWaiters = [];
    this._status = null;
    this._onStatus = noop;
  }

  onStatus(fn) { this._onStatus = fn; } // 'connecting'|'connected'|'disconnected'

  /**
   * Settle once the socket is connected (immediately if it already is).
   *
   * Every wait ENDS: a connection resolves it, and a lost or closed socket
   * rejects it with a TerminalSocketLost. A wait that could only ever resolve
   * left every caller of a dead socket hanging — the surface that asked never
   * finished, and its retries piled up behind a socket that was not coming back
   * without one of them noticing.
   */
  whenConnected(signal) {
    if (this._connected) return Promise.resolve();
    if (this._closed) return Promise.reject(new TerminalSocketLost("closed"));
    const waiter = {};
    const connected = new Promise((resolve, reject) => {
      Object.assign(waiter, { resolve, reject });
      this._connectWaiters.push(waiter);
    });
    return withAttachmentCancellation(connected, signal).finally(() => {
      this._connectWaiters = this._connectWaiters.filter((held) => held !== waiter);
    });
  }

  /** End every wait on a connection that is not coming. */
  _failConnectWaiters(reason) {
    for (const { reject } of this._connectWaiters.splice(0)) reject(new TerminalSocketLost(reason));
  }

  /** Move an agent registration onto the wire id the bridge just named, and
   *  hand it whatever frames arrived under that id while it was unknown. */
  _rekeyAgent(currentId, entry, wireId) {
    if (!wireId || wireId === currentId) {
      if (wireId) entry.onTermId(wireId);
      return;
    }
    if (this._terms.has(wireId)) this._displaceAgentPane(wireId);
    if (this._terms.get(currentId) === entry) this._terms.delete(currentId);
    this._terms.set(wireId, entry);
    entry.termId = wireId; // a pending ack must name the id the bridge knows
    entry.awaitingBirth = false;
    const outran = this._orphanFrames.get(wireId);
    if (outran) {
      this._orphanFrames.delete(wireId);
      this._restoreOrphanFrames(entry, outran);
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
    this._stashOrphanFrame(p, held);
  }

  _stashOrphanFrame(p, held) {
    if (held.length >= ORPHAN_FRAME_LIMIT) return;
    held.push(p);
    this._orphanFrames.set(p.term_id, held);
  }

  /** An attach that never took must not leave its registration behind — a dead
   *  id would swallow pushes and be retried on every reconnect forever. */
  _ownsAttachment(entry) {
    return this._terms.get(entry.termId) === entry && !entry.signal?.aborted && entry.isCurrent();
  }

  _deregisterFailedAttach(termId, entry) {
    if (this._terms.get(termId) === entry) this._forgetTerm(termId);
  }

  _displaceAgentPane() {
    // The RTC transport replaces a same-id registration locally.
  }

  _restoreOrphanFrames(entry, frames) { entry.preAttach.unshift(...frames); }

  _applyAgentAttachState(entry, r) {
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
  }

  /** Deregister a terminal (tab unmounted) — the server PTY keeps running. */
  detach(termId, signal) {
    if (signal && this._terms.get(termId)?.signal !== signal) return;
    this._forgetTerm(termId);
  }

  _register(termId, kind, attachParams, opts) {
    const previous = this._terms.get(termId);
    if (previous) this._releasePane(previous);
    const entry = {
      registry: this, kind, attachParams, deviceId: this.deviceId,
      afterRender: opts.afterRender === true,
      signal: opts.signal, isCurrent: opts.isCurrent || (() => true),
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
      ...paneCallbacks(opts),
    };
    this._terms.set(termId, entry);
    return entry;
  }

  _paneDisposer(entry) {
    return () => entry.registry._deregisterFailedAttach(entry.termId, entry);
  }

  /** Say what the panes show, once per change: a status they are already
   *  showing is not news, and a wire changing under a live channel is not a
   *  disconnection. */
  _report(status) {
    if (this._status === status) return;
    this._status = status;
    this._onStatus(status);
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
      this._receiveStreamFrame(entry, p);
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

  /** Report this terminal's applied cursor to the bridge, at most once per
   *  TERM_ACK_THROTTLE_MS. The bridge pauses a client that falls too far past
   *  its last ack and resyncs it with a snapshot when it catches up, so the
   *  report is what keeps a busy terminal streaming rather than stalling
   *  everything behind it.
   *
   *  A window already open is left alone — the cursor is read when the ack is
   *  SENT, so a burst of frames costs one call and reports the last of them.
   *  Each transport settles the ACK according to its own wire rules. */
  _scheduleAck(entry, send) {
    if (entry.ackTimer) return;
    entry.ackTimer = setTimeout(() => {
      entry.ackTimer = null;
      send();
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
    this._terms.delete(termId);
    if (entry) this._releasePane(entry);
    return entry;
  }

  _releasePane(entry) { this._cancelPendingAck(entry); }

  /** Frames only stream while a session is pumping, so an applied frame on an
   *  idle agent screen means a new session started — report it live (once) so
   *  the "no active agent session" chip clears without a re-attach. */
  _markAgentLive(entry) {
    if (entry.kind === "agent" && !entry.live) {
      entry.live = true;
      entry.onLive(true);
    }
  }
}
