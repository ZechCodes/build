// @vitest-environment jsdom
// @vitest-environment-options {"url":"https://127.0.0.1:9443/app/"}
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "../src/appState.js";
import { createDeviceConnectionAttempts } from "../src/core/deviceConnectionAttempts.js";
import { contextFor, onDeviceStateChanged, resetDeviceContexts } from "../src/core/deviceContexts.js";
import * as connections from "../src/core/httpConnections.js";
import { openHttpDeviceConnection } from "../src/core/httpDeviceConnection.js";
import { uiAddress } from "../src/core/localUiState.js";
import * as uiStore from "../src/core/localUiStore.js";

const accountId = "10000000-0000-4000-8000-000000000056";
const deviceId = "studio";
const origin = "https://127.0.0.1:8443";
const terminalAddress = uiAddress({ entityId: accountId, view: "account", kind: "projected-http-terminal" });
const opening = {
  session_id: "session", opening_generation: "opening",
  greeting: { api_version: "3.24.0", bridge_epoch: "epoch", capabilities: ["term.websocket"] },
  transport: {
    resource_manifest_version: 1, operation_ids: [], terminal: "webrtc", durable_events: false,
    terminal_websocket: { version: 1, protocol: "build.term.v1" },
  },
};

function serveOpening() {
  const endpoint = {
    identity: { deviceId, origin },
    open: async () => opening,
    openTerminal: vi.fn(),
    async subscribe(_opening, deliver) {
      await deliver({ event: "hello", data: {
        durable: false, recovery: "current_state_refresh", bridge_epoch: "epoch", opening_generation: "opening",
      } });
      return { binding: "stream", close: vi.fn() };
    },
    close: vi.fn(),
  };
  vi.spyOn(connections, "httpEndpointFor").mockResolvedValue(endpoint);
}

function attemptConnection() {
  const owner = createDeviceConnectionAttempts().forDevice(deviceId);
  const selection = connections.deviceTransportFor(deviceId);
  const greet = vi.fn();
  let openingResult;
  const result = owner.connect((attempt) => {
    openingResult = openHttpDeviceConnection(deviceId, attempt, { selection, greet, onAdopt: vi.fn() });
    return openingResult;
  });
  return { selection, greet, openingResult, result, cancel: () => owner.cancel() };
}

beforeEach(async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  App.devices = [{ id: deviceId, transport: {
    source: "hosted", rtc_allowed: false, origin, reason: null, assignment_revision: 1,
  } }];
  connections.configureHttpManifest({ version: 1, operations: [] });
  await connections.restoreHttpConnections();
});

afterEach(async () => {
  resetDeviceContexts();
  connections.resetHttpConnections();
  await uiStore.wipeUiRecords();
  vi.restoreAllMocks();
  delete window.buildBridgeHttp;
  document.head.replaceChildren();
  App.devices = [];
});

it("publishes WSS from the committed cache at the first observation of an adopted opening", async () => {
  serveOpening();
  expect(connections.cachedTerminalTransport(deviceId)).toBe("unknown");
  const observed = [];
  const stop = onDeviceStateChanged(() => {
    if (contextFor(deviceId)?.session) observed.push(connections.cachedTerminalTransport(deviceId));
  });
  try {
    const { result, selection } = attemptConnection();
    await result;
    expect(observed[0]).toBe("wss");
    expect((await uiStore.readUiRecord(terminalAddress))?.value).toEqual([[deviceId, { key: selection.key, mode: "wss" }]]);
  } finally {
    stop();
  }
});

it("does not adopt an opening cancelled during the compatibility write", async () => {
  serveOpening();
  const writeStarted = Promise.withResolvers();
  const releaseWrite = Promise.withResolvers();
  const update = uiStore.updateUiRecord;
  vi.spyOn(uiStore, "updateUiRecord").mockImplementation(async (address, ...args) => {
    if (address.kind === terminalAddress.kind) {
      writeStarted.resolve();
      await releaseWrite.promise;
    }
    return update(address, ...args);
  });
  const { result, openingResult, greet, cancel } = attemptConnection();
  const settled = result.catch((error) => error);
  try {
    await expect(Promise.race([writeStarted.promise.then(() => "writing"), settled])).resolves.toBe("writing");
    expect(contextFor(deviceId)).toBeNull();
    cancel();
  } finally {
    releaseWrite.resolve();
  }
  await expect(settled).resolves.toMatchObject({ message: "connection attempt cancelled" });
  await expect(openingResult).rejects.toThrow("HTTP attempt superseded");
  expect(greet).not.toHaveBeenCalled();
  expect(contextFor(deviceId)).toBeNull();
  expect(await uiStore.readUiRecord(terminalAddress)).toBeUndefined();
});

it("commits update guidance when the native preload cannot open the HTTPS endpoint", async () => {
  window.buildBridgeHttp = { discover: vi.fn() };
  expect(connections.cachedTerminalTransport(deviceId)).toBe("unknown");
  const { result, selection } = attemptConnection();
  await expect(result).rejects.toMatchObject({ code: "desktop_update_required" });
  expect(contextFor(deviceId)).toBeNull();
  expect(connections.cachedTerminalTransport(deviceId)).toBe("update_required");
  expect((await uiStore.readUiRecord(terminalAddress))?.value).toEqual([
    [deviceId, { key: selection.key, mode: "update_required" }],
  ]);
});
