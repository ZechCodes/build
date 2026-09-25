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

const errorText = ({ error, message } = {}) => (message ? `${error}: ${message}` : error || "");

/// A sentence ends once: an error message that already has its full stop keeps it.
const asSentence = (text) => (text && !/[.!?]$/.test(text) ? `${text}.` : text);

const seconds = (ms) => `${(Math.max(0, Number(ms) || 0) / 1000).toFixed(1)} s`;

/// Why the cache stood down, where there is something truer to say than the
/// browser's error: what it was and what brings it back.
const STOOD_DOWN_BECAUSE = Object.freeze({
  blocked: "another tab with an older version of Build kept it closed. Close that tab and reload to turn it back on.",
});

/// One sentence per state. Polymorphic on the state's name rather than a chain
/// of conditions: each state says its own thing.
/// A write refused for want of space is the one refusal somebody can act on.
const refusedText = (refused) => (refused?.error === "QuotaExceededError"
  ? ` At ${formatDiagnosticTime(refused.at)} a write was not kept because this browser's storage for Build was full.`
  : "");

const SAYS = Object.freeze({
  ready: ({ lastRecovery, lastRefused }) => (lastRecovery
    ? `Working. Reconnected at ${formatDiagnosticTime(lastRecovery.at)} after ${seconds(lastRecovery.afterMs)}.`
    : "Working.") + refusedText(lastRefused),
  recovering: (health) => `Reconnecting since ${formatDiagnosticTime(health.since)}. ${errorText(health)}`.trim(),
  resting: (health) =>
    `Not answering since ${formatDiagnosticTime(health.since)}. Trying again when the app is next opened, or in a moment. ${errorText(health)}`.trim(),
  blocked: (health) => `Waiting since ${formatDiagnosticTime(health.since)} for another tab with an older version of Build to close.`,
  "stood-down": (health) => (STOOD_DOWN_BECAUSE[health.reason]
    ? `Off for this session since ${formatDiagnosticTime(health.at)}: ${STOOD_DOWN_BECAUSE[health.reason]}`
    : `Off for this session since ${formatDiagnosticTime(health.at)}. ${asSentence(errorText(health))} Reload to turn it back on.`),
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
