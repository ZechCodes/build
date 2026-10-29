// One role's proof lifecycle and output admission, independent of ICE and
// rendezvous lifetimes. Application codecs are installed only after an ack.
import { bindCarrier } from "./carrier.js";
import { createChannelBinding, DTLS_JSON_PROTOCOL } from "./peerBinding.js";

export const CHANNELS = Object.freeze([Object.freeze(["app", 0]), Object.freeze(["term", 1])]);

const CONTROL_LIMIT_BYTES = 64 * 1024;
const BIND_TIMEOUT_MS = 10000;
const controlSize = (message) => new TextEncoder().encode(JSON.stringify(message)).byteLength;
const checkedControl = (control) => {
  if (controlSize(control) > CONTROL_LIMIT_BYTES) throw new Error("oversized binding control");
  return control;
};
const isBindingControl = (message) => ["channel_challenge_v1", "channel_bound_v1"].includes(message?.type);

function pendingBinding(credentials) {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  // Setup may still be waiting for its authenticated answer when a malformed
  // early control fails. The outer setup consumes this rejection afterward.
  promise.catch(() => {});
  return { credentials, promise, resolve, reject, lease: new AbortController(), state: "challenge", early: null, proof: null, timer: null };
}

/** Proof controls never enter an application codec. This router also keeps an
 * existing terminal binding usable while its replacement proves possession. */
function createChannelBinder({ raw, channel, role, credentials = null }) {
  let selection = null;
  let pending = credentials ? pendingBinding(credentials) : null;
  const initial = pending?.promise;
  let stopClose = () => {};
  let active = null;
  let activeGate = null;
  let activeBindingId = null;
  let activeFrames = new Set();
  let ended = false;
  const current = (attempt) => !ended && pending === attempt;
  const fail = (attempt, error) => {
    if (!current(attempt)) return;
    pending = null;
    clearTimeout(attempt.timer);
    attempt.lease.abort(error);
    attempt.proof?.close();
    activeGate?.resume();
    attempt.reject(error);
  };
  const identity = () => {
    if (channel.id !== role[1] || channel.label !== role[0]) throw new Error("wrong native channel identity");
    if (channel.readyState !== "open") throw new Error("the binding channel is not open");
  };
  const startTimer = () => {
    if (!pending || pending.timer || channel.readyState !== "open") return;
    const attempt = pending;
    attempt.timer = setTimeout(() => fail(attempt, Object.assign(new Error("the channel binding timed out"), { blockedReason: "timeout" })), BIND_TIMEOUT_MS);
  };
  const endpoint = (listeners) => {
    const gate = createRoleWriteGate(raw);
    return { gate, carrier: {
      ...raw,
      onEnvelope(fn) { listeners.add(fn); return () => listeners.delete(fn); },
      sendGenerated: gate.sendGenerated,
    } };
  };
  const finish = async (attempt, message) => {
    attempt.state = "acknowledging";
    const codec = await attempt.proof.acceptAck(message);
    if (!current(attempt)) { codec.close(); return; }
    identity();
    const listeners = new Set();
    const output = endpoint(listeners);
    const replacement = bindCarrier(output.carrier, codec);
    const previous = active;
    active = replacement;
    activeGate = output.gate;
    activeBindingId = codec.bindingId;
    activeFrames = listeners;
    pending = null;
    clearTimeout(attempt.timer);
    previous?.retire(new Error("the terminal binding was replaced"));
    attempt.resolve(replacement);
  };
  const sendControl = (attempt, control) => raw.sendGenerated(() => checkedControl(control), {
    signal: attempt.lease.signal,
    assertCurrent: () => {
      if (!current(attempt)) throw new Error("the channel proof attempt ended");
      identity();
    },
  });
  const prove = async (attempt, message) => {
    attempt.state = "proving";
    identity();
    attempt.proof = attempt.credentials.createChannelBinding({ transcript: selection.transcript,
      channelId: role[1], channelLabel: role[0] });
    const control = await attempt.proof.prove(message);
    if (!current(attempt)) return;
    identity();
    attempt.state = "ack";
    await sendControl(attempt, control);
  };
  const process = async (attempt, message) => {
    try {
      if (attempt.state === "challenge" && message.type === "channel_challenge_v1") await prove(attempt, message);
      else if (attempt.state === "ack" && message.type === "channel_bound_v1") await finish(attempt, message);
      else throw new Error("unexpected channel binding control");
    } catch (error) { fail(attempt, error); }
  };
  const resume = () => {
    startTimer();
    if (!pending?.early || !selection || channel.readyState !== "open") return;
    const attempt = pending;
    const message = attempt.early;
    attempt.early = null;
    void process(attempt, message);
  };
  const receiveControl = (message) => {
    const attempt = pending;
    if (!attempt || message.session_id !== attempt.credentials.sessionId) return;
    if (controlSize(message) > CONTROL_LIMIT_BYTES) return fail(attempt, new Error("oversized binding control"));
    if (!selection || channel.readyState !== "open") {
      if (attempt.early) return fail(attempt, new Error("too many early binding controls"));
      attempt.early = message;
      return;
    }
    void process(attempt, message);
  };
  const receiveApplication = (message) => {
    if (active && pending && message?.binding_id !== activeBindingId) return fail(pending, new Error("application payload before replacement binding"));
    if (active) { for (const listener of [...activeFrames]) listener(message); }
    else if (pending) fail(pending, new Error("application payload before channel binding"));
  };
  const stopFrames = raw.onEnvelope((message) => {
    if (ended || (selection && selection.protocol !== DTLS_JSON_PROTOCOL)) return;
    if (isBindingControl(message)) receiveControl(message);
    else receiveApplication(message);
  });
  const close = () => {
    if (ended) return;
    if (pending) fail(pending, new Error("the peer binding closed"));
    ended = true;
    stopFrames();
    stopClose();
    channel.removeEventListener("open", resume);
    active?.retire(new Error("the peer binding closed"));
  };
  stopClose = raw.onClose(close);
  if (!ended) channel.addEventListener("open", resume);
  return {
    select(value) {
      selection = value;
      if (value.protocol === DTLS_JSON_PROTOCOL) resume();
      else if (pending) { clearTimeout(pending.timer); pending.resolve(null); pending = null; }
    },
    bound: () => initial,
    carrier: () => active?.isClosed() ? null : active,
    begin(nextCredentials, beginControl) {
      if (ended) return Promise.reject(new Error("the peer binding closed"));
      if (pending) fail(pending, new Error("the terminal binding attempt was replaced"));
      pending = pendingBinding(nextCredentials);
      activeGate?.pause();
      const attempt = pending;
      startTimer();
      sendControl(attempt, beginControl).catch((error) => fail(attempt, error));
      return attempt.promise;
    },
    close,
  };
}

