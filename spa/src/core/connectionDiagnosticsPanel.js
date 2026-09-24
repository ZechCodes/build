// Settings → Diagnostics: the connection dump, on the screen it is needed on.
//
// deploy/OPS.md tells people to run `buildConnectionDiagnostics()` in a console
// before reloading a tab that lost its connection. A phone has no console, and
// a phone is where the reconnects are — so the same history is read here,
// through the module's export rather than the global, and can be copied,
// shared, or cleared without a keyboard.
//
// Markup first, then mount, the shape core/downloads.js and core/harnessDefaults.js
// use: the host ships the panel in its own html and this fills it.

import { esc } from "./text.js";
import { diagnosticRows, diagnosticsJson, diagnosticsSummary } from "./connectionDiagnosticsModel.js";
import { applyFieldTraits } from "./fieldTraits.js";

/// How often the open section re-reads the history.
///
/// A poll rather than a subscription because the history is a plain array with
/// no hook to offer, and a second is under the time it takes to look from a
/// button to the list. It is armed on mount and dropped on dispose — nothing
/// ticks while Settings is not on screen.
const POLL_MS = 1000;

/// What the dump holds, in the words deploy/OPS.md uses. It is on the page
/// rather than behind a link because the buttons under it send this somewhere:
/// whoever presses Share is owed the contents before they do.
const PRIVACY_NOTE =
  "Timestamps and connection identifiers only — no message content, no keys, no addresses. The history lives in this tab and a reload clears it.";

export function connectionDiagnosticsPanelHtml() {
  return `<div class="panel" id="diagnostics">
      <h3>🔌 Diagnostics</h3>
      <div class="dim" style="font-size:13px;margin-bottom:8px">The last connection events this tab recorded. Useful when a device keeps dropping.</div>
      <div class="dim" style="font-size:12px;margin-bottom:10px">${esc(PRIVACY_NOTE)}</div>
      <div class="addproj">
        <button class="btn mini" id="diagcopy">Copy</button>
        <button class="btn mini" id="diagshare" hidden>Share</button>
        <button class="btn mini" id="diagclear">Clear</button>
      </div>
      <div class="dim" id="diagsummary" style="font-size:12px;margin:10px 0 2px" role="status"></div>
      <div class="diaglist" id="diaglist"></div>
      <div class="adderr" id="diagerr"></div>
    </div>`;
}

/// One event. The head wraps — time, machine, kind — and the two long strings
/// under it are monospace and scroll sideways rather than forcing the panel
/// wide, because a 390px screen has neither the room nor a way to zoom out.
const rowHtml = (row) => `<div class="diagrow">
      <div class="diagrow-head">
        <span class="diagrow-time mono">${esc(row.time)}</span>
        <span class="diagrow-device">${esc(row.device)}</span>
        <span class="diagrow-kind mono">${esc(row.kind)}</span>
      </div>
      ${row.source ? `<div class="diagrow-source mono">${esc(row.source)}</div>` : ""}
      ${row.detail ? `<div class="diagrow-detail mono">${esc(row.detail)}</div>` : ""}
    </div>`;

const EMPTY_HTML = '<div class="dim" style="font-size:13px">Nothing recorded yet. Connection events show up here as they happen.</div>';

export const diagnosticsListHtml = (rows) => (rows.length ? rows.map(rowHtml).join("") : EMPTY_HTML);

