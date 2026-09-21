// A harness out of usage on a device (issue #58).
//
// When Claude runs out of usage, every agent on that machine using it stops,
// and until this existed nothing in the client said why: a limited agent looked
// exactly like one waiting for the human. The bridge keeps one record per
// harness and carries the list on its board (`board.list`, and the board item of
// a `changes` push, since wire 1.11.0); this module holds what each device last
// said, and paints it where the reader is — a strip at the top of every
// conversation on that machine, counting down to the reset, with the harness's
// own words behind a press, and the same reason on a message still queued for
// the agent.
//
// Live state rather than cached: a limit is about the machine now, and one read
// from disk after the machine has moved on would be a banner telling a lie.

import { esc } from "./text.js";
import { providerLabel } from "./providerCatalog.js";

/** What each device last said, by id: the bridge's `usage_limits` as it sent them. */
const byDevice = new Map();
const listeners = new Set();

/** Record what a device's board said about its limits. A list the bridge did
 *  not send (an older bridge, or a board item that moved something else) says
 *  nothing, and changes nothing. */
export function setUsageLimits(deviceId, limits) {
  if (!deviceId || !Array.isArray(limits)) return;
  const was = JSON.stringify(byDevice.get(deviceId) || []);
  if (was === JSON.stringify(limits)) return;
  if (limits.length) byDevice.set(deviceId, limits);
  else byDevice.delete(deviceId);
  for (const listener of [...listeners]) listener(deviceId);
}

export const usageLimitsOf = (deviceId) => byDevice.get(deviceId) || [];

