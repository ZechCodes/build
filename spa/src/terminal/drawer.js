// The terminal drawer — the basement. Always accessible, never the default view.
// Lazily boots ghostty-web (bundled, wasm inlined) on first open.

import * as transport from "@build/secure-transport";
import { $ } from "../dom.js";
import { RELAY_URL } from "../config.js";
import { App } from "../app.js";
import { fetchGatewayToken } from "../api.js";
import { pinnedDeviceTransportKey } from "../devices.js";
import { TerminalSession } from "./session.js";

let termSession = null;
let termFitAddon = null;
let term = null;

/** Refit the terminal to the drawer's current size and tell the PTY, but only
 *  while the drawer is open (a hidden drawer has no measurable size). Driven by a
 *  ResizeObserver on the drawer and by window resize (rotation, split-view).
 *
 *  We compute the target grid with the addon's proposeDimensions() but apply it
 *  with a direct term.resize() rather than the addon's fit(): fit() carries a
 *  _lastCols cache and a 50ms _isResizing guard that, when several triggers fire
 *  close together, can swallow the real update. A direct resize is deterministic
 *  and idempotent (skipped when the grid is unchanged), and still emits the
 *  terminal's onResize → the PTY. */
function fitTerminalToViewport() {
  if (!termFitAddon || !term || !$("#drawer").classList.contains("show")) return;
  const dims = termFitAddon.proposeDimensions();
  if (!dims || !(dims.cols > 0) || !(dims.rows > 0)) return;
  if (dims.cols !== term.cols || dims.rows !== term.rows) term.resize(dims.cols, dims.rows);
}

/**
 * Re-point the drawer at the app session's (new) device. A healthy session
 * never reconnects on its own — the liveness ping keeps it pinned to the old
 * device — so a device switch must drop it; the auto-reconnect then re-reads
 * preferDeviceId and attaches to the new device's PTY.
 */
export function retargetTerminal() {
  if (!termSession) return;
  const wantedDeviceId = App.session?.deviceId || App.selectedDeviceId || null;
  if (wantedDeviceId && termSession.deviceId !== wantedDeviceId) termSession.simulateDrop();
}

export async function toggleTerminal() {
  const drawer = $("#drawer");
  if (drawer.classList.contains("show")) {
    drawer.classList.remove("show");
    return;
  }
  drawer.classList.add("show");
  if (!termSession) {
    const { init, Terminal, FitAddon } = await import("ghostty-web");
    await init();
    $("#term").innerHTML = "";
    term = new Terminal({ cols: 140, rows: 16, fontSize: 13, theme: { background: "#15161e", foreground: "#a9b1d6" } });
    term.open($("#term"));
    // Fit the terminal (and the PTY) to the drawer instead of a fixed 140×16 —
    // a fixed grid overflows a phone and never matches the real window.
    termFitAddon = new FitAddon();
    term.loadAddon(termFitAddon);
    termFitAddon.fit();
    termSession = new TerminalSession({
      url: RELAY_URL,
      transport,
      WebSocketImpl: WebSocket,
      getToken: fetchGatewayToken,
      getPinnedDeviceKey: pinnedDeviceTransportKey,
      // The terminal follows the app session's device (falling back to the
      // user's sticky choice), re-evaluated on every reconnect; switchDevice
      // calls retargetTerminal() to force that reconnect.
      preferDeviceId: () => App.session?.deviceId || App.selectedDeviceId || null,
    });
    termSession.onSnapshot((bytes) => {
      term.reset();
      term.write(bytes);
    });
    termSession.onOutput((bytes) => term.write(bytes));
    term.onData((data) => termSession.input(data).catch(() => {}));
    // Every refit (initial, ResizeObserver, or window resize) tells the PTY the
    // new grid so wrapping stays correct.
    term.onResize(({ cols, rows }) => termSession.resize(cols, rows).catch(() => {}));
    // Expose the live terminal for the QA harness (feature-check terminal-resize).
    window.__buildTerminal = term;
    await termSession.start(term.cols, term.rows);
    // Refit when the drawer's box changes (viewport resize, rotation, keyboard).
    new ResizeObserver(() => fitTerminalToViewport()).observe($("#term"));
    window.addEventListener("resize", fitTerminalToViewport);
  } else {
    // Re-fit on reopen — the viewport may have changed while it was closed.
    fitTerminalToViewport();
  }
}

export function initTerminalDrawer() {
  $("#dx").onclick = () => $("#drawer").classList.remove("show");
  window.addEventListener("keydown", (e) => {
    if (e.key === "`" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName)) {
      e.preventDefault();
      toggleTerminal();
    }
  });
}
