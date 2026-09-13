// @vitest-environment jsdom
// The two version gates (wire spec step 2.5), as the connection gate shows
// them: the adapter selection each greeting makes reaches views/gate.js, and a
// bridge nobody here speaks to takes #root until one arrives that does.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const App = { devices: [{ id: "d1", name: "studio", status: "online" }], session: { deviceId: "d1" }, gated: false };
const render = vi.fn();
const startFeed = vi.fn();
const stopFeed = vi.fn();
const paintDevicePicker = vi.fn();
const mintInstallCommand = vi.fn(async () => ({ install_command: "curl -fsSL https://getbuild.ing/i | sh", expires_in_s: 600 }));
let selected = null;

vi.mock("../src/api.js", () => ({
  lookupDevice: async () => ({}),
  approveDevice: async () => {},
  fetchDownloads: async () => ({}),
  mintInstallCommand: (...args) => mintInstallCommand(...args),
}));
vi.mock("../src/devices.js", () => ({
  refreshDevices: async () => App.devices,
  paintDevicePicker: (...args) => paintDevicePicker(...args),
  deviceName: (id) => App.devices.find((d) => d.id === id)?.name || null,
}));
vi.mock("../src/connection.js", () => ({
  openAppSession: async () => ({}),
  adoptSession: () => {},
  greetLiveBridge: () => {},
  setConn: () => {},
  onBridgeSelected: (fn) => {
    selected = fn;
  },
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "macos-arm64" }));
vi.mock("../src/app.js", () => ({ App, render: (...args) => render(...args) }));
vi.mock("../src/core/taskFeed.js", () => ({ startFeed: (...a) => startFeed(...a), stopFeed: (...a) => stopFeed(...a) }));
vi.mock("../src/core/cacheSync.js", () => ({ startCacheSync: () => {} }));
vi.mock("../src/core/inboxShell.js", () => ({ initInboxRail: () => {} }));
vi.mock("../src/core/toolbar.js", () => ({ initToolbar: () => {} }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));
const root = () => document.getElementById("root");

beforeEach(async () => {
  vi.clearAllMocks();
  vi.resetModules();
  selected = null;
  App.gated = false;
  App.updateAvailable = false;
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  await import("../src/views/gate.js");
});

describe("the version gates", () => {
  it("registers for the adapter selection when the module loads", () => {
    expect(typeof selected).toBe("function");
  });

  it("gates a bridge above every adapter as the app being behind, naming the device", async () => {
    selected({ unsupported: "app", version: "2.0.0" });
    await flush();
    expect(App.gated).toBe(true);
    expect(document.body.classList.contains("gated")).toBe(true);
    expect(stopFeed).toHaveBeenCalled();
    expect(root().querySelector("h1").textContent).toBe("This app is behind the bridge on studio");
    expect(root().textContent).toContain("2.0.0");
    expect(root().querySelector("#gate-reload")).toBe(null);
  });

  it("offers the reload only once the served-version watcher has something newer", async () => {
    App.updateAvailable = true;
    selected({ unsupported: "app", version: "2.0.0" });
    await flush();
    expect(root().querySelector("#gate-reload")).toBeTruthy();
  });

  it("gates a bridge below every adapter as the bridge needing updating, with the install line", async () => {
    selected({ unsupported: "bridge", version: "0.9.0" });
    await flush();
    expect(App.gated).toBe(true);
    expect(root().querySelector("h1").textContent).toBe("The bridge on studio needs updating");
    expect(mintInstallCommand).toHaveBeenCalledTimes(1);
    expect(root().textContent).toContain("curl -fsSL https://getbuild.ing/i | sh");
    expect(root().querySelector("#gate-install-copy")).toBeTruthy();
  });

  it("keeps the gate up when the install line cannot be minted", async () => {
    mintInstallCommand.mockRejectedValueOnce(new Error("invite only"));
    selected({ unsupported: "bridge", version: "0.9.0" });
    await flush();
    expect(root().querySelector("h1").textContent).toBe("The bridge on studio needs updating");
    expect(root().querySelector("#gate-install-copy")).toBe(null);
  });

  it("lets the app back in when a later greeting selects an adapter", async () => {
    selected({ unsupported: "bridge", version: "0.9.0" });
    await flush();
    selected({ major: 1, version: "1.1.0", range: ">=1.0.0 <2.0.0", create: () => ({}) });
    await flush();
    expect(App.gated).toBe(false);
    expect(document.body.classList.contains("gated")).toBe(false);
    expect(startFeed).toHaveBeenCalledTimes(1);
    expect(render).toHaveBeenCalledTimes(1);
  });

  it("does nothing for a supported selection when no version gate is up", async () => {
    selected({ major: 1, version: "1.0.0", range: ">=1.0.0 <2.0.0", create: () => ({}) });
    await flush();
    expect(App.gated).toBe(false);
    expect(startFeed).not.toHaveBeenCalled();
    expect(render).not.toHaveBeenCalled();
  });
});
