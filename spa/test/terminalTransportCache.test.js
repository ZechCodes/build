// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";

const status = vi.hoisted(() => ({ current: "disconnected", listener: null }));
vi.mock("../src/terminal/manager.js", () => ({
  terminalDeviceId: () => "device",
  terminalManager: () => ({}),
  followTerminalDevice: () => {},
  resetTerminalManager: () => {},
  provideTerminalSessions: () => {},
  subscribeTerminalStatus: (listener) => {
    status.listener = listener;
    listener(status.current);
    return () => { status.listener = null; };
  },
}));

const accountId = "10000000-0000-4000-8000-000000000001";
const device = { deviceId: "device", origin: "https://bridge.example", endpointEpoch: "epoch", familyId: "family", selected: "http" };

async function coldPane(terminalMode) {
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  const { writeUiRecord } = await import("../src/core/localUiStore.js");
  const { uiAddress } = await import("../src/core/localUiState.js");
  await writeUiRecord(uiAddress({ entityId: accountId, view: "account", kind: "http-connections" }), {
    accountId, connections: [{ ...device, terminalMode }],
  });
  vi.resetModules();
  const { restoreHttpConnections } = await import("../src/core/httpConnections.js");
  const { attachConnectionOverlay, attachTransportLabel } = await import("../src/core/surfaceTabs.js");
  await restoreHttpConnections();
  const host = document.createElement("div");
  const label = attachTransportLabel(host, device.deviceId);
  const overlay = attachConnectionOverlay(host, device.deviceId);
  return {
    host, label: () => host.querySelector(".term-transport")?.textContent,
    hideChip: () => host.querySelector(".term-conn").hidden,
    dispose() { label.dispose(); overlay.dispose(); },
  };
}

afterEach(() => {
  status.current = "disconnected";
  status.listener = null;
  document.head.replaceChildren();
});

it("paints cached HTTP fallback on a fresh module before the first Opening", async () => {
  const pane = await coldPane("webrtc");
  expect(pane.label()).toBe("Terminal over WebRTC");
  status.listener("connected");
  expect(pane.label()).toBe("Terminal over WebRTC");
  pane.dispose();
});

it("suppresses the legacy reconnect chip on a cold WSS device", async () => {
  const pane = await coldPane("wss");
  expect(pane.label()).toBeUndefined();
  expect(pane.hideChip()).toBe(true);
  status.listener("connected");
  expect(pane.hideChip()).toBe(true);
  pane.dispose();
});

it("uses the saved HTTPS preference when the terminal mode has never been cached", async () => {
  const pane = await coldPane(undefined);
  expect(pane.label()).toBe("Terminal over WebRTC");
  pane.dispose();
});

it.each([
  [null, "HTTPS terminal support has not been checked yet."],
  ["wss", "Terminal over HTTPS"],
  ["update_required", "Update the Build bridge or desktop app to use HTTPS terminals."],
])("paints the projected terminal compatibility on a cold offline mount (%s)", async (mode, text) => {
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  const { App } = await import("../src/appState.js");
  const { attachTransportLabel } = await import("../src/core/surfaceTabs.js");
  const { rememberHttpTerminalMode, resetHttpConnections, restoreHttpConnections } = await import("../src/core/httpConnections.js");
  App.devices = [{ id: device.deviceId, transport: {
    source: "hosted", rtc_allowed: false, origin: "https://127.0.0.1:8443", reason: null, assignment_revision: 11,
  } }];
  await restoreHttpConnections();
  if (mode) await rememberHttpTerminalMode(device.deviceId, mode, () => true);
  resetHttpConnections();
  await restoreHttpConnections();
  const host = document.createElement("div");
  const label = attachTransportLabel(host, device.deviceId);
  expect(host.textContent).toBe(text);
  label.dispose();
  resetHttpConnections();
  App.devices = [];
});