/// Put text on the clipboard, by whichever route this browser gives.
///
/// `navigator.clipboard` is unavailable over plain http and in a few mobile
/// browsers, and that is exactly where somebody reading this panel may be —
/// so a hidden textarea and `execCommand` stand behind it. Answers whether the
/// text got there, because the button says so either way.
export async function copyText(text, { clipboard, page = document } = {}) {
  try {
    if (clipboard?.writeText) {
      await clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the textarea */
  }
  return copyThroughTextarea(text, page);
}

function copyThroughTextarea(text, page) {
  const holder = page.createElement("textarea");
  holder.value = text;
  holder.setAttribute("readonly", "");
  applyFieldTraits(holder, "identifier");
  holder.style.position = "fixed";
  holder.style.opacity = "0";
  page.body.appendChild(holder);
  try {
    holder.select();
    return page.execCommand ? page.execCommand("copy") : false;
  } catch {
    return false;
  } finally {
    holder.remove();
  }
}

/// Say what a press did, on the button itself, and put it back after a moment.
function reportOnButton(button, said, timers) {
  const was = button.textContent;
  button.textContent = said;
  timers.add(setTimeout(() => (button.textContent = was), 1500));
}

/// Repaint the list from the history, unless it would say what is on screen
/// already — a tick that rebuilt an unchanged list would drop the reader out of
/// wherever they had scrolled to, once a second.
function painterFor(panel, { history, devices }) {
  const list = panel.querySelector("#diaglist");
  const summary = panel.querySelector("#diagsummary");
  let painted = null;
  return () => {
    const rows = diagnosticRows(history(), devices());
    const html = diagnosticsListHtml(rows);
    if (html === painted) return;
    painted = html;
    list.innerHTML = html;
    summary.textContent = diagnosticsSummary(rows);
  };
}

function wireCopy(panel, { report, clipboard, timers, say }) {
  const copy = panel.querySelector("#diagcopy");
  copy.onclick = async () => {
    say("");
    if (await copyText(diagnosticsJson(report()), { clipboard })) reportOnButton(copy, "Copied", timers);
    else say("This browser would not take the clipboard. Select the list and copy it by hand.");
  };
}

/// Share is the phone's way out of a tab with no console and no file system.
/// Wired only where the browser actually has one: a button that opens nothing
/// is worse than no button, so it stays hidden otherwise.
function wireShare(panel, { report, share, say }) {
  if (!share) return;
  const button = panel.querySelector("#diagshare");
  button.hidden = false;
  button.onclick = async () => {
    say("");
    try {
      await share({ title: "Build connection diagnostics", text: diagnosticsJson(report()) });
    } catch (refusal) {
      // A share sheet the user dismissed reports as an abort. That is not a
      // failure worth a red line under the button they just pressed.
      if (refusal?.name !== "AbortError") say(refusal?.message || "Sharing failed.");
    }
  };
}

function wireClear(panel, { clear, paint, timers, say }) {
  const button = panel.querySelector("#diagclear");
  button.onclick = () => {
    say("");
    clear?.();
    paint();
    reportOnButton(button, "Cleared", timers);
  };
}

/**
 * Fill the panel and keep it current until it is disposed.
 *
 * Everything it reads is injected: the history and its clear, the account's
 * machines, the clipboard, and the share sheet. That is what lets a test drive
 * it, and it is also the honest dependency list — this module knows how to show
 * a dump, and nothing about where connections come from.
 *
 * Answers a dispose. The caller registers it; the poll and every pending
 * "Copied" go with it.
 */
export function mountConnectionDiagnostics(host, options = {}) {
  const panel = host?.querySelector?.("#diagnostics");
  if (!panel) return () => {};
  const { history, report, clear, devices, clipboard, share, pollMs } = diagnosticsDependencies(options);
  const error = panel.querySelector("#diagerr");
  const say = (message) => { error.textContent = message; };
  const timers = new Set();
  const paint = painterFor(panel, { history, devices });
  paint();
  const ticker = setInterval(paint, pollMs);

  // The list renders the events; the copy and the share carry the whole record,
  // including how many events the ring had to drop (#60).
  wireCopy(panel, { report, clipboard, timers, say });
  wireShare(panel, { report, share, say });
  wireClear(panel, { clear, paint, timers, say });

  return () => {
    clearInterval(ticker);
    for (const timer of timers) clearTimeout(timer);
    timers.clear();
  };
}

/// What the panel reads, with this browser's own answers behind whatever the
/// caller did not name. Gathered here rather than in the signature so the mount
/// reads as the six things it does and not as a list of fallbacks.
function diagnosticsDependencies({ history, report, clear, devices, clipboard, share, pollMs } = {}) {
  const navigator = globalThis.navigator;
  return {
    history,
    // A caller that names only `history` still copies something sensible: the
    // events, with nothing claimed about what was dropped.
    report: report || history,
    clear,
    devices: devices || NO_DEVICES,
    clipboard: clipboard === undefined ? navigator?.clipboard : clipboard,
    share: share === undefined ? navigator?.share?.bind(navigator) : share,
    pollMs: pollMs || POLL_MS,
  };
}

const NO_DEVICES = () => [];