export function onUsageLimitsChanged(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** For tests: forget every device. */
export function resetUsageLimits() {
  byDevice.clear();
  listeners.clear();
}

/** Who ran out, as the reader calls it: the harness family, not the carrier —
 *  "Claude", whether its agents run headless or in a terminal. */
const FAMILY_NAMES = { claude_adk: "Claude", claude: "Claude", codex_app_server: "Codex", codex: "Codex", pi: "Pi" };
const harnessName = (harness) => FAMILY_NAMES[harness] || providerLabel(harness);

/** Which limit, in the harness's own noun: "session limit", "weekly limit",
 *  "Opus limit". The harness names each of its windows differently, and the
 *  banner follows it; a sentence that names none reads as the session's. */
function limitName(said) {
  const named = /hit your (.+?) limit\b/i.exec(said || "");
  return named ? `${named[1]} limit` : "session limit";
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** How long until the reset, in the coarsest units that still move: minutes
 *  inside the hour, hours and minutes inside the day, days and hours beyond.
 *  A part unit rounds up, so the count never says a reset is sooner than it is. */
function resetText(resetsAt, now) {
  const at = resetsAt ? Date.parse(resetsAt) : NaN;
  if (Number.isNaN(at)) return "reset time unknown";
  const left = at - now;
  if (left <= 0) return "resets now";
  if (left <= HOUR) return `resets in ${Math.ceil(left / MINUTE)} min`;
  if (left <= 24 * HOUR) {
    const minutes = Math.ceil(left / MINUTE);
    const rest = minutes % 60;
    return `resets in ${Math.floor(minutes / 60)} h${rest ? ` ${rest} min` : ""}`;
  }
  const hours = Math.ceil(left / HOUR);
  const rest = hours % 24;
  return `resets in ${Math.floor(hours / 24)} d${rest ? ` ${rest} h` : ""}`;
}

/** The banner's one line: "Claude session limit reached · resets in 34 min". */
export function usageLimitText(limit, now = Date.now()) {
  return `${harnessName(limit.harness)} ${limitName(limit.said)} reached · ${resetText(limit.resets_at, now)}`;
}

/** How long until the line above says something different, so the countdown
 *  repaints on the minute it moves rather than on a clock of its own. `null`
 *  when it never will. */
function untilTextChanges(limit, now) {
  const at = limit.resets_at ? Date.parse(limit.resets_at) : NaN;
  if (Number.isNaN(at) || at <= now) return null;
  return ((at - now) % MINUTE) || MINUTE;
}

/** How long until any of these devices' lines says something different; `null`
 *  when none ever will. For a surface that paints several devices' limits and
 *  counts them down together. */
export function untilAnyTextChanges(deviceIds, now = Date.now()) {
  const waits = deviceIds
    .flatMap((deviceId) => usageLimitsOf(deviceId))
    .map((limit) => untilTextChanges(limit, now))
    .filter((wait) => wait !== null);
  return waits.length ? Math.min(...waits) : null;
}

/** Why a message to an agent on `provider` is still queued, when the reason is
 *  that its harness is out of usage on this device; `null` otherwise. */
export function queuedReason(deviceId, provider, now = Date.now()) {
  const held = usageLimitsOf(deviceId).find((limit) => limit.harness === provider);
  return held ? usageLimitText(held, now) : null;
}

/** Say the reason on every Queued chip under `root`, or stop saying it. A pass
 *  over what is painted rather than a word inside the thread's renderer,
 *  because nothing about a message changes when its harness runs out. */
export function applyQueuedReason(root, reason) {
  for (const chip of root.querySelectorAll(".delivery-status.queued, .delivery-status.submitted")) {
    if (reason) chip.title = reason;
    else chip.removeAttribute("title");
  }
}

const bannerHtml = (limit, now) => `<details class="usage-limit-banner" data-harness="${esc(limit.harness)}">
  <summary><span class="usage-limit-text">${esc(usageLimitText(limit, now))}</span></summary>
  <p class="usage-limit-said">${esc(limit.said)}</p>
</details>`;

/**
 * The strip at the top of a conversation on `deviceId`: one line per harness
 * out of usage there, above the conversation's body.
 *
 * `panelOf` answers the panel as it stands, since the rail rebuilds its panel
 * whenever it switches what it shows; `sync` puts the strip back after one,
 * and a press that opened a line is kept across every repaint. The strip also
 * repaints itself when the device's limits change and on each minute the
 * countdown moves. `onPaint` runs after every paint, for whatever else the
 * surface says about the same limit (the Queued hover). Returns
 * `{ sync, dispose }`.
 */
export function mountUsageLimitBanner(panelOf, deviceId, { onPaint = () => {} } = {}) {
  let timer = null;
  const sync = () => {
    clearTimeout(timer);
    timer = null;
    const panel = panelOf();
    if (!panel) return;
    const now = Date.now();
    paintBanners(panel, usageLimitsOf(deviceId), now);
    onPaint();
    const next = untilAnyTextChanges([deviceId], now);
    if (next !== null) timer = setTimeout(sync, next);
  };
  const stop = onUsageLimitsChanged((changed) => {
    if (changed === deviceId) sync();
  });
  sync();
  return {
    sync,
    dispose() {
      stop();
      clearTimeout(timer);
      const panel = panelOf();
      if (panel) paintBanners(panel, [], Date.now());
    },
  };
}

/** One strip or none, with one line per limit, rewritten only when its words
 *  moved — the rail repaints its panel far more often than a minute — and
 *  keeping whichever lines the reader had opened. */
function paintBanners(panel, limits, now) {
  let strip = panel.querySelector(":scope > .usage-limit-banners");
  if (!limits.length) {
    strip?.remove();
    return;
  }
  if (!strip) {
    strip = document.createElement("div");
    strip.className = "usage-limit-banners";
    strip.setAttribute("role", "status");
    const body = panel.querySelector(":scope > .rail-body");
    panel.insertBefore(strip, body);
  }
  const html = limits.map((limit) => bannerHtml(limit, now)).join("");
  if (strip.dataset.painted === html) return;
  const open = new Set(
    [...strip.querySelectorAll("details[open]")].map((line) => line.dataset.harness),
  );
  strip.innerHTML = html;
  strip.dataset.painted = html;
  for (const line of strip.querySelectorAll("details")) line.open = open.has(line.dataset.harness);
}
