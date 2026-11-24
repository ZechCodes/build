const { contextBridge, ipcRenderer } = require("electron");
const invoke = (method, args) =>
  ipcRenderer.invoke("build:bridge-http", method, args);
function on(channel, listener) {
  if (typeof listener !== "function") throw new TypeError("Expected an event listener");
  const receive = (_event, payload) => listener(payload);
  ipcRenderer.on(channel, receive);
  return () => ipcRenderer.removeListener(channel, receive);
}
const facade = {
  discover: (origin) => invoke("discover", [origin]),
  discoverSaved: (origin, identity) => invoke("discoverSaved", [origin, identity]),
  settingsRead: () => invoke("settingsRead", []),
  settingsWrite: (change) => invoke("settingsWrite", [change]),
  resume: (identity) => invoke("resume", [identity]),
  open: (handle, apiRange) => invoke("open", [handle, apiRange]),
  request: (handle, operationId, params, options = {}) =>
    invoke("request", [handle, operationId, params, options]),
  subscribe: (handle, scope, options) =>
    invoke("subscribe", [handle, scope, options]),
  ack: (stream, sequence) => invoke("ack", [stream, sequence]),
  unsubscribe: (stream) => invoke("unsubscribe", [stream]),
  cancel: (requestId) => invoke("cancel", [requestId]),
  close: (handle) => invoke("close", [handle]),
  terminalOpen: (handle, requestId, body) => invoke("terminalOpen", [handle, requestId, body]),
  terminalCall: (terminal, method, params, id) => invoke("terminalCall", [terminal, method, params, id]),
  terminalConsume: (terminal, sequence) => invoke("terminalConsume", [terminal, sequence]),
  terminalDetach: (terminal, attachId) => invoke("terminalDetach", [terminal, attachId]),
  terminalClose: (terminal) => invoke("terminalClose", [terminal]),
  onEvent: (listener) => on("build:bridge-http-event", listener),
  onTerminalEvent: (listener) => on("build:bridge-http-terminal-event", listener),
};
contextBridge.exposeInMainWorld("buildBridgeHttp", Object.freeze(facade));
