// @vitest-environment jsdom
// Settings → Creation device: the one control for home. New projects and
// captures go to the machine named here; everything else — the inbox, the
// projects rail — is the whole account's. Picking a machine connects nothing
// and moves nothing: it is a preference, written through the one function that
// owns it (connection.js chooseCreationDevice).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const CATALOG = {
  default_provider: "claude",
  providers: [{ id: "claude", label: "Claude Code", models: [], efforts: [], creatable: true }],
};

const { App, chooseCreationDevice } = vi.hoisted(() => ({
  App: { call: null, devices: [], selectedDeviceId: null },
  chooseCreationDevice: vi.fn(),
}));

let devices = [];

const call = vi.fn(async (method) => {
  if (method === "project.list") return { projects: [] };
  if (method === "settings.get") return { projects_dir: "~/code", default_harness: "claude" };
  if (method === "models.list") return CATALOG;
  return {};
});

vi.mock("../src/app.js", () => ({ App, go: vi.fn() }));
vi.mock("../src/connection.js", () => ({
  chooseCreationDevice: (...args) => chooseCreationDevice(...args),
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  openDeviceSettingsSession: async () => null,
  syncHome: () => {},
  forgetHomeFollow: () => {},
  setConn: () => {},
  CONNECTION_STATUS: {},
}));
vi.mock("../src/api.js", () => ({
  revokeDevice: async () => {},
  fetchDownloads: async () => ({ platforms: [], install_command: "" }),
  mintInstallCommand: async () => ({ install_command: "", expires_in_s: 600 }),
}));
// The account list is what the select offers, so reading it is what fills it.
vi.mock("../src/devices.js", () => ({
  refreshDevices: async () => {
    App.devices = devices;
    return devices;
  },
}));
vi.mock("../src/push.js", () => ({
  pushState: async () => "unsupported",
  enablePush: async () => {},
  disablePush: async () => {},
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "linux-x86_64" }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser: () => {} }));
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: () => {} }));
vi.mock("../src/sheets/setRemote.js", () => ({ openSetRemote: () => {} }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));
const $ = (selector) => document.querySelector(selector);

let renderSettings;

beforeEach(async () => {
  vi.clearAllMocks();
  devices = [
    { id: "dev-1", name: "Laptop", fingerprint: "AAAABBBBCCCCDDDD", status: "online" },
    { id: "dev-2", name: "Studio", fingerprint: "EEEEFFFF00001111", status: "online" },
  ];
  App.call = (...args) => call(...args);
  App.devices = [];
  App.selectedDeviceId = null;
  document.body.innerHTML = bodyHtml;
  ({ renderSettings } = await import("../src/views/settings.js"));
});

describe("Settings → Creation device", () => {
  it("offers the account's devices by name, with the one creation goes to shown", async () => {
    App.selectedDeviceId = "dev-2";
    await renderSettings();
    await flush();

    const select = $("#creationdev");
    expect([...select.options].map((option) => option.textContent)).toEqual(["Laptop", "Studio"]);
    expect(select.value).toBe("dev-2");
  });

  it("shows the device creation falls back to while nothing is picked", async () => {
    await renderSettings();
    await flush();

    expect($("#creationdev").value).toBe("dev-1");
  });

  it("writes the pick through the one function that owns it", async () => {
    await renderSettings();
    await flush();

    const select = $("#creationdev");
    select.value = "dev-2";
    select.dispatchEvent(new Event("change", { bubbles: true }));

    expect(chooseCreationDevice).toHaveBeenCalledTimes(1);
    expect(chooseCreationDevice).toHaveBeenCalledWith("dev-2");
  });

  it("says what the choice is for, and what it is not for", async () => {
    await renderSettings();
    await flush();

    const panel = $("#creationdev").closest(".panel");
    expect(panel.textContent).toContain("New projects and captures go to");
    expect(panel.textContent).toContain("every device");
  });

  it("says so plainly on an account with no devices yet", async () => {
    devices = [];
    await renderSettings();
    await flush();

    expect($("#creationdev").textContent).toContain("No devices yet");
    expect($("#creationdev").disabled).toBe(true);
  });
});
