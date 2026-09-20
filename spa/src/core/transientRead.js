// A read that failed because the wire went away, told apart from a read the
// bridge refused — and what a surface does about each.
//
// # Why this exists
//
// A phone's session dies at the network layer every few minutes (ICE
// disconnected → failed, a new session minted over the relay). Every call in
// flight when that happens fails, and every surface reported each one: "Could
// not read this issue", over an issue that was on screen the whole time,
// painted from the cache a moment earlier. The read had failed; nothing the
// reader could see had.
//
// So the rule is about what the reader is looking at, not about what failed:
//
//   the wire went away, and there is a copy on screen    → say nothing, wait
//   the wire went away, and there is nothing on screen   → wait, then say so
//   the bridge refused                                   → say so, as always
//
// "Wait" is a subscription, never a cadence: the surface is told when its
// machine can answer again and reads once. Nothing here polls.
//
// # What counts as the wire going away
//
// Positive evidence only. A call that timed out or whose delivery is unknown
// says so on the Error (core/sessionRpc.js); every other way a session, a
// carrier or a rendezvous can end fails calls with one of a small set of
// sentences, and those are listed below against the modules that throw them.
//
// Everything else is said out loud — including anything unrecognised. A bug in
// this client that threw a TypeError would otherwise be swallowed into a wait
// that never ends, and a surface silently showing an old copy forever is a
// worse failure than one toast too many.
//
// This module is pure: the machine it is watching is injected (`watch`), which
// core/deviceReconnect.js supplies over the real registry.

import { deviceBlockedMark, deviceOfflineMark, lastReadText, messageOf } from "./text.js";

/**
 * Every sentence the transport itself fails a call with.
 *
 * Each is quoted from the module that throws it; `transientRead.test.js` reads
 * those modules and holds them to it, because a reworded sentence here is not
 * a broken test anywhere — it is every dropped read back in a toast, silently.
 */
const TRANSPORT_WENT = Object.freeze([
  "your device went offline", // core/session.js, severed and as its noCarrier
  "your device is offline — reconnecting…", // core/session.js, while it is being made again
  "session closed", // core/session.js, deliberately let go of
  "nothing is carrying this session", // core/sessionRpc.js, the default noCarrier
  "the channel closed", // core/carrier.js
  "the relay socket closed", // core/carrier.js
  "the rendezvous closed", // core/rendezvous.js
  deviceOfflineMark, // core/deviceContexts.js refusing a call to a machine that is away
  deviceBlockedMark, // ...and to one this browser could not reach directly
]);

const WENT = new Set(TRANSPORT_WENT);

/**
 * Whether this read failed because the wire went away rather than because the
 * bridge answered no.
 *
 * A machine that is merely BEHIND is not this: it is answering, in a shape
 * this tab cannot read, and reconnecting is not the cure — so it is said out
 * loud like any other refusal rather than waited on forever.
 */
export function isTransientTransportError(error) {
  if (!error) return false;
  if (error.timedOut === true || error.uncertain === true) return true;
  return WENT.has(messageOf(error).trim());
}

/** The mark a surface wears while it is holding a copy quietly. Prepended to
 *  the surface rather than woven into what it drew, so every surface wears the
 *  same one and no renderer has to know this exists. */
const NOTE_CLASS = "read-wait";

const noteIn = (host) => {
  const first = host?.firstElementChild;
  return first?.classList?.contains(NOTE_CLASS) ? first : null;
};

function paintNote(host, text) {
  if (!host) return;
  const shown = noteIn(host);
  if (!text) {
    shown?.remove();
    return;
  }
  if (shown) {
    shown.textContent = text;
    return;
  }
  const note = host.ownerDocument.createElement("p");
  note.className = NOTE_CLASS;
  // Announced, because nothing else tells a reader who cannot see the surface
  // that what they are on is not the newest thing there is.
  note.setAttribute("role", "status");
  note.textContent = text;
  host.prepend(note);
}

/**
 * One surface's answer to a read that failed.
 *
 * `watch` is the machine, as three questions: is it away right now (`away`),
 * is something being done about it (`reconnecting`), and tell me when either
 * changes (`moved`, returning its own unsubscribe).
 *
 * `hasContent` is the surface's own answer to "is there anything on me" — a
 * cached paint counts. It is what decides whether a failure is worth saying.
 *
 * `retry` is the surface's own read, run once when the machine is back.
 */
export function createReadRetry({ host = null, watch, retry, hasContent = () => false, now = () => Date.now() }) {
  let lastReadAt = 0;
  let waiting = false;
  let retrying = false;
  let off = null;

  const disarm = () => {
    const taking = off;
    off = null;
    taking?.();
  };

  const mark = () => paintNote(host, waiting && watch.reconnecting() ? lastReadText(lastReadAt) : "");

  const rest = () => {
    waiting = false;
    retrying = false;
    disarm();
    mark();
  };

  /** The machine moved. Still away: only the mark changes, because "in 3
   *  seconds" becoming "reconnecting" is news. Back: read once. */
  const moved = () => {
    mark();
    // `off` and not just `away()`: the two subscriptions behind `moved`
    // announce one reconnect twice, and a surface read twice is a surface that
    // read once too often.
    if (!off || watch.away()) return;
    disarm();
    retrying = true;
    retry();
  };

  const arm = () => {
    if (!off) off = watch.moved(moved);
  };

  return {
    /** A copy painted from the cache, read when the cache took it. Only stands
     *  in until a live read lands: after that the surface knows better. */
    seen(at) {
      if (!lastReadAt && at) lastReadAt = at;
      mark();
    },

    /** A read landed. Whatever was being waited for has happened. */
    succeeded(at) {
      lastReadAt = at || now();
      rest();
    },

    /**
     * A read failed. True when it was handled quietly and the caller should
     * leave the surface as it is; false when the caller says it out loud.
     */
    failed(error) {
      if (!isTransientTransportError(error)) {
        rest();
        return false;
      }
      const wasRetry = retrying;
      retrying = false;
      // The machine is answering: this was one lost call, not a dead session,
      // so there is no reconnect to wait for. Waiting on one that will never
      // come would hold the surface silent forever.
      if (!watch.away()) {
        rest();
        return hasContent();
      }
      waiting = true;
      arm();
      mark();
      return !wasRetry || hasContent();
    },

    /** Redraw the mark. A surface that rewrites its own innards calls this
     *  after, because the mark lives among them. */
    mark,
    waiting: () => waiting,
    dispose: disarm,
  };
}
