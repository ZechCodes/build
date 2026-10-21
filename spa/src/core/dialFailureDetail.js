// What a failed dial says about itself in the diagnostics.
//
// Every way the rendezvous could fail — the gateway token, the relay socket, the
// relay's open deadline, the device's answer — used to land as `unreached` with
// `status: null` and nothing else. The 2026-10-21 wake report was four of those
// in thirty seconds, each failing in about fifty milliseconds, and it could not
// say whether the laptop's network was back yet or the relay was refusing it.
//
// So a failure now carries the step it died on (tagged by the rendezvous), what
// kind of thing was thrown, and whether the browser believed it was online. Its
// words are kept only when this app or a browser's network stack wrote them: a
// report is copied and pasted, and a server's error text is not ours to paste
// (an ICE-server refusal once carried credentials).

/** Words a failure may keep: the app's own, and the fixed wording browsers
 *  give a fetch that never reached a server. Anything else is dropped. */
const OWN_WORDS = new Set([
  // core/rendezvous.js
  "the rendezvous closed",
  "the relay did not answer",
  "device did not answer",
  // api.js
  "could not mint a gateway token",
  // core/session.js
  "your device went offline",
  // Chromium / Electron, Firefox, Safari, Node
  "Failed to fetch",
  "NetworkError when attempting to fetch resource.",
  "Load failed",
  "fetch failed",
]);

/** An Error by its name; a socket's error event (which is not one) by its type. */
function kindOf(error) {
  if (error instanceof Error) return error.name;
  if (typeof error?.type === "string") return `Event:${error.type}`;
  return error == null ? null : typeof error;
}

const ownWords = (error) => (OWN_WORDS.has(error?.message) ? error.message : null);

export function dialFailureDetail(error, { online = globalThis.navigator?.onLine } = {}) {
  return {
    step: error?.dialStep ?? null,
    error: kindOf(error),
    message: ownWords(error),
    online: typeof online === "boolean" ? online : null,
  };
}
