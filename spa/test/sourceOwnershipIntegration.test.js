// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { createBridgeEpoch } from "../src/core/bridgeEpoch.js";
import { adoptBridgeSelection, adoptDeviceSession, captureDeviceGreeting, greetingInFlight, resetDeviceContexts, setContextOffline } from "../src/core/deviceContexts.js";
import { greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { environmentSupportAddress } from "../src/core/environmentSupport.js";
import { projectSettingsAddress } from "../src/core/settingsRecords.js";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { mountProjectChanges } from "../src/views/projectChanges.js";
import { projectChangesRpc } from "../src/core/projectChangesRpc.js";
import { guardProjectSourceRpc } from "../src/core/sourceOwnership.js";

// Only the unrelated Git pane is stubbed. Greeting, epoch acceptance,
// device authority, settings, Changes and RPC ownership guards are real.
vi.mock("../src/views/workspaceChanges.js", () => ({ mountWorkspaceChanges: () => ({ workspaceMoved() {}, selectDirectory() {}, setVisible() {}, dispose() {} }) }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const deviceId = "legacy-integration";
const project = { project_id: "p", name: "Legacy", path: "/legacy", sources: [
  { id: "first", name: "First", path: "/legacy", is_git: true, remote: "old", base_branch: "main" },
  { id: "second", name: "Second", path: "/second", is_git: true, base_branch: "main" },
] };
let context, session, changes;

async function acceptLegacyGreeting({ updateSource = true, epoch = false, extraCapabilities = [] } = {}) {
  const greeting = { api_version: "3.36.0", capabilities: ["sources.syncBase", "sources.syncBasePath", ...(updateSource ? ["project.update_source"] : []), ...(epoch ? ["bridge.epoch"] : []), ...extraCapabilities],
    ...(epoch ? { bridge_epoch: "integration-epoch", journal_incarnation: "integration-journal" } : {}),
  };
  session = { deviceId, bridgeEpoch: createBridgeEpoch(), call: vi.fn(async method => {
    if (method === "session.hello") return greeting;
    return method === "project.list" ? { projects: [project] } : project;
  }) };
  context = adoptDeviceSession(session);
  const authority = greetingInFlight(context);
  await greetBridge(session.call, { deviceId, strict: true, isCurrent: () => authority.current(),
    acceptGreeting: received => session.bridgeEpoch.acceptGreeting(received),
    install: selection => {
      const adapter = selection.create(session.call);
      adoptBridgeSelection(context, selection, adapter, authority);
      return adapter;
    },
  });
  await vi.waitFor(async () => expect((await readCached(environmentSupportAddress(deviceId)))?.value.list).toBe(extraCapabilities.includes("environments")));
  if (!epoch) {
    expect(session.bridgeEpoch.current()).toBeNull();
    expect(captureDeviceGreeting(context)).toBeNull(); // epoch-only consumers retain their contract
  }
}

function settings() {
  openProjectSettings("p", { deviceId, callRpc: context.rpc });
}
function sourceCard() { return document.querySelector('[data-source-id="first"]'); }
function snapshot() {
  return [...document.querySelectorAll(".ps-source")].map(card => ({
    fields: [...card.querySelectorAll("input[data-field]")].map(input => [input.dataset.field, input.readOnly]),
    save: Boolean(card.querySelector("[data-save-source]")), remove: Boolean(card.querySelector("[data-remove-source]")),
    syncToggle: Boolean(card.querySelector("[data-sync-base]")), syncNow: Boolean(card.querySelector("[data-sync-now]")),
  }));
}
function mountChanges() {
  const host = document.createElement("div"); document.body.append(host);
  changes = mountProjectChanges(host, { project, route: { projectId: "p", sourceId: "first" }, context,
    onSourceSelection() {}, onCommitSelection() {}, onOpenFile() {} });
  return host;
}

beforeEach(async () => {
  resetDeviceContexts(); resetChangeEvents(); await wipeCache();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  await writeCached(projectSettingsAddress(deviceId, "p"), project);
});
afterEach(() => { changes?.dispose(); changes = null; resetDeviceContexts(); resetChangeEvents(); document.body.replaceChildren(); });

it.each(["save", "remove", "set remote", "sync toggle", "sync now"])("allows legacy %s through a real accepted greeting without bridge.epoch", async action => {
  await acceptLegacyGreeting({ updateSource: action !== "set remote" });
  expect.soft(captureDeviceGreeting(context, { allowLegacy: true })?.epoch).toBeNull();
  settings();
  await vi.waitFor(() => expect(sourceCard()?.querySelector("[data-save-source]")).toBeTruthy());
  if (["save", "set remote"].includes(action)) {
    const input = sourceCard().querySelector(`[data-field="${action === "save" ? "name" : "remote"}"]`);
    input.value = "Updated"; input.dispatchEvent(new Event("input"));
    sourceCard().querySelector("[data-save-source]").click();
  } else if (action === "remove") document.querySelector('[data-remove-source="second"]').click();
  else if (action === "sync toggle") {
    const toggle = sourceCard().querySelector("[data-sync-base]"); toggle.checked = true; toggle.dispatchEvent(new Event("change"));
  } else sourceCard().querySelector("[data-sync-now]").click();
  const method = { save: "project.update_source", remove: "project.remove_source", "set remote": "project.set_remote", "sync toggle": "project.update_source", "sync now": "project.sync_source" }[action];
  await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith(method, expect.objectContaining({ project_id: "p" })));
});

it("allows Project Changes Sync through a real accepted greeting without bridge.epoch", async () => {
  await acceptLegacyGreeting();
  const host = mountChanges();
  await vi.waitFor(() => expect(host.querySelector("[data-project-sync-base]").hidden).toBe(false));
  host.querySelector("[data-project-sync-base]").click();
  await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("project.sync_source", expect.objectContaining({ project_id: "p", source_id: "first" })));
  const rpc = projectChangesRpc(context, project.sources[0], { projectId: "p", currentProject: () => project });
  await expect(rpc("project.sync_source", {})).resolves.toMatchObject({ project_id: "p" });
});

it.each(["environments", "projects.addedSources"])("refuses both dispatch entry points when a no-epoch greeting advertises %s", async capability => {
  await acceptLegacyGreeting({ extraCapabilities: [capability] });
  const owned = { ...project, added_sources: project.sources };
  await writeCached(projectSettingsAddress(deviceId, "p"), owned);
  const settingsRpc = guardProjectSourceRpc(deviceId, context.rpc, async () => (await readCached(projectSettingsAddress(deviceId, "p"))).value);
  const changesRpc = projectChangesRpc(context, project.sources[0], { projectId: "p", currentProject: () => owned });
  await expect(settingsRpc("project.update_source", { project_id: "p", source_id: "first", name: "Refused" })).rejects.toThrow(/read-only/);
  await expect(changesRpc("project.sync_source", {})).rejects.toThrow(/read-only/);
  expect(session.call.mock.calls.some(([method]) => ["project.update_source", "project.sync_source"].includes(method))).toBe(false);
});

it.each(["offline", "connecting"])("reopens settings and Changes from the same populated cache while %s", async state => {
  await acceptLegacyGreeting({ epoch: true });
  settings();
  await vi.waitFor(() => expect(sourceCard()?.querySelector("[data-save-source]")).toBeTruthy());
  const connected = snapshot();
  document.querySelector("#pscancel").click();
  let host = mountChanges();
  await vi.waitFor(() => expect(host.querySelector("[data-project-sync-base]").hidden).toBe(false));
  const connectedSync = host.querySelector("[data-project-sync-base]").hidden;
  changes.dispose(); changes = null; host.remove();
  const cached = (await readCached(environmentSupportAddress(deviceId))).value;
  if (state === "offline") setContextOffline(deviceId);
  else adoptDeviceSession({ deviceId, call: vi.fn(() => new Promise(() => {})), bridgeEpoch: createBridgeEpoch() });
  expect(captureDeviceGreeting(context, { allowLegacy: true })).toBeNull();
  settings();
  await vi.waitFor(() => expect(sourceCard()).toBeTruthy());
  expect(snapshot()).toEqual(connected);
  document.querySelector("#pscancel").click();
  host = mountChanges();
  await vi.waitFor(() => expect(host.querySelector("[data-project-source-context]").textContent).not.toBe(""));
  expect(host.querySelector("[data-project-sync-base]").hidden).toBe(connectedSync);
  expect((await readCached(environmentSupportAddress(deviceId))).value).toEqual(cached);
});
