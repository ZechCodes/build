// @vitest-environment jsdom
// Settings → Downloads: the same block the first-run screen shows, for the
// second machine and for updating this one. It sits above Devices & keys,
// which is where the empty-device sentence now sends people.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { asset, downloadsPayload, mintedCommand } from "./downloadsFixture.js";
import { wipeCache } from "../src/core/localCache.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const DOWNLOADS = downloadsPayload({
  platforms: [{ key: "linux-x86_64", label: "Linux · x86_64", url: asset("linux-x86_64") }],
});

let devices = [];
let downloads = async () => DOWNLOADS;
const { App, deviceListeners } = vi.hoisted(() => ({
  App: { devices: [], selectedDeviceId: null },
  deviceListeners: new Set(),
}));
const fetchDownloads = vi.fn(() => downloads());
const mintInstallCommand = vi.fn(async () => ({ install_command: mintedCommand(), expires_in_s: 600 }));

// No device context is registered here, so the creation device's catalog is the
// empty one — which is all this page's downloads block cares about.
vi.mock("../src/app.js", () => ({
  App,
  go: vi.fn(),
}));
vi.mock("../src/api.js", () => ({
  revokeDevice: async () => {},
  fetchDownloads: (...args) => fetchDownloads(...args),
  mintInstallCommand: (...args) => mintInstallCommand(...args),
}));
vi.mock("../src/devices.js", () => ({
  refreshDevices: async () => {
    App.devices = devices;
    for (const listener of [...deviceListeners]) listener(devices);
    return devices;
  },
  readCachedDevices: async () => {
    App.devices = devices;
    for (const listener of [...deviceListeners]) listener(devices);
    return devices;
  },
  onDevicesChanged: (listener) => {
    deviceListeners.add(listener);
    return () => deviceListeners.delete(listener);
  },
}));
vi.mock("../src/push.js", () => ({
  pushState: async () => "unsupported",
  enablePush: async () => {},
  disablePush: async () => {},
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "linux-x86_64" }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 30));

let renderSettings;

beforeEach(async () => {
  await wipeCache();
  vi.clearAllMocks();
  deviceListeners.clear();
  devices = [];
  downloads = async () => DOWNLOADS;
  App.devices = [];
  document.body.innerHTML = bodyHtml;
  ({ renderSettings } = await import("../src/views/settings.js"));
});

describe("Settings → Downloads", () => {
  it("mounts the same downloads block the gate shows, for this browser's platform", async () => {
    await renderSettings();
    await flush();
    expect(document.querySelector("#downloads a.btn.primary").getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
  });

  it("sends an account with no devices to that panel rather than to a command", async () => {
    await renderSettings();
    await flush();
    expect(document.getElementById("devlist").textContent).toContain(
      "Install the bridge above, then add it with its pairing code",
    );
    expect(document.getElementById("adddev")).toBeTruthy();
  });

  it("finishes without waiting on the downloads round trip", async () => {
    // The rule at the boundary the caller sees: renderAccount does work after
    // `await renderSettings()`, so the promise itself must not carry the api's
    // clock. Race it against a never-settling /app/downloads.
    downloads = () => new Promise(() => {});
    await expect(
      Promise.race([renderSettings().then(() => "settled"), flush().then(() => "stalled")]),
    ).resolves.toBe("settled");
    expect(typeof document.getElementById("adddev").onclick).toBe("function");
    expect(typeof document.getElementById("creationdev").onchange).toBe("function");
    expect(document.getElementById("downloads").textContent).toContain("loading…");
  });

  it("keeps the devices panel intact when the downloads route refuses", async () => {
    downloads = async () => {
      throw new Error("invite only");
    };
    devices = [{ id: "d1", name: "studio", fingerprint: "AAAABBBBCCCCDDDDEEEE", status: "online" }];
    await renderSettings();
    await vi.waitFor(() => {
      expect(document.getElementById("downloadserr").textContent).toBe("invite only");
      expect(document.getElementById("devlist").textContent).toContain("studio");
    });
    expect(document.querySelector("#devlist .revoke")).toBeTruthy();
  });
});
