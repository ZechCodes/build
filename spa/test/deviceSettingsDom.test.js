// @vitest-environment jsdom
// One machine's own settings page: everything the bridge owns — its projects,
// where they are kept, and how agents run there — read and written over the
// connection this page opens to that machine, and nothing else.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { App, openSession, openBrowser, openNewRepo, openSetRemote, refreshModelCatalog, contextFor, refreshFeed, fetchDevices, renameDevice, revokeDevice, retireDevice, confirmAction } =
  vi.hoisted(() => ({
    App: { devices: [], viewDispose: null },
    openSession: vi.fn(),
    openBrowser: vi.fn(),
    openNewRepo: vi.fn(),
    openSetRemote: vi.fn(),
    refreshModelCatalog: vi.fn(async () => ({})),
    contextFor: vi.fn(),
    refreshFeed: vi.fn(async () => []),
    fetchDevices: vi.fn(),
    renameDevice: vi.fn(),
    revokeDevice: vi.fn(),
    retireDevice: vi.fn(),
    confirmAction: vi.fn(),
  }));
vi.mock("../src/app.js", () => ({ App, render: () => {} }));
vi.mock("../src/connection.js", () => ({
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {}, deviceRecoverySnapshot: () => [], onDeviceRecoveryChanged: () => () => {},
  chooseCreationDevice: () => {},
  retireDevice: (...args) => retireDevice(...args),
  openDeviceSettingsSession: openSession,
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncHome: () => {},
  goOffline: () => {},
  deviceWentAway: () => {},
  connectDevice: async () => null,
  forgetHomeFollow: () => {},
  forgetSecurityStops: () => {},
  forgetRendezvousSockets: () => {},
  securityStopText: () => "",
}));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo }));
vi.mock("../src/sheets/setRemote.js", () => ({ openSetRemote }));
vi.mock("../src/api.js", () => ({ fetchDevices, renameDevice, revokeDevice }));
vi.mock("../src/core/confirm.js", () => ({ confirmAction }));
// The page reads this machine's own context for one thing only: whether its
// bridge speaks an API major this tab can read. The rest of the module is named
// because core/deviceNotice.js — which words that answer — imports it.
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor,
  canAnswer: () => false,
  onDeviceStateChanged: () => () => {},
  routeContext: () => null,
}));
vi.mock("../src/core/taskFeed.js", () => ({ refreshFeed }));
import { renderDeviceSettings } from "../src/views/deviceSettings.js";
import { paintDevicePicker, readCachedDevices } from "../src/devices.js";
import { paintBridgeUpdateMark } from "../src/core/inboxShell.js";
import { DEVICES_ADDRESS, readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { deviceProjectsAddress, deviceSettingsAddress } from "../src/core/settingsRecords.js";
import { bridgeUpdateAddress } from "../src/core/bridgeUpdates.js";
import { dispatchChangeEvent } from "../src/core/changeEvents.js";

const CATALOG = {
  default_provider: "claude",
  providers: [
    { id: "claude", label: "Claude Code", models: [], efforts: [] },
    { id: "codex", label: "Codex", models: [], efforts: [] },
  ],
};
const SETTINGS = {
  projects_dir: "/projects",
  default_harness: "claude",
  agent_modes: { claude: "tui", codex: "headless" },
  isolation: "worktree",
  isolation_available: { rift: true, reason: null },
};
const PROJECTS = [
  {
    project_id: "p1",
    name: "relaydb",
    path: "/projects/relaydb",
    base_branch: "main",
    isolation_effective: "rift",
    remote: "git@github.com:org/relaydb.git",
    is_git: true,
  },
];
const UPDATE = {
  running_version: "0.2.0", platform: "linux-x86_64", development_build: false,
  latest_release: { version: "0.3.0", tag: "bridge-v0.3.0", published_at: "2026-09-23T12:00:00Z" },
  last_checked_at: "2026-09-23T12:00:00Z", state: "available", last_error: null,
  update_available: true, can_install: true,
};

let session;
beforeEach(async () => {
  vi.clearAllMocks();
  await wipeCache();
  document.body.innerHTML = '<main id="root"></main><div id="scrim"><div id="sheet"></div></div>';
  App.devices = [{ id: "other", name: "Other machine", status: "online" }];
  App.accountEpoch = 0;
  await writeCached(DEVICES_ADDRESS, App.devices);
  await readCachedDevices();
  App.route = { name: "device", id: "other" };
  session = {
    deviceId: "other",
    call: vi.fn(async (method) => {
      if (method === "project.list") return { projects: PROJECTS };
      if (method === "models.list") return CATALOG;
      if (method === "bridge.update_status") return UPDATE;
      if (method === "bridge.check_update") return UPDATE;
      if (method === "bridge.install_update") return UPDATE;
      return { ...SETTINGS };
    }),
    close: vi.fn(),
  };
  openSession.mockResolvedValue(session);
  refreshModelCatalog.mockResolvedValue({});
  confirmAction.mockResolvedValue(false);
  contextFor.mockImplementation((deviceId) => (deviceId === "other" ? { refreshModelCatalog } : null));
});
afterEach(() => App.viewDispose?.());

describe("the machine's own panels", () => {
  it("shows bridge version and sends both install choices to this device", async () => {
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector(".bridge-update-state")?.textContent).toContain("0.3.0"));
    expect(document.querySelector(".bridge-update-body").textContent).toContain("0.2.0");
    document.querySelector("[data-bridge-check]").click();
    await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("bridge.check_update", {}));
    await vi.waitFor(() => expect(document.querySelector("[data-bridge-install-now]").disabled).toBe(false));
    document.querySelector("[data-bridge-install-now]").click();
    await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("bridge.install_update", { when: "now" }));
    await vi.waitFor(() => expect(document.querySelector("[data-bridge-install-idle]").disabled).toBe(false));
    document.querySelector("[data-bridge-install-idle]").click();
    await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("bridge.install_update", { when: "idle" }));
  });

  it("keeps development builds visible while refusing replacement", async () => {
    session.call.mockImplementation(async (method) => method === "bridge.update_status"
      ? { ...UPDATE, development_build: true, can_install: false }
      : method === "project.list" ? { projects: PROJECTS } : method === "models.list" ? CATALOG : SETTINGS);
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector(".bridge-update-body")?.textContent).toContain("development build"));
    expect(document.querySelector("[data-bridge-check]").disabled).toBe(false);
    expect(document.querySelector("[data-bridge-install-now]").disabled).toBe(true);
    expect(document.querySelector("[data-bridge-install-idle]").disabled).toBe(true);
  });

  it("shows an older bridge's compatibility message", async () => {
    session.call.mockImplementation(async (method) => {
      if (method === "bridge.update_status") throw new Error("unknown method: bridge.update_status");
      return method === "project.list" ? { projects: PROJECTS } : method === "models.list" ? CATALOG : SETTINGS;
    });
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector(".bridge-update-body")?.textContent).toContain("unavailable on this bridge"));
    expect(document.querySelector("[data-bridge-install-now]").disabled).toBe(true);
  });

  it("takes a bridge push through the real event dispatcher, cache, and live settings panel", async () => {
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector(".bridge-update-state")?.textContent).toContain("0.3.0"));
    const event = { type: "bridge.update_status", ...UPDATE, state: "failed", last_error: "Health check did not pass" };
    expect(dispatchChangeEvent(event, "other")).toBe(true);
    await vi.waitFor(() => expect(document.querySelector(".bridge-update-error")?.textContent).toContain("Health check did not pass"));
    expect((await readCached(bridgeUpdateAddress("other"))).value.last_error).toBe("Health check did not pass");
    expect(document.querySelector(".bridge-update-state").textContent).toBe("The update failed.");
  });

  it("marks the inbox settings cog and only the device that needs an update", async () => {
    document.body.insertAdjacentHTML("beforeend", '<div id="devpick"></div><button id="nav-account" aria-label="Settings"></button>');
    App.devices.push({ id: "second", name: "Server", status: "online" });
    paintDevicePicker();
    paintBridgeUpdateMark();
    expect(dispatchChangeEvent({ type: "bridge.update_status", ...UPDATE }, "other")).toBe(true);
    await vi.waitFor(() => expect(document.querySelector("#nav-account .bridge-update-dot")).toBeTruthy());
    expect(document.querySelector('[data-settings-device="other"] .bridge-update-dot')).toBeTruthy();
    expect(document.querySelector('[data-settings-device="second"] .bridge-update-dot')).toBeNull();
    App.devices[0].status = "offline";
    paintBridgeUpdateMark();
    expect(document.querySelector("#nav-account .bridge-update-dot")).toBeNull();
  });

  it("paints a cached projects folder while settings.get is absent", async () => {
    await writeCached(deviceSettingsAddress("other"), { ...SETTINGS, projects_dir: "/cached-projects" });
    session.call = vi.fn((method) => method === "settings.get"
      ? new Promise(() => {})
      : Promise.resolve(method === "project.list" ? { projects: PROJECTS } : method === "models.list" ? CATALOG : SETTINGS));
    void renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector("#device-projects-path")?.textContent).toBe("/cached-projects"));
    expect(document.querySelector("#device-projects-change").disabled).toBe(false);
  });

  it("repaints the projects folder after a real settings cache write", async () => {
    await renderDeviceSettings();
    await writeCached(deviceSettingsAddress("other"), { ...SETTINGS, projects_dir: "/announced-projects" });
    await vi.waitFor(() => expect(document.querySelector("#device-projects-path").textContent).toBe("/announced-projects"));
    expect((await readCached(deviceSettingsAddress("other"))).value.projects_dir).toBe("/announced-projects");
  });

  it("paints cached projects and bridge settings while project.list never answers", async () => {
    await writeCached(deviceProjectsAddress("other"), PROJECTS);
    session.call = vi.fn((method) => {
      if (method === "project.list") return new Promise(() => {});
      if (method === "models.list") return Promise.resolve(CATALOG);
      return Promise.resolve(SETTINGS);
    });
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelector("#projlist .projrow")?.textContent).toContain("relaydb"));
    await vi.waitFor(() => expect(document.querySelector("#defaultharness")?.value).toBe("claude"));
    expect(session.call).toHaveBeenCalledWith("project.list");
  });

  it("lists the device's projects over its own connection, with Add project and Set remote on that connection", async () => {
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.querySelectorAll("#projlist .projrow")).toHaveLength(1));

    expect(session.call).toHaveBeenCalledWith("project.list");
    const rows = [...document.querySelectorAll("#projlist .projrow")];
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("relaydb");
    expect(rows[0].textContent).toContain("Rift (copy-on-write)");

    document.querySelector("#newrepo").click();
    const addOptions = openNewRepo.mock.calls[0][1];
    expect(addOptions.defaultDeviceId).toBe("other");
    expect(addOptions.devices).toContainEqual(expect.objectContaining({ id: "other", name: "Other machine" }));
    await addOptions.callRpcFor("other")("project.create", { name: "docs" });
    expect(session.call).toHaveBeenLastCalledWith("project.create", { name: "docs" });

    document.querySelector("#projlist .setremote").click();
    const [project, , remoteOptions] = openSetRemote.mock.calls[0];
    expect(project.project_id).toBe("p1");
    await remoteOptions.callRpc("project.set_remote", { project_id: "p1", url: "" });
    expect(session.call).toHaveBeenLastCalledWith("project.set_remote", { project_id: "p1", url: "" });
  });

  it("mounts the agent modes, default harness and isolation panels over the same connection", async () => {
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.getElementById("defaultharness")?.value).toBe("claude"));

    expect(document.getElementById("agentmode-claude").value).toBe("tui");
    expect(document.getElementById("agentmode-codex").value).toBe("headless");
    expect(document.getElementById("defaultharness").value).toBe("claude");
    expect(document.querySelector("[data-isolation=select]").value).toBe("worktree");
    expect(document.querySelector("[data-isolation=select]").disabled).toBe(false);
    expect(document.querySelector("[data-triage-setting]")).toBeNull();
    // Everything the bridge owns is asked of this page's own connection.
    expect(session.call).toHaveBeenCalledWith("models.list");
  });

  // The machine is answering; nothing here can read the shape of its answers,
  // so settings.get would be a guess and the folder browser would write one.
  // The page says which side is behind and offers no way on.
  it("says the bridge is behind instead of asking a machine this app cannot read", async () => {
    contextFor.mockImplementation(() => ({ unsupported: "bridge", apiVersion: "0.9.0" }));

    await renderDeviceSettings();


    const status = document.querySelector("#device-settings-status").textContent;
    expect(status).toContain("Other machine speaks Build API 0.9.0");
    expect(status).toContain("update its bridge");
    expect(openSession).not.toHaveBeenCalled();
    expect(document.querySelector("#device-settings-retry").hidden).toBe(true);
    expect(document.querySelector("#device-projects-change").disabled).toBe(true);
    expect(document.querySelector("#projlist")).toBeNull();
  });

  // And the other way round: this tab is the one that is out of date.
  it("says the app is behind when the machine speaks a newer API than this tab", async () => {
    contextFor.mockImplementation(() => ({ unsupported: "app", apiVersion: "2.0.0" }));

    await renderDeviceSettings();


    expect(document.querySelector("#device-settings-status").textContent).toContain("reload to open it");
    expect(openSession).not.toHaveBeenCalled();
  });

  it("stands no panel up for a machine it cannot reach, and stands them up on retry", async () => {
    App.devices[0].status = "offline";
    await renderDeviceSettings();


    // Six panels all saying the machine is away say nothing six times; the page
    // says it once, where the way back on is.
    expect(document.querySelector("#projlist")).toBeNull();
    expect(document.getElementById("defaultharness")).toBeNull();
    expect(document.querySelector("#device-settings-status").textContent).toContain("Bring this device online");

    document.querySelector("#device-settings-retry").click();
    await vi.waitFor(() => expect(document.querySelector("#projlist").textContent).toContain("relaydb"));
    expect(document.getElementById("defaultharness").value).toBe("claude");
  });

  it("refreshes that device's model catalog when a harness setting is saved", async () => {
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.getElementById("defaultharness")?.disabled).toBe(false));

    const select = document.getElementById("defaultharness");
    select.value = "codex";
    select.dispatchEvent(new Event("change"));

    await vi.waitFor(() => expect(refreshModelCatalog).toHaveBeenCalled());

    expect(session.call).toHaveBeenCalledWith("settings.set", { default_harness: "codex" });
    expect(contextFor).toHaveBeenCalledWith("other");
    expect(refreshModelCatalog).toHaveBeenCalled();
  });

  it("saves for a machine the app holds no context for, and outlives a refused refresh", async () => {
    contextFor.mockReturnValue(null);
    await renderDeviceSettings();
    await vi.waitFor(() => expect(document.getElementById("agentmode-claude")?.disabled).toBe(false));
    const modes = document.getElementById("agentmode-claude");
    modes.value = "headless";
    modes.dispatchEvent(new Event("change"));

    await vi.waitFor(() => expect(document.querySelector('[data-agent-mode-status="claude"]').textContent).toBe("Saved."));
    expect(document.querySelector('[data-agent-mode-status="claude"]').textContent).toBe("Saved.");

    contextFor.mockImplementation(() => ({ refreshModelCatalog }));
    refreshModelCatalog.mockRejectedValueOnce(new Error("the catalog is gone"));
    modes.value = "tui";
    modes.dispatchEvent(new Event("change"));

    await vi.waitFor(() => expect(document.querySelector('[data-agent-mode-status="claude"]').textContent).toBe("Saved."));
    expect(document.querySelector('[data-agent-mode-status="claude"]').textContent).toBe("Saved.");
    expect(document.querySelector("[data-agent-modes-error]").textContent).toBe("");
  });
});
describe("device settings", () => {
  it("repaints the device page from a real cache announcement with no action response", async () => {
    App.devices = [];
    fetchDevices.mockImplementation(() => new Promise(() => {}));
    await readCachedDevices();
    await renderDeviceSettings();
    expect(document.querySelector("h1").textContent).toBe("Other machine settings");
    await writeCached(DEVICES_ADDRESS, [{ id: "other", name: "Cached workshop", status: "online" }]);

    await vi.waitFor(() => expect(document.querySelector("h1").textContent).toBe("Cached workshop settings"));
    expect(document.querySelector("#device-name").value).toBe("Cached workshop");
    expect(renameDevice).not.toHaveBeenCalled();
  });

  it("paints the cached device name while a rename answer is late, then repaints from its committed record", async () => {
    let finishRename;
    renameDevice.mockReturnValue(new Promise((resolve) => { finishRename = resolve; }));
    await renderDeviceSettings();

    const name = document.querySelector("#device-name");
    expect(document.querySelector("h1").textContent).toBe("Other machine settings");
    name.value = "  Workshop  ";
    document.querySelector("#device-name-form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(renameDevice).toHaveBeenCalledWith("other", "Workshop"));

    expect(renameDevice).toHaveBeenCalledWith("other", "Workshop");
    expect(document.querySelector("h1").textContent).toBe("Other machine settings");
    expect((await readCached(DEVICES_ADDRESS)).value[0].name).toBe("Other machine");
    finishRename({ device_id: "other", name: "Workshop" });
    await vi.waitFor(() => expect(document.querySelector("h1").textContent).toBe("Workshop settings"));

    expect(App.devices[0].name).toBe("Workshop");
    expect((await readCached(DEVICES_ADDRESS)).value[0].name).toBe("Workshop");
    expect(document.querySelector("#device-name-status").textContent).toBe("Saved.");
  });

  it("keeps the confirmed device name when a rename is refused", async () => {
    renameDevice.mockRejectedValue(new Error("Name could not be saved"));
    await renderDeviceSettings();
    const name = document.querySelector("#device-name");
    name.value = "Workshop";
    document.querySelector("#device-name-form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.querySelector("#device-name-status").textContent).toBe("Name could not be saved"));

    expect(App.devices[0].name).toBe("Other machine");
    expect(name.value).toBe("Other machine");
    expect(document.querySelector("#device-name-status").textContent).toBe("Name could not be saved");
  });

  it("writes the rename against a newer cached presence record", async () => {
    let finishRename;
    renameDevice.mockReturnValue(new Promise((resolve) => { finishRename = resolve; }));
    await renderDeviceSettings();
    document.querySelector("#device-name").value = "Workshop";
    document.querySelector("#device-name-form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await writeCached(DEVICES_ADDRESS, [{ id: "other", name: "Other machine", status: "offline" }]);
    finishRename({ device_id: "other", name: "Workshop" });
    await vi.waitFor(() => expect(App.devices[0].name).toBe("Workshop"));

    expect((await readCached(DEVICES_ADDRESS)).value[0]).toMatchObject({ name: "Workshop", status: "offline" });
  });

  it("finishes a rename in the shared device store after its settings panel closes", async () => {
    let finishRename;
    renameDevice.mockReturnValue(new Promise((resolve) => { finishRename = resolve; }));
    await renderDeviceSettings();
    document.querySelector("#device-name").value = "Workshop";
    document.querySelector("#device-name-form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    App.viewDispose();
    finishRename({ device_id: "other", name: "Workshop" });
    await vi.waitFor(() => expect(App.devices[0].name).toBe("Workshop"));

    expect((await readCached(DEVICES_ADDRESS)).value[0].name).toBe("Workshop");
  });

  it("leaves the device paired when deactivation is cancelled", async () => {
    await renderDeviceSettings();
    document.querySelector("#device-deactivate").click();
    await vi.waitFor(() => expect(document.querySelector("#device-deactivate")?.disabled).toBe(false));

    expect(confirmAction).toHaveBeenCalledWith(expect.objectContaining({ danger: true }));
    expect(revokeDevice).not.toHaveBeenCalled();
    expect(document.querySelector("#device-deactivate").disabled).toBe(false);
  });

  it("keeps device settings open and retryable when deactivation fails", async () => {
    confirmAction.mockResolvedValue(true);
    revokeDevice.mockRejectedValue(new Error("Could not deactivate device"));
    await renderDeviceSettings();
    document.querySelector("#device-deactivate").click();

    await vi.waitFor(() => expect(document.querySelector("#device-deactivate-status").textContent).toBe("Could not deactivate device"));

    expect(App.devices).toHaveLength(1);
    expect(retireDevice).not.toHaveBeenCalled();
    expect(document.querySelector("#device-deactivate-status").textContent).toBe("Could not deactivate device");
    expect(document.querySelector("#device-deactivate").disabled).toBe(false);
  });

  it.each(["online", "offline"])("deactivates an %s device and retires all of its local resources", async (status) => {
    App.devices[0].status = status;
    confirmAction.mockResolvedValue(true);
    revokeDevice.mockResolvedValue();
    const onDeviceDeactivated = vi.fn();
    await renderDeviceSettings({ onDeviceDeactivated });
    document.querySelector("#device-deactivate").click();
    await vi.waitFor(() => expect(retireDevice).toHaveBeenCalledWith("other"));

    expect(revokeDevice).toHaveBeenCalledWith("other");
    expect(App.devices).toEqual([]);
    expect((await readCached(DEVICES_ADDRESS)).value).toEqual([]);
    expect(onDeviceDeactivated).toHaveBeenCalledWith("other");
  });

  it("finishes local cleanup when the settings panel closes during revocation", async () => {
    let finishRevoke;
    confirmAction.mockResolvedValue(true);
    revokeDevice.mockReturnValue(new Promise((resolve) => { finishRevoke = resolve; }));
    const onDeviceDeactivated = vi.fn();
    await renderDeviceSettings({ onDeviceDeactivated });
    document.querySelector("#device-deactivate").click();
    await vi.waitFor(() => expect(revokeDevice).toHaveBeenCalledWith("other"));
    App.viewDispose();
    finishRevoke();
    await vi.waitFor(() => expect(retireDevice).toHaveBeenCalledWith("other"));

    expect(App.devices).toEqual([]);
    expect((await readCached(DEVICES_ADDRESS)).value).toEqual([]);
    expect(onDeviceDeactivated).not.toHaveBeenCalled();
  });

  it("closes an initial settings connection that lands after deactivation", async () => {
    let finishOpen;
    openSession.mockReturnValue(new Promise((resolve) => { finishOpen = resolve; }));
    confirmAction.mockResolvedValue(true);
    revokeDevice.mockResolvedValue();
    const rendering = renderDeviceSettings();
    document.querySelector("#device-deactivate").click();
    await vi.waitFor(() => expect(retireDevice).toHaveBeenCalledWith("other"));
    finishOpen(session);
    await rendering;

    expect(App.devices).toEqual([]);
    expect(session.close).toHaveBeenCalled();
    expect(session.call).not.toHaveBeenCalled();
  });

  it("shows bridge-owned preferences on the named device and saves isolation through its pinned session", async () => {
    await renderDeviceSettings();

    expect(document.querySelector("#root").textContent).toContain("Work isolation");
    expect(document.querySelector("#root").textContent).toContain("Agent modes");
    expect(document.querySelector("#root").textContent).toContain("Fallback agent");
    expect(document.querySelector("#root").textContent).not.toContain("Diff triage");
    const select = document.querySelector("[data-isolation=select]");
    await vi.waitFor(() => expect(select.value).toBe("worktree"));
    select.value = "rift";
    select.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("settings.set", { isolation: "rift" }));
  });

  it("uses the named device's Rift capability rather than another machine's", async () => {
    // Every machine answers for itself: a second device that has Rift says
    // nothing about whether this one does, and is never asked.
    const spare = {
      deviceId: "spare",
      call: vi.fn().mockResolvedValue({ isolation_available: { rift: true } }),
      close: vi.fn(),
    };
    App.devices = [...App.devices, { id: "spare", name: "Spare machine", status: "online" }];
    openSession.mockImplementation(async (deviceId) => (deviceId === "spare" ? spare : session));
    session.call.mockImplementation(async (method) => {
      if (method === "settings.get") return {
        projects_dir: "/projects",
        isolation: "worktree",
        isolation_available: { rift: false, reason: "Rift CLI was not found" },
        default_harness: "claude",
        agent_modes: { claude: "headless", codex: "headless" },
      };
      if (method === "models.list") return { providers: [{ id: "claude", label: "Claude Code" }] };
      return {};
    });

    await renderDeviceSettings();

    await vi.waitFor(() => expect(document.querySelector("[data-isolation=select]").value).toBe("worktree"));
    const rift = [...document.querySelector("[data-isolation=select]").options].find(({ value }) => value === "rift");
    expect(rift.disabled).toBe(true);
    expect(document.querySelector("[data-isolation=lock]").textContent).toContain("Rift CLI was not found");
    expect(spare.call).not.toHaveBeenCalled();
  });

  it("does not let a detached preference control call its old device", async () => {
    await renderDeviceSettings();
    const stale = document.querySelector("[data-isolation=select]");
    App.viewDispose();

    stale.value = "rift";
    stale.dispatchEvent(new Event("change"));

    expect(session.call).not.toHaveBeenCalledWith("settings.set", expect.anything());
  });

  it.each(["resolve", "reject"])(
    "keeps reconnected isolation authoritative when an old save %ss late",
    async (completion) => {
      await renderDeviceSettings();
      const oldSession = session;
      const oldSelect = document.querySelector("[data-isolation=select]");
      let resolveSave;
      let rejectSave;
      oldSession.call.mockReturnValueOnce(new Promise((resolve, reject) => {
        resolveSave = resolve;
        rejectSave = reject;
      }));
      oldSelect.value = "rift";
      oldSelect.dispatchEvent(new Event("change"));

      openSession.mock.calls[0][1].onLost();
      const newSettings = {
        projects_dir: "/new",
        isolation: "worktree",
        isolation_available: { rift: false, reason: "Rift is absent on the reconnected device" },
        default_harness: "claude",
        agent_modes: { claude: "headless", codex: "headless" },
      };
      const newSession = {
        deviceId: "other",
        close: vi.fn(),
        call: vi.fn(async (method) =>
          method === "models.list" ? { providers: [{ id: "claude", label: "Claude Code" }] } : newSettings),
      };
      openSession.mockResolvedValueOnce(newSession);
      document.querySelector("#device-settings-retry").click();
      await vi.waitFor(() => expect(document.querySelector("[data-isolation=select]").value).toBe("worktree"));
      const callsBeforeLateSave = newSession.call.mock.calls.length;

      if (completion === "resolve") resolveSave({ isolation: "rift", isolation_available: { rift: true } });
      else rejectSave(new Error("old save failed"));

      await Promise.allSettled(oldSession.call.mock.results.map(({ value }) => value));

      const currentSelect = document.querySelector("[data-isolation=select]");
      expect(currentSelect).not.toBe(oldSelect);
      expect(currentSelect.value).toBe("worktree");
      expect([...currentSelect.options].find(({ value }) => value === "rift").disabled).toBe(true);
      expect(document.querySelector("[data-isolation=lock]").textContent).toContain(
        "Rift is absent on the reconnected device",
      );
      expect(newSession.call).toHaveBeenCalledTimes(callsBeforeLateSave);
    },
  );

  it("opens the named device and saves through the same connection as the folder browser", async () => {
    await renderDeviceSettings();
    expect(openSession).toHaveBeenCalledWith("other", { onLost: expect.any(Function) });
    expect(document.querySelector("h1").textContent).toContain("Other machine");
    document.querySelector("#device-projects-change").click();
    const options = openBrowser.mock.calls[0][0];
    expect(options.startPath).toBe("/projects");
    expect(options.fallbackFromMissingStart).toBe(true);
    await options.callRpc("fs.list", { path: "/next" });
    expect(session.call).toHaveBeenLastCalledWith("fs.list", { path: "/next" });
    session.call.mockResolvedValueOnce({ projects_dir: "/next" });
    await options.onChoose("/next");
    expect(session.call).toHaveBeenLastCalledWith("settings.set", { projects_dir: "/next" });
    expect(document.querySelector("#device-projects-path").textContent).toBe("/next");
    expect(document.querySelector("#device-settings-status").textContent).toContain("Saved");
  });
  it("does not connect to an offline or unknown device", async () => {
    App.devices[0].status = "offline";
    await renderDeviceSettings();
    expect(openSession).not.toHaveBeenCalled();
    expect(document.querySelector("#device-projects-change").disabled).toBe(true);
    App.route.id = "unknown";
    await renderDeviceSettings();
    expect(openSession).not.toHaveBeenCalled();
    expect(document.querySelector("#root").textContent).toContain("Device not found");
  });
  it("closes a connection that finishes opening after leaving", async () => {
    let resolve;
    openSession.mockReturnValue(new Promise((done) => { resolve = done; }));
    const rendering = renderDeviceSettings();
    App.viewDispose();
    resolve(session);
    await rendering;
    expect(session.close).toHaveBeenCalled();
    expect(session.call).not.toHaveBeenCalled();
  });
  it("rejects a stale picker save after leaving the page", async () => {
    await renderDeviceSettings();
    document.querySelector("#device-projects-change").click();
    const options = openBrowser.mock.calls[0][0];
    App.viewDispose();
    await options.onChoose("/wrong");
    expect(session.call).not.toHaveBeenCalledWith("settings.set", expect.anything());
    expect(session.close).toHaveBeenCalled();
  });
  it("shows save errors without replacing the confirmed directory", async () => {
    await renderDeviceSettings();
    document.querySelector("#device-projects-change").click();
    document.querySelector("#sheet").innerHTML = '<div id="berr"></div>';
    session.call.mockRejectedValueOnce(new Error("Permission denied"));
    await openBrowser.mock.calls[0][0].onChoose("/restricted");
    expect(document.querySelector("#berr").textContent).toBe("Permission denied");
    expect(document.querySelector("#device-projects-path").textContent).toBe("/projects");
  });
});

it("disables folder selection after disconnect and ignores old connection callbacks after retry", async () => {
  await renderDeviceSettings();
  const onLost = openSession.mock.calls[0][1].onLost;
  onLost();
  expect(document.querySelector("#device-projects-change").disabled).toBe(true);
  expect(document.querySelector("#device-settings-retry").hidden).toBe(false);
  expect(session.close).toHaveBeenCalled();
  document.querySelector("#device-settings-retry").click();
  await vi.waitFor(() => expect(document.querySelector("#device-projects-change").disabled).toBe(false));
  onLost();
  expect(document.querySelector("#device-projects-change").disabled).toBe(false);
});
it("closes the connection when loading settings fails", async () => {
  session.call.mockImplementation(async (method) => {
    if (method === "settings.get") throw new Error("Settings unavailable");
    if (method === "project.list") return { projects: PROJECTS };
    if (method === "models.list") return CATALOG;
    return { ...SETTINGS };
  });
  await renderDeviceSettings();
  expect(session.close).toHaveBeenCalled();
  expect(document.querySelector("#device-projects-change").disabled).toBe(true);
  expect(document.querySelector("#device-settings-retry").hidden).toBe(false);
});
it.each(["resolve", "reject"])("ignores a stale save that %ss after reconnect, while allowing a new save", async (completion) => {
  await renderDeviceSettings();
  document.querySelector("#device-projects-change").click();
  let resolveSave, rejectSave;
  session.call.mockReturnValueOnce(new Promise((resolve, reject) => { resolveSave = resolve; rejectSave = reject; }));
  const staleSave = openBrowser.mock.calls[0][0].onChoose("/old");
  openSession.mock.calls[0][1].onLost();
  const newSession = { deviceId: "other", close: vi.fn(), call: vi.fn().mockResolvedValue({ projects_dir: "/new" }) };
  openSession.mockResolvedValueOnce(newSession);
  document.querySelector("#device-settings-retry").click();
  await vi.waitFor(() => expect(document.querySelector("#device-projects-path").textContent).toBe("/new"));
  document.querySelector("#device-projects-change").click();
  newSession.call.mockResolvedValueOnce({ projects_dir: "/latest" });
  await openBrowser.mock.calls[1][0].onChoose("/latest");
  expect(newSession.call).toHaveBeenLastCalledWith("settings.set", { projects_dir: "/latest" });
  if (completion === "resolve") resolveSave({ projects_dir: "/old" });
  else rejectSave(new Error("Old connection error"));
  await staleSave;
  expect(document.querySelector("#device-projects-path").textContent).toBe("/latest");
  expect(document.querySelector("#device-settings-status").textContent).toBe("Saved. New projects will use this folder.");
});
