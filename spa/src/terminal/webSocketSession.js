import { assertAttachmentCurrent, withAttachmentCancellation } from "./attachmentLifetime.js";
import { decodeSnapshot, ORPHAN_FRAME_LIMIT, TerminalPaneSession, TerminalSocketLost, terminalCloseScope, terminalScope } from "./paneSession.js";
import { createWebSocketBinding, terminalRetryDelay } from "./webSocketBinding.js";
import { createWebSocketFrameQueue } from "./webSocketRender.js";
import { httpFailure } from "../core/httpEndpoint.js";

const textEncoder = new TextEncoder();
const noop = () => {};
const SNAPSHOT_TOO_LARGE = "This screen is too large to display";
const isNewScreenFrame = (frame, cursor) => frame.cursor > cursor || (frame.reset && frame.cursor === cursor);
const reconnecting = () => Object.assign(new Error("reconnecting to your machine…"), { retryable: true });
const paneResult = (entry) => ({ term_id: entry.termId, live: entry.live });

function encodeInput(bytes) {
  const chunks = [];
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    chunks.push(String.fromCharCode(...bytes.subarray(offset, offset + 8192)));
  }
  return btoa(chunks.join(""));
}

/** The selected Opening's pane session, independent of the application wire. */
export class WebSocketTerminalSession extends TerminalPaneSession {
  constructor({ userTerminalDevice, userTerminalRpc, rosterCall, onUncertainRoster, panes }) {
    super({ userTerminalRpc });
    this._userTerminalDevice = userTerminalDevice;
    this._rosterCall = rosterCall;
    this._onUncertainRoster = onUncertainRoster;
    this._deviceId = null;
    this._binding = null;
    for (const entry of panes) {
      entry.registry = this;
      this._terms.set(entry.termId, entry);
      this._watchPane(entry, this._paneDisposer(entry));
    }
  }

  get deviceId() { return this._deviceId; }

  // Follow the selected Opening while retaining panes on the same device.
  follow(deviceId, owner) {
    const previous = this._binding;
    if (this._deviceId === deviceId && (previous?.owner || null) === owner) return;
    previous?.close();
    this._binding = null;
    this._preparePanes(deviceId);
    this._deviceId = deviceId;
    if (!owner) return;
    const binding = createWebSocketBinding({
      owner,
      hasPanes: () => this._activePanes().length > 0,
      onReady: (carrier) => this._restorePanes(binding, carrier),
      onLoss: (error) => this._carrierLost(binding, error),
      onPush: (frame) => this._receivePush(frame),
    });
    this._binding = binding;
    if (this._activePanes().length) void binding.ensure().catch(noop);
  }

  _preparePanes(deviceId) {
    this._orphanFrames.clear();
    this._connected = false;
    for (const [termId, entry] of this._terms) {
      if (entry.deviceId !== deviceId) this._forgetTerm(termId);
      else this._suspendPane(entry, reconnecting());
    }
    this._report("disconnected");
  }

  _activePanes() {
    return [...this._terms.values()].filter((entry) => this._ownsAttachment(entry) && !entry.permanentFailure);
  }

  async _restorePanes(binding, carrier) {
    this._connected = true;
    this._report("connected");
    if (this._binding !== binding) return;
    this._connectWaiters.splice(0).forEach(({ resolve }) => resolve());
    for (const entry of this._activePanes()) {
      if (this._binding !== binding || binding.carrier() !== carrier) return;
      if (!entry.needsRestore || !this._ownsAttachment(entry)) continue;
      await this._attachPane(entry, binding, carrier).catch(noop);
    }
  }

  _carrierLost(binding, error) {
    this._orphanFrames.clear();
    this._connected = false;
    this._report("disconnected");
    const failure = error.error_code === "scope_ended" ? reconnecting() : error;
    for (const entry of this._activePanes()) {
      if (this._binding !== binding) return;
      this._suspendPane(entry, failure);
    }
  }

  _suspendPane(entry, error) {
    this._releaseAttachment(entry);
    if (entry.permanentFailure) return;
    entry.needsRestore = true;
    this._showFailure(entry, error);
  }

  _showFailure(entry, error) {
    entry.failure = error;
    const connecting = error.retryable && error.httpStatus !== 429;
    const message = error.error_code === "snapshot_too_large" ? SNAPSHOT_TOO_LARGE : error.message;
    const retry = connecting ? null : () => this._retryPane(entry, true);
    entry.onFailure({ message, retry });
  }