export function createRoleBindings(binding, carriers, channels) {
  const idle = { select() {}, close() {}, bound: () => Promise.resolve(null) };
  if (!binding) return { appBinder: idle, termBinder: idle };
  return {
    appBinder: createChannelBinder({ raw: carriers[0], channel: channels[0], role: CHANNELS[0], credentials: binding }),
    termBinder: createChannelBinder({ raw: carriers[1], channel: channels[1], role: CHANNELS[1] }),
  };
}

export function terminalCredentials(minted, appBinding) {
  const credentials = minted.peerBinding || minted;
  if (credentials.createChannelBinding) return credentials;
  return {
    sessionId: credentials.sessionId,
    createChannelBinding: (options) => createChannelBinding({ ...options, transport: appBinding.transport, ...credentials }),
  };
}

/** Writes admitted before pause precede the proof on the physical FIFO. Later
 * writes remain unsent until failure resumes the old binding, or retirement
 * aborts them after the replacement ack. No sequence is allocated while held. */
function createRoleWriteGate(raw) {
  let paused = false;
  const waiting = new Set();
  const wait = (signal) => new Promise((resolve, reject) => {
    const cleanup = () => { waiting.delete(resume); signal?.removeEventListener("abort", abort); };
    const resume = () => { cleanup(); resolve(); };
    const abort = () => { cleanup(); reject(signal.reason); };
    waiting.add(resume);
    signal?.addEventListener("abort", abort, { once: true });
    if (signal?.aborted) abort();
  });
  const delayed = async (prepare, options) => {
    while (paused) await wait(options.signal);
    if (options.signal?.aborted) throw options.signal.reason;
    return raw.sendGenerated(prepare, options);
  };
  return {
    pause() { paused = true; },
    resume() { paused = false; for (const resume of [...waiting]) resume(); },
    sendGenerated(prepare, options = {}) {
      return paused ? delayed(prepare, options) : raw.sendGenerated(prepare, options);
    },
  };
}
