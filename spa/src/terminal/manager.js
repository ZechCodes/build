// The per-browser-tab terminal socket owner. ONE TerminalSocket multiplexes
// every terminal tab (user shells + agent screens) by term_id, kept off the app
// RPC session so PTY floods never head-of-line-block RPCs. Created lazily on the
// first terminal/agent tab mount — users who never open one pay no socket.

import * as transport from "@build/secure-transport";
import { RELAY_URL } from "../config.js";
import { App } from "../app.js";
import { fetchGatewayToken } from "../api.js";
import { pinnedDeviceTransportKey } from "../devices.js";
import { TerminalSocket } from "./session.js";
import { createStatusHub } from "./statusHub.js";

let socket = null;

// One hub for the whole page: the singleton socket's status feeds it, every pane
// overlay subscribes to it. Lives at module scope so subscribeTerminalStatus
// works even before the first pane mounts the socket.
const statusHub = createStatusHub();

export { createStatusHub };

/** Subscribe to the terminal socket's connectivity status. The callback fires
 *  immediately with the current status if one is already known. Returns an
 *  unsubscribe function. */
export function subscribeTerminalStatus(fn) {
  return statusHub.subscribe(fn);
}

export function terminalManager() {
  if (!socket) {
    socket = new TerminalSocket({
      url: RELAY_URL,
      transport,
      WebSocketImpl: WebSocket,
      getToken: fetchGatewayToken,
      getPinnedDeviceKey: pinnedDeviceTransportKey,
      // The terminals follow the app session's device (falling back to the
      // user's sticky choice), re-evaluated on every reconnect; switchDevice
      // calls retargetTerminals() to force that reconnect.
      preferDeviceId: () => App.session?.deviceId || App.selectedDeviceId || null,
    });
    socket.onStatus((status) => statusHub.set(status));
    // A failed initial connect self-heals: _connect closes the socket, whose
    // close event schedules the backoff reconnect.
    socket.start().catch(() => {});
  }
  return socket;
}

/**
 * Re-point the terminal socket at the app session's (new) device. A healthy
 * socket never reconnects on its own — the liveness ping keeps it pinned to the
 * old device — so a device switch must drop it; the auto-reconnect then re-reads
 * preferDeviceId, attaches to the new device, and re-attaches every open tab.
 */
export function retargetTerminals() {
  if (!socket) return;
  const wantedDeviceId = App.session?.deviceId || App.selectedDeviceId || null;
  if (wantedDeviceId && socket.deviceId !== wantedDeviceId) socket.simulateDrop();
}