  async _mountPane(termId, kind, params, opts) {
    assertAttachmentCurrent(opts);
    const entry = this._register(termId, kind, params, opts);
    const dispose = this._paneDisposer(entry);
    const binding = this._binding;
    try {
      opts.onRegistered?.(dispose, {
        input: (data) => this._inputPane(entry, data),
        resize: (cols, rows) => this._resizePane(entry, cols, rows),
      });
      this._watchPane(entry, dispose);
      this._assertPane(entry, binding);
      if (!binding) {
        entry.needsRestore = true;
        throw reconnecting();
      }
      const carrier = await withAttachmentCancellation(binding.ensure(), entry.signal);
      this._assertPane(entry, binding);
      return await withAttachmentCancellation(this._attachPane(entry, binding, carrier), entry.signal);
    } catch (error) {
      if (!this._ownsAttachment(entry)) throw error;
      if (!entry.failure && this._binding === binding) this._showFailure(entry, error);
      return paneResult(entry);
    }
  }

  _assertPane(entry, binding) {
    assertAttachmentCurrent({ isCurrent: () => this._ownsAttachment(entry) && this._binding === binding });
  }

  _attachPane(entry, binding, carrier) {
    if (entry.ws?.carrier === carrier) return entry.ws.ready || Promise.resolve(paneResult(entry));
    entry.needsRestore = false;
    const attempt = { carrier, queue: createWebSocketFrameQueue(), abort: new AbortController() };
    entry.ws = attempt;
    entry.attached = false;
    attempt.ready = this._requestAttach(entry, binding, attempt).finally(() => { attempt.ready = null; });
    return attempt.ready;
  }

  async _requestAttach(entry, binding, attempt) {
    const isAgent = entry.kind === "agent";
    if (isAgent) this._agentAttachesInFlight++;
    try {
      const result = await this._sendAttach(entry, binding, attempt);
      this._assertPane(entry, binding);
      assertAttachmentCurrent({ isCurrent: () => entry.ws === attempt });
      if (isAgent) this._rekeyAgent(entry.termId, entry, result.term_id);
      if (entry.ws === attempt) this._applyAttach(entry, attempt, result);
      return result;
    } catch (error) {
      if (entry.ws === attempt) this._refusePane(entry, error);
      throw error;
    } finally {
      if (isAgent) this._agentAttachSettled();
    }
  }

  _sendAttach(entry, binding, attempt) {
    const params = { ...entry.attachParams, cols: entry.cols, rows: entry.rows };
    const call = (method, params) => binding.call(attempt.carrier, method, params, {
      signal: attempt.abort.signal,
      assertCurrent: () => {
        this._assertPane(entry, binding);
        assertAttachmentCurrent({ isCurrent: () => entry.ws === attempt });
      },
      onAttachId: (id) => { attempt.id = id; },
    });
    if (entry.kind === "agent") return call("agent.attach", params);
    return this._userTerminalRpc(entry.deviceId, "term.attach", { ...params, term_id: entry.termId }, call);
  }

  _applyAttach(entry, attempt, result) {
    entry.failure = null;
    entry.reattachFailures = 0;
    entry.attached = true;
    attempt.initial = { reset: true, data: result.snapshot, cursor: result.cursor };
    entry.onFailure(null);
    if (entry.ws !== attempt) return;
    this._applyAgentAttachState(entry, result);
    this._paintPane(entry);
  }

  _refusePane(entry, error) {
    this._releaseAttachment(entry);
    if (/unknown (term_id|id)/.test(error.message)) {
      this._deregisterFailedAttach(entry.termId, entry);
      entry.onClosed("reaped");
      return;
    }
    entry.permanentFailure = !error.retryable;
    this._showFailure(entry, error);
    if (error.retryable) this._scheduleReattach(entry);
    this._binding?.idle();
  }

  _scheduleReattach(entry) {
    const failures = entry.reattachFailures ?? 0;
    entry.reattachFailures = failures + 1;
    entry.reattachTimer = setTimeout(() => this._retryPane(entry), terminalRetryDelay(failures));
  }

