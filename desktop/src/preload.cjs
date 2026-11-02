const { contextBridge, ipcRenderer } = require("electron");
const invoke = (method, args) =>
  ipcRenderer.invoke("build:bridge-http", method, args);
const facade = {
  discover: (origin) => invoke("discover", [origin]),
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
  onEvent: (listener) => {
    if (typeof listener !== "function")
      throw new TypeError("Expected an event listener");
    const receive = (_event, payload) => listener(payload);
    ipcRenderer.on("build:bridge-http-event", receive);
    return () => ipcRenderer.removeListener("build:bridge-http-event", receive);
  },
};
contextBridge.exposeInMainWorld("buildBridgeHttp", Object.freeze(facade));
