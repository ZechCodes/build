// The local cache's state, under the Diagnostics dump beside the build line.
//
// Every surface paints from the cache, so a cache that stopped answering looks
// exactly like a bridge that sent nothing: blank screens either way (#169). A
// phone has no console to tell them apart, so the cache's own account of itself
// sits here — working, reconnecting, waiting to try again, or off for the
// session and why. The events behind it are in the dump above, under
// `local-cache`.

import { esc } from "./text.js";
import { cacheHealth } from "./localCache.js";
import { formatDiagnosticTime } from "./connectionDiagnosticsModel.js";

/// How often the line re-reads the cache's state while Settings is open. The
/// state has no subscription to offer, and a second is under the time it takes
/// to look from a reconnect to this line.
const POLL_MS = 1000;

const seconds = (ms) => `${(Math.max(0, Number(ms) || 0) / 1000).toFixed(1)} s`;

/// The line says what happened in words, never the browser's own message: the
/// message can contradict how Build works ("Refresh the page to try again"),
/// and the error behind every state is in the Diagnostics dump above.
const PRIVATE_WINDOW_ERRORS = new Set(["InvalidStateError", "SecurityError"]);

/// Why an open failed for good, by the error it failed with.
const openFailedBecause = ({ error }) => {
  if (error === "VersionError") return "a newer version of Build has changed it. Reload to use that version.";
  if (PRIVATE_WINDOW_ERRORS.has(error)) return "this browser refused to open it, as a private window does. Nothing is kept between visits.";
  return "this browser refused to open it. Reload to try again.";
};

/// Why the cache stood down, by the reason it gave: what it was, and what
/// brings it back.
const STOOD_DOWN_BECAUSE = Object.freeze({
  blocked: () => "another tab with an older version of Build kept it closed. Close that tab and reload to turn it back on.",
  persistent: () => "it kept failing to open. Reload to try again.",
  "open-failed": openFailedBecause,
  "transaction-failed": () => "this browser refused to use it. Reload to try again.",
});
const standDownText = (health) => (STOOD_DOWN_BECAUSE[health.reason] || (() => "Reload to turn it back on."))(health);

/// A write refused for want of space is the one refusal somebody can act on.
const refusedText = (refused) => (refused?.error === "QuotaExceededError"
  ? ` At ${formatDiagnosticTime(refused.at)} a write was not kept because this browser's storage for Build was full.`
  : "");

/// One sentence per state. Polymorphic on the state's name rather than a chain
/// of conditions: each state says its own thing.
const SAYS = Object.freeze({
  ready: ({ lastRecovery, lastRefused }) => (lastRecovery
    ? `Working. Reconnected at ${formatDiagnosticTime(lastRecovery.at)} after ${seconds(lastRecovery.afterMs)}.`
    : "Working.") + refusedText(lastRefused),
  recovering: (health) => `Reconnecting since ${formatDiagnosticTime(health.since)}.`,
  resting: (health) =>
    `Not answering since ${formatDiagnosticTime(health.since)}. Trying again when the app is next opened, or in a moment.`,
  blocked: (health) => `Waiting since ${formatDiagnosticTime(health.since)} for another tab with an older version of Build to close.`,
  "stood-down": (health) => `Off for this session since ${formatDiagnosticTime(health.at)}: ${standDownText(health)}`,
  absent: () => "This browser has no IndexedDB, so nothing is kept between visits.",
});

/** What the line says about one `cacheHealth()` answer. */
export const cacheHealthText = (health) => (SAYS[health?.state] || SAYS.absent)(health || {});

/// The line itself: one fact, laid out like the build line under it.
export function cacheHealthLineHtml() {
  return `<div class="buildversion" id="cachehealth">
      <span class="dim">Local cache</span>
      <span class="mono" id="cachehealthtext" role="status">${esc(cacheHealthText(cacheHealth()))}</span>
    </div>`;
}

/**
 * Keep the line current until disposed. `health` is injected so a test can
 * drive it; the page's own is the cache's. Answers the dispose, which the host
 * registers with the rest of its teardown.
 */
export function mountCacheHealthLine(host, { health = cacheHealth, pollMs = POLL_MS } = {}) {
  const text = host?.querySelector?.("#cachehealthtext");
  if (!text) return () => {};
  const paint = () => {
    const said = cacheHealthText(health());
    if (text.textContent !== said) text.textContent = said;
  };
  paint();
  const ticker = setInterval(paint, pollMs);
  return () => clearInterval(ticker);
}