  _releaseAttachment(entry) {
    const attempt = entry.ws;
    entry.ws = null;
    entry.attached = false;
    entry.awaitingBirth = false;
    clearTimeout(entry.reattachTimer);
    this._cancelPendingAck(entry);
    if (!attempt) return;
    attempt.initial = null;
    attempt.queue.clear();
    attempt.abort.abort();
    if (attempt.id) attempt.carrier.detach(attempt.id);
  }

  async _retryPane(entry, manual = false) {
    if (!this._ownsAttachment(entry)) return;
    const binding = this._binding;
    if (!binding) return;
    entry.permanentFailure = false;
    if (manual) entry.reattachFailures = 0;
    try {
      const carrier = await binding.ensure({ manual });
      this._assertPane(entry, binding);
      await this._attachPane(entry, binding, carrier);
    } catch { /* The entry retains its refusal or will attach on the next socket. */ }
  }

  _receivePush(frame) {
    if (frame.type !== "attachment_error") return this._applyPush(frame);
    const entry = [...this._terms.values()].find((entry) => entry.ws?.id === frame.attach_id);
    if (!entry) return;
    this._refusePane(entry, httpFailure(frame.reason));
  }

  _stashOrphanFrame(frame, held) {
    // Unknown agent ids share a bounded staging area until in-flight replies
    // identify them. A dropped frame leaves a marker for snapshot recovery.
    if (held.overflow) return;
    const frames = [...this._orphanFrames.values()].flat();
    const resets = frames.some((p) => p.type === "term.reset");
    const ordinary = frames.reduce((total, p) => total + (p.type === "term.reset" ? 0 : p.data.length), 0);
    if (this._orphanFrames.size >= ORPHAN_FRAME_LIMIT && !held.length) return;
    if (frames.length >= ORPHAN_FRAME_LIMIT || (frame.type === "term.reset" ? resets : ordinary + frame.data.length > 1024 * 1024)) {
      held.overflow = true;
    } else held.push({ type: frame.type, cursor: frame.cursor, data: frame.data });
    this._orphanFrames.set(frame.term_id, held);
  }

  _receiveStreamFrame(entry, frame) {
    const attempt = entry.ws;
    if (!attempt || !this._ownsAttachment(entry)) return;
    if (!attempt.queue.push({ reset: frame.type === "term.reset", data: frame.data, cursor: frame.cursor })) {
      this._resnapshotPane(entry);
      return;
    }
    this._paintPane(entry);
  }

  _resnapshotPane(entry) {
    this._releaseAttachment(entry);
    entry.needsSnapshot = true;
    if (!entry.rendering) this._resumePainting(entry);
  }

  _resumePainting(entry) {
    if (!entry.needsSnapshot) return this._paintPane(entry);
    entry.needsSnapshot = false;
    void this._retryPane(entry);
  }

  _paintPane(entry) {
    if (entry.rendering || !entry.attached || !this._ownsAttachment(entry)) return;
    const attempt = entry.ws;
    if (!attempt) return;
    const initial = attempt.initial;
    const frame = initial || attempt.queue.shift();
    attempt.initial = null;
    if (!frame) return;
    const previous = attempt.cursor ?? -1;
    if (!isNewScreenFrame(frame, previous)) return this._paintPane(entry);
    this._renderFrame(entry, attempt, frame, Boolean(initial), previous);
  }

  _renderFrame(entry, attempt, frame, initial, previous) {
    let bytes = decodeSnapshot(frame.data);
    if (!frame.reset) {
      const length = frame.cursor - previous;
      if (length > bytes.length) return this._resnapshotPane(entry);
      bytes = bytes.subarray(bytes.length - length);
    }
    entry.rendering = true;
    const cursor = frame.cursor;
    const done = () => this._framePainted(entry, attempt, cursor, initial);
    const paint = frame.reset ? entry.onSnapshot : entry.onOutput;
    if (entry.afterRender) paint(bytes, done);
    else { paint(bytes); done(); }
  }

  _framePainted(entry, attempt, cursor, initial) {
    entry.rendering = false;
    if (entry.ws === attempt && this._ownsAttachment(entry)) {
      attempt.cursor = cursor;
      if (initial) this._ackCursor(entry, attempt);
      else {
        this._markAgentLive(entry);
        this._scheduleAck(entry, () => this._ackCursor(entry, attempt));
      }
    }
    this._resumePainting(entry);
  }

