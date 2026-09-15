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

const { App, chooseCreationDevice, retireDevice, revokeDevice } = vi.hoisted(() => ({
  App: { devices: [], selectedDeviceId: null, viewDispose: null },
  chooseCreationDevice: vi.fn(),
  retireDevice: vi.fn(),
  revokeDevice: vi.fn(async () => {}),
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
  retireDevice: (...args) => retireDevice(...args),
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  openDeviceSettingsSession: async () => null,
  syncHome: () => {},
  goOffline: () => {},
  forgetHomeFollow: () => {},
  forgetSecurityStops: () => {},
  forgetRendezvousSockets: () => {},
  securityStopText: () => "",
}));
vi.mock("../src/api.js", () => ({
  revokeDevice: (...args) => revokeDevice(...args),
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
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));
const $ = (selector) => document.querySelector(selector);

let renderSettings;
let adoptDeviceSession;
let contextFor;
let resetDeviceContexts;
let setContextOffline;

beforeEach(async () => {
  vi.clearAllMocks();
  devices = [
    { id: "dev-1", name: "Laptop", fingerprint: "AAAABBBBCCCCDDDD", status: "online" },
    { id: "dev-2", name: "Studio", fingerprint: "EEEEFFFF00001111", status: "online" },
  ];
  App.devices = [];
  App.selectedDeviceId = null;
  App.viewDispose?.();
  App.viewDispose = null;
  document.body.innerHTML = bodyHtml;
  ({ renderSettings } = await import("../src/views/settings.js"));
  ({ adoptDeviceSession, contextFor, resetDeviceContexts, setContextOffline } = await import("../src/core/deviceContexts.js"));
  resetDeviceContexts();
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

  // The control is what the account picked, not what the pick currently
  // resolves to: showing the fallback made a pick that was merely away read as
  // a pick the user never made, and changing it back was impossible — the
  // select already said the other machine.
  it("shows the machine the account picked even while that machine is away", async () => {
    devices[0].status = "offline"; // Laptop, the picked one
    App.selectedDeviceId = "dev-1";
    await renderSettings();
    await flush();

    expect($("#creationdev").value).toBe("dev-1");
    // …and where the work is going meanwhile is said, not left to be guessed.
    expect($("#creationfallback").textContent).toBe("Laptop is offline; new work goes to Studio until it returns.");
  });

  it("says nothing under the control while the machine picked is the one taking the work", async () => {
    App.selectedDeviceId = "dev-1";
    await renderSettings();
    await flush();

    expect($("#creationfallback").textContent).toBe("");
  });

  it("says the work waits when the picked machine is away and no other is online", async () => {
    devices = devices.map((device) => ({ ...device, status: "offline" }));
    App.selectedDeviceId = "dev-2";
    await renderSettings();
    await flush();

    expect($("#creationdev").value).toBe("dev-2");
    expect($("#creationfallback").textContent).toBe("Studio is offline; new work waits until a device is back.");
  });

  // The note is about whichever machine the select is showing, so it is worked
  // out again every time that changes. Left painted once, it went on naming the
  // machine the reader had just picked away from.
  it("says where the work is going about the machine just picked, not the one before it", async () => {
    devices[0].status = "offline"; // Laptop, the picked one
    App.selectedDeviceId = "dev-1";
    await renderSettings();
    await flush();
    expect($("#creationfallback").textContent).toBe("Laptop is offline; new work goes to Studio until it returns.");

    const select = $("#creationdev");
    select.value = "dev-2";
    select.dispatchEvent(new Event("change", { bubbles: true }));

    expect($("#creationfallback").textContent).toBe("");
  });

  // Whether a machine is reachable is not news this page asks for — it is told,
  // the way every other surface that greys what a lost machine holds is told.
  it("says the machine picked has gone the moment it goes, and stops when it is back", async () => {
    App.selectedDeviceId = "dev-1";
    adoptDeviceSession({ deviceId: "dev-1", call, close: () => {} });
    await renderSettings();
    await flush();
    expect($("#creationfallback").textContent).toBe("");

    devices[0].status = "offline";
    setContextOffline("dev-1");
    expect($("#creationfallback").textContent).toBe("Laptop is offline; new work goes to Studio until it returns.");

    devices[0].status = "online";
    adoptDeviceSession({ deviceId: "dev-1", call, close: () => {} });
    expect($("#creationfallback").textContent).toBe("");
  });

  it("stops listening when the page goes", async () => {
    App.selectedDeviceId = "dev-1";
    adoptDeviceSession({ deviceId: "dev-1", call, close: () => {} });
    await renderSettings();
    await flush();

    App.viewDispose();
    App.viewDispose = null;
    devices[0].status = "offline";
    setContextOffline("dev-1");

    expect($("#creationfallback").textContent).toBe("");
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

// What the account owns and what a bridge owns are two different pages: the
// projects a machine holds, where it keeps them and how agents run there are
// facts of that machine, read on that machine's own page.
describe("Settings → what the account keeps", () => {
  it("asks the bridge nothing on the account page", async () => {
    adoptDeviceSession({ deviceId: "dev-1", call, close: () => {} });
    await renderSettings();
    await flush();

    const asked = call.mock.calls.map(([method]) => method);
    expect(asked).not.toContain("project.list");
    expect(asked).not.toContain("settings.get");
    expect(document.querySelector("#projlist")).toBeNull();
    expect(document.querySelector("[data-isolation=select]")).toBeNull();
    expect(document.querySelector("#defaultharness")).toBeNull();
    expect(document.querySelector("[data-triage-setting]")).toBeNull();
  });

  it("links each paired device to its own settings page", async () => {
    await renderSettings();
    await flush();

    const links = [...document.querySelectorAll("#devlist a.devsettings")];
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["#/device/dev-1/settings", "#/device/dev-2/settings"]);
    expect(links[0].textContent).toContain("Settings");
  });

  // Unpairing is the account's word and the app's: the api is told, and the
  // machine is let go of through the one function that lets a device go —
  // which drops the connection it was riding and tells every surface over it
  // (connection.js retireDevice), rather than only forgetting it.
  it("lets a revoked device go through the connection layer", async () => {
    await renderSettings();
    await flush();

    devices = devices.filter((device) => device.id !== "dev-1");
    document.querySelector("#devlist .revoke").click();
    await flush();

    expect(revokeDevice).toHaveBeenCalledWith("dev-1");
    expect(retireDevice).toHaveBeenCalledWith("dev-1");
  });
});
