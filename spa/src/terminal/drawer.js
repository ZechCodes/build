// The terminal drawer — the basement. Always accessible, never the default view.
// Lazily boots ghostty-web (bundled, wasm inlined) on first open.

import * as transport from "@build/secure-transport";
import { $ } from "../dom.js";
import { RELAY_URL } from "../config.js";
import { App } from "../app.js";
import { fetchGatewayToken } from "../api.js";
import { TerminalSession } from "./session.js";

let termSession = null;

export async function toggleTerminal() {
  const drawer = $("#drawer");
  if (drawer.classList.contains("show")) {
    drawer.classList.remove("show");
    return;
  }
  drawer.classList.add("show");
  if (!termSession) {
    const { init, Terminal } = await import("ghostty-web");
    await init();
    $("#term").innerHTML = "";
    const term = new Terminal({ cols: 140, rows: 16, fontSize: 13, theme: { background: "#15161e", foreground: "#a9b1d6" } });
    term.open($("#term"));
    termSession = new TerminalSession({
      url: RELAY_URL,
      transport,
      WebSocketImpl: WebSocket,
      getToken: fetchGatewayToken,
      // The terminal follows the app session's device (falling back to the
      // user's sticky choice), re-evaluated on every reconnect.
      preferDeviceId: () => App.session?.deviceId || App.selectedDeviceId || null,
    });
    termSession.onSnapshot((bytes) => {
      term.reset();
      term.write(bytes);
    });
    termSession.onOutput((bytes) => term.write(bytes));
    term.onData((data) => termSession.input(data).catch(() => {}));
    await termSession.start(140, 16);
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
