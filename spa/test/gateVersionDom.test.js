// @vitest-environment jsdom
// The two version gates (wire spec step 2.5), as the connection gate shows
// them: every machine's greeting settles what its bridge speaks, and the gate
// re-reads the account whenever that changes. A bridge nobody here speaks to is
// a machine that cannot answer — so it takes #root only while no other machine
// can, and a later greeting that does select an adapter hands the app back.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { fakeSession } from "./deviceSessionFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const App = { devices: [], gated: false, updateAvailable: false, viewingContext: {} };
const render = vi.fn();
const unmountView = vi.fn();
const startFeed = vi.fn();
const stopFeed = vi.fn();
const dropFeedDevice = vi.fn();
const paintDevicePicker = vi.fn();
const mintInstallCommand = vi.fn(async () => ({
  install_command: "curl -fsSL https://getbuild.ing/i | sh",
  expires_in_s: 600,
}));

vi.mock("../src/api.js", () => ({
  lookupDevice: async () => ({}),
  approveDevice: async () => {},
  fetchDownloads: async () => ({}),
  mintInstallCommand: (...args) => mintInstallCommand(...args),
}));
vi.mock("../src/devices.js", () => ({
  refreshDevices: async () => App.devices,
  paintDevicePicker: (...args) => paintDevicePicker(...args),
}));
vi.mock("../src/connection.js", () => ({
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  openDeviceSettingsSession: async () => ({}),
  chooseCreationDevice: () => {},
  retireDevice: () => {},
  goOffline: () => {},
  syncHome: () => {},
  forgetHomeFollow: () => {},
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "macos-arm64" }));
vi.mock("../src/app.js", () => ({
  App,
  render: (...args) => render(...args),
  unmountView: (...args) => unmountView(...args),
}));
vi.mock("../src/core/taskFeed.js", () => ({
  startFeed: (...args) => startFeed(...args),
  stopFeed: (...args) => stopFeed(...args),
  dropFeedDevice: (...args) => dropFeedDevice(...args),
}));
vi.mock("../src/core/cacheSync.js", () => ({ startCacheSync: () => {} }));
vi.mock("../src/core/inboxShell.js", () => ({ initInboxRail: () => {} }));
vi.mock("../src/core/toolbar.js", () => ({ initToolbar: () => {} }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));
const root = () => document.getElementById("root");

let adoptBridgeSelection;
let adoptDeviceSession;
let contextFor;

/** A paired machine whose bridge has just greeted, with what its greeting
 *  settled: `unsupported` names the side that is behind, and null is a bridge
 *  an adapter here claims. */
const greeted = (deviceId, selection) => {
  adoptDeviceSession(fakeSession(deviceId));
  adoptBridgeSelection(contextFor(deviceId), selection);
};

beforeEach(async () => {
  vi.clearAllMocks();
  vi.resetModules();
  App.devices = [{ id: "d1", name: "studio", status: "online" }];
  App.gated = false;
  App.updateAvailable = false;
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  ({ adoptBridgeSelection, adoptDeviceSession, contextFor } = await import("../src/core/deviceContexts.js"));
  const gate = await import("../src/views/gate.js");
  // The app is entered: the gate is listening for the account running out of
  // machines that can answer, which is what stands a version gate up.
  gate.holdAppWhileNoDeviceAnswers();
});

describe("the version gates", () => {
  it("gates a bridge above every adapter as the app being behind, naming the device", async () => {
    greeted("d1", { unsupported: "app", version: "2.0.0" });
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
    greeted("d1", { unsupported: "app", version: "2.0.0" });
    await flush();

    expect(root().querySelector("#gate-reload")).toBeTruthy();
  });

  it("gates a bridge below every adapter as the bridge needing updating, with the install line", async () => {
    greeted("d1", { unsupported: "bridge", version: "0.9.0" });
    await flush();

    expect(App.gated).toBe(true);
    expect(root().querySelector("h1").textContent).toBe("The bridge on studio needs updating");
    expect(mintInstallCommand).toHaveBeenCalledTimes(1);
    expect(root().textContent).toContain("curl -fsSL https://getbuild.ing/i | sh");
    expect(root().querySelector("#gate-install-copy")).toBeTruthy();
  });

  it("keeps the gate up when the install line cannot be minted", async () => {
    mintInstallCommand.mockRejectedValueOnce(new Error("invite only"));
    greeted("d1", { unsupported: "bridge", version: "0.9.0" });
    await flush();

    expect(root().querySelector("h1").textContent).toBe("The bridge on studio needs updating");
    expect(root().querySelector("#gate-install-copy")).toBe(null);
  });

  it("lets the app back in when a later greeting selects an adapter", async () => {
    greeted("d1", { unsupported: "bridge", version: "0.9.0" });
    await flush();

    // The machine's bridge was updated: it re-greets, and this greeting is
    // claimed by an adapter this build carries.
    adoptBridgeSelection(contextFor("d1"), { major: 1, version: "1.1.0", unsupported: null });
    await flush();

    expect(App.gated).toBe(false);
    expect(document.body.classList.contains("gated")).toBe(false);
    expect(startFeed).toHaveBeenCalledTimes(1);
    expect(render).toHaveBeenCalledTimes(1);
  });

  // One machine out of two speaking a major nothing here claims is that
  // machine's business: its rows grey and its surfaces say so, while the
  // account still has a machine to stand on.
  it("leaves the app alone while another machine can still answer", async () => {
    App.devices = [
      { id: "d1", name: "studio", status: "online" },
      { id: "d2", name: "laptop", status: "online" },
    ];
    greeted("d2", { major: 1, version: "1.1.0", unsupported: null });

    greeted("d1", { unsupported: "app", version: "2.0.0" });
    await flush();

    expect(App.gated).toBe(false);
    expect(document.body.classList.contains("gated")).toBe(false);
    expect(root().querySelector("h1")).toBe(null);
  });

  it("does nothing for a supported selection when no version gate is up", async () => {
    greeted("d1", { major: 1, version: "1.0.0", unsupported: null });
    await flush();

    expect(App.gated).toBe(false);
    expect(startFeed).not.toHaveBeenCalled();
    expect(render).not.toHaveBeenCalled();
  });
});
