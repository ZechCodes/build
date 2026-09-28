// @vitest-environment jsdom
// Settings → Creation device: the one control for home. New projects and
// captures go to the machine named here; everything else — the inbox, the
// projects rail — is the whole account's. Picking a machine connects nothing
// and moves nothing: it is a preference, written through the one function that
// owns it (connection.js chooseCreationDevice).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const CATALOG = {
  default_provider: "claude",
  providers: [{ id: "claude", label: "Claude Code", models: [], efforts: [], creatable: true }],
};

const { App, chooseCreationDevice, retireDevice, revokeDevice, deviceListeners } = vi.hoisted(() => ({
  App: { devices: [], selectedDeviceId: null, viewDispose: null },
  chooseCreationDevice: vi.fn(),
  retireDevice: vi.fn(),
  revokeDevice: vi.fn(async () => {}),
  deviceListeners: new Set(),
}));

let devices = [];
let cachedDevices = [];
let pullDevices = async () => devices;

const call = vi.fn(async (method) => {
  if (method === "project.list") return { projects: [] };
  if (method === "settings.get") return { projects_dir: "~/code", default_harness: "claude" };
  if (method === "models.list") return CATALOG;
  return {};
});

vi.mock("../src/app.js", () => ({ App, go: vi.fn() }));
vi.mock("../src/connection.js", () => ({
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {}, deviceRecoverySnapshot: () => [], onDeviceRecoveryChanged: () => () => {},
  chooseCreationDevice: (...args) => chooseCreationDevice(...args),
  retireDevice: (...args) => retireDevice(...args),
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  openDeviceSettingsSession: async () => null,
  syncHome: () => {},
  goOffline: () => {},
  deviceWentAway: () => {},
  connectDevice: async () => null,
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
    App.devices = await pullDevices();
    for (const listener of [...deviceListeners]) listener(App.devices);
    return App.devices;
  },
  readCachedDevices: async () => {
    App.devices = cachedDevices;
    for (const listener of [...deviceListeners]) listener(App.devices);
    return App.devices;
  },
  onDevicesChanged: (listener) => {
    deviceListeners.add(listener);
    return () => deviceListeners.delete(listener);
  },
}));
let pushReading = async () => "unsupported";
vi.mock("../src/push.js", () => ({
  pushState: () => pushReading(),
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
  pushReading = async () => "unsupported";
  const { wipeCache } = await import("../src/core/localCache.js");
  await wipeCache();
  const { wipeUiRecords } = await import("../src/core/localUiStore.js");
  await wipeUiRecords();
  vi.clearAllMocks();
  deviceListeners.clear();
  devices = [
    { id: "dev-1", name: "Laptop", fingerprint: "AAAABBBBCCCCDDDD", status: "online" },
    { id: "dev-2", name: "Studio", fingerprint: "EEEEFFFF00001111", status: "online" },
  ];
  cachedDevices = devices;
  pullDevices = async () => devices;
  App.devices = [];
  App.selectedDeviceId = null;
  App.viewDispose?.();
  App.viewDispose = null;
  localStorage.clear();
  document.body.innerHTML = bodyHtml;
  ({ renderSettings } = await import("../src/views/settings.js"));
  ({ adoptDeviceSession, contextFor, resetDeviceContexts, setContextOffline } = await import("../src/core/deviceContexts.js"));
  resetDeviceContexts();
});

it("paints the cached notification state before the browser answers, then writes the fresh reading", async () => {
  const { readUiRecord, writeUiRecord } = await import("../src/core/localUiStore.js");
  const { uiAddress } = await import("../src/core/localUiState.js");
  const address = uiAddress({ view: "settings", kind: "push" });
  await writeUiRecord(address, { state: "enabled", permission: "granted", subscribed: true });
  let answer;
  pushReading = () => new Promise((resolve) => { answer = resolve; });
  const rendering = renderSettings();
  await vi.waitFor(() => expect($("#pushtoggle")?.textContent).toBe("Turn off notifications"));
  answer("denied");
  await rendering;
  await vi.waitFor(() => expect($("#pushtoggle").textContent).toBe("Blocked"));
  expect((await readUiRecord(address)).value).toMatchObject({ state: "denied", subscribed: false });
});

it("wires a cached enabled notification toggle while the fresh state is pending", async () => {
  const { writeUiRecord } = await import("../src/core/localUiStore.js");
  const { uiAddress } = await import("../src/core/localUiState.js");
  await writeUiRecord(uiAddress({ view: "settings", kind: "push" }), { state: "enabled", permission: "granted", subscribed: true });
  const read = vi.fn(() => new Promise(() => {}));
  pushReading = read;
  await renderSettings();
  await vi.waitFor(() => expect($("#pushtoggle")?.textContent).toBe("Turn off notifications"));
  $("#pushtoggle").click();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
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

// One defaults panel on this page: what every new agent this browser creates
// starts on. What a PROJECT agent starts on is the machine's, so it is asked on
// that machine's own page (core/projectAgentSetting.js) and never here.
describe("Settings → agent defaults", () => {
  it("keeps the project agent off the browser's page", async () => {
    adoptDeviceSession({ deviceId: "dev-1", call, close: () => {} });
    await renderSettings();
    await flush();

    expect([...document.querySelectorAll("[data-harness-defaults]")].map((panel) => panel.dataset.harnessDefaults))
      .toEqual(["def"]);
    expect(document.getElementById("projprovider")).toBeNull();
    expect(document.getElementById("projectagentharness")).toBeNull();
    expect(document.querySelector("#root").textContent).not.toContain("Project agent");
  });
});

// What the account owns and what a bridge owns are two different pages: the
// projects a machine holds, where it keeps them and how agents run there are
// facts of that machine, read on that machine's own page.
describe("Settings → what the account keeps", () => {
  it("paints cached devices and the creation choice without waiting for the account pull", async () => {
    cachedDevices = [{ id: "dev-cache", name: "Cached laptop", fingerprint: "AAAABBBBCCCCDDDD", status: "offline" }];
    pullDevices = () => new Promise(() => {});

    await renderSettings();
    await flush();

    expect($("#devlist").textContent).toContain("Cached laptop");
    expect([...$("#creationdev").options].map((option) => option.textContent)).toEqual(["Cached laptop"]);
  });

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

// The connection dump, on the page it is needed on. deploy/OPS.md asks people
// to run `buildConnectionDiagnostics()` in a console before reloading a tab
// that lost its connection — which is no use on the phone where the reconnects
// happen. Settings reads the same history through the module.
describe("Settings → Diagnostics", () => {
  it("lists the recorded events, newest first, naming the machine", async () => {
    const { recordConnectionDiagnostic, clearConnectionDiagnosticHistory } =
      await import("../src/core/connectionDiagnostics.js");
    clearConnectionDiagnosticHistory();
    recordConnectionDiagnostic("dev-1:sess-1", "negotiating", { phase: "initial" });
    recordConnectionDiagnostic("dev-2:sess-2", "restart-failed", { reason: "timeout" });

    await renderSettings();
    await flush();

    const rows = [...document.querySelectorAll(".diagrow")];
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("restart-failed");
    expect(rows[0].textContent).toContain("Studio");
    expect(rows[0].textContent).toContain("reason=timeout");
    expect(rows[1].textContent).toContain("Laptop");
    clearConnectionDiagnosticHistory();
  });

  it("takes its poll away with the page, so nothing ticks off screen", async () => {
    const armed = vi.spyOn(globalThis, "setInterval");
    const dropped = vi.spyOn(globalThis, "clearInterval");

    await renderSettings();
    await flush();
    const ticker = armed.mock.results.at(-1)?.value;
    App.viewDispose?.();
    App.viewDispose = null;

    expect(ticker).toBeDefined();
    expect(dropped.mock.calls.flat()).toContain(ticker);
    armed.mockRestore();
    dropped.mockRestore();
  });
});

// Which build this tab is running, under the dump it is read beside. It sits
// in Settings because that is where somebody with a question about this tab is
// already standing — and on the night it mattered there was nowhere to look.
describe("Settings → the build it is running", () => {
  it("shows the line under Diagnostics, with the whole version in its title", async () => {
    await renderSettings();
    await flush();

    const line = document.querySelector("#buildversion");
    expect(line).not.toBeNull();
    expect(line.textContent.replace(/\s+/g, " ").trim()).toBe("Build dev Copy");
    // The suites run an unstamped bundle, so `dev` is the honest answer here.
    expect(document.querySelector("#buildversionsha").getAttribute("title")).toBe("dev");
    expect(document.querySelector("#diagnostics").compareDocumentPosition(line))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("says whether the local cache is answering, beside the build line (#169)", async () => {
    await renderSettings();
    await flush();

    const line = document.querySelector("#cachehealth");
    expect(line).not.toBeNull();
    expect(line.textContent).toContain("Local cache");
    expect(document.querySelector("#cachehealthtext").textContent).toMatch(/^(Working\.|This browser has no IndexedDB)/);
    expect(document.querySelector("#buildversion").compareDocumentPosition(line))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("copies the version when the button is pressed", async () => {
    const writeText = vi.fn(async () => {});
    const had = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });

    await renderSettings();
    await flush();
    document.querySelector("#buildversioncopy").click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("dev"));

    if (had) Object.defineProperty(navigator, "clipboard", had);
    else delete navigator.clipboard;
  });
});