  _ackCursor(entry, attempt) {
    if (entry.ws !== attempt || !this._ownsAttachment(entry)) return;
    const params = { term_id: entry.termId, cursor: attempt.cursor };
    void this._binding?.call(attempt.carrier, "term.ack", params).catch((error) => {
      if (error.error_code === "busy" && entry.ws === attempt) {
        this._scheduleAck(entry, () => this._ackCursor(entry, attempt));
      }
    });
  }

  _inputPane(entry, data) {
    const params = { term_id: entry.termId, data: encodeInput(textEncoder.encode(data)) };
    return this._sendEffect("term.input", params, entry);
  }

  _resizePane(entry, cols, rows) {
    Object.assign(entry, { cols, rows });
    return this._sendEffect("term.resize", { term_id: entry.termId, cols, rows }, entry);
  }

  _sendEffect(method, params, entry = this._terms.get(params.term_id)) {
    if (!entry?.attached || !entry.ws || !this._ownsAttachment(entry)) return Promise.reject(new TerminalSocketLost("disconnected"));
    const attempt = entry.ws;
    return this._binding.call(attempt.carrier, method, params, {
      signal: attempt.abort.signal,
      assertCurrent: () => assertAttachmentCurrent({ isCurrent: () => this._ownsAttachment(entry) && entry.ws === attempt }),
    });
  }

  async _mutateShell(method, params, rosterScope, rosterEntityId) {
    const binding = this._binding;
    if (!binding) throw new TerminalSocketLost("disconnected");
    const deviceId = this.deviceId;
    try {
      return await binding.use((carrier) =>
        this._userTerminalRpc(deviceId, method, params, (verb, scoped) => binding.call(carrier, verb, scoped)));
    } catch (error) {
      if (error.uncertain) {
        error.terminalWebSocketEffect = true;
        void Promise.resolve(this._onUncertainRoster(deviceId, rosterScope, rosterEntityId)).catch(noop);
      }
      throw error;
    }
  }

  async closeTerminal(termId, scope = this._terms.get(termId)?.attachParams || {}, { rosterEntityId, rosterScope = scope } = {}) {
    const entry = this._terms.get(termId);
    const binding = this._binding;
    await this._mutateShell("term.close", { ...terminalCloseScope(rosterScope), term_id: termId }, rosterScope, rosterEntityId);
    if (this._binding === binding && this._terms.get(termId) === entry) this._forgetTerm(termId);
  }

  createTerminal(scope, cols, rows, { rosterEntityId } = {}) {
    const params = terminalScope(scope);
    return this._mutateShell("term.create", { ...params, cols, rows }, params, rosterEntityId);
  }

  async listTerminals(scope) {
    const deviceId = this._userTerminalDevice();
    const result = await this._userTerminalRpc(deviceId, "term.list", terminalScope(scope),
      (method, params) => this._rosterCall(deviceId, method, params));
    return result.terminals || [];
  }

  attachTerminal(termId, scope = {}, options = undefined) {
    if (options === undefined) { options = scope; scope = {}; }
    return this._mountPane(termId, "user", terminalScope(scope), options || {});
  }

  attachAgent(target, options = {}) {
    return this._mountPane(`agent:pending-${++this._agentAttachSeq}`, "agent", { ...target }, options);
  }

  async input(termId, data) {
    const bytes = textEncoder.encode(data);
    await this._sendEffect("term.input", { term_id: termId, data: encodeInput(bytes) });
  }

  async resize(termId, cols, rows) {
    const entry = this._terms.get(termId);
    if (entry) Object.assign(entry, { cols, rows });
    await this._sendEffect("term.resize", { term_id: termId, cols, rows });
  }

  _watchPane(entry, dispose) {
    entry.signal?.addEventListener("abort", dispose, { once: true });
    entry.stopPane = () => entry.signal?.removeEventListener("abort", dispose);
  }

  _displaceAgentPane(termId) { this._forgetTerm(termId); }

  _restoreOrphanFrames(entry, frames) {
    if (frames.overflow) this._resnapshotPane(entry);
    else frames.forEach((frame) => this._receiveStreamFrame(entry, frame));
  }

  _forgetTerm(termId) {
    const entry = super._forgetTerm(termId);
    this._binding?.idle();
    return entry;
  }

  _releasePane(entry) {
    entry.stopPane?.();
    this._releaseAttachment(entry);
  }

  close() {
    this._closed = true;
    this.follow(null, null);
    this._failConnectWaiters("closed");
  }
}
