// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
let feed;
const listeners = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => { listeners.add(fn); fn(feed); return () => listeners.delete(fn); },
  startFeed: () => {}, stopFeed: () => {}, refreshFeed: async () => [], deliverFeed: () => {}, dropFeedDevice: () => {},
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));
import { App } from "../src/app.js";
import { initToolbar, stopToolbar, toolbarRouteChanged } from "../src/core/toolbar.js";
import { adoptDeviceSession, adoptBridgeSelection, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";
import { writeCached, wipeCache } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { stampWorkspace } from "../src/core/feedMerge.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";
const workspace = (locked = false) => stampWorkspace({ id: "ws-398", workspace_id: "ws-398", project_id: "p1", name: "Workspace lock", locked, status: "ready", managed: true, directories: [] }, "dev-398");
const update = (locked) => { feed.workspaces = [workspace(locked)]; feed.devices["dev-398"].workspaces = feed.workspaces; listeners.forEach((fn) => fn(feed)); };
const support = (setLocked) => writeCached({ deviceId: "dev-398", entityId: "", kind: "workspace-lock-support" }, { setLocked });
const button = () => document.querySelector("[data-workspace-lock]");
const rpc = vi.fn();
const session = () => ({ deviceId: "dev-398", call: rpc, close: () => {}, peer: () => {}, onCarrier: () => {}, onPush: () => {} });
beforeEach(async () => {
  await stopToolbar(); await wipeCache(); await wipeUiRecords(); resetDeviceContexts(); resetChangeEvents(); listeners.clear();
  document.body.innerHTML = '<div id="toolbar"></div>';
  App.route = { name: "workspace", deviceId: "dev-398", projectId: "p1", workspaceId: "ws-398" };
  App.deviceFilter = []; App.devices = [];
  const projects = [{ id: "p1", project_id: "p1", deviceId: "dev-398", projectKey: "dev-398/p1", name: "Build", path: "/source" }];
  feed = { items: [], projects, workspaces: [workspace()], devices: { "dev-398": { items: [], projects, workspaces: [workspace()] } } };
  rpc.mockReset(); notifyError.mockReset();
  const context = adoptDeviceSession(session());
  adoptBridgeSelection(context, { version: "3.14.0" }, { capabilities: { workspaces: { setLocked: true } } });
});
afterEach(async () => { await stopToolbar(); resetDeviceContexts(); resetChangeEvents(); });
it("toggles through RPC and flips its icon only when the cached feed moves", async () => {
  await support(true); await initToolbar();
  await vi.waitFor(() => expect(button()).not.toBeNull());
  expect(button().getAttribute("aria-label")).toBe("Lock workspace");
  expect(button().title).toBe("Lock workspace");
  expect(button().querySelector("svg").classList.contains("lucide-lock-open")).toBe(true);
  const unlockedIcon = button().innerHTML;
  rpc.mockResolvedValue({ ...workspace(true), locked: true });
  button().click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledWith("workspace.set_locked", { workspace_id: "ws-398", locked: true }));
  await vi.waitFor(() => expect(button().disabled).toBe(false));
  expect(button().innerHTML).toBe(unlockedIcon);
  update(true);
  expect(button().getAttribute("aria-label")).toBe("Unlock workspace");
  expect(button().title).toBe("Unlock workspace");
  expect(button().querySelector("svg").classList.contains("lucide-lock")).toBe(true);
  button().click();
  await vi.waitFor(() => expect(rpc).toHaveBeenLastCalledWith("workspace.set_locked", { workspace_id: "ws-398", locked: false }));
  update(false);
  expect(button().getAttribute("aria-label")).toBe("Lock workspace");
});
it("omits the lock control without cached capability and reacts when it arrives", async () => {
  await initToolbar(); expect(button()).toBeNull();
  expect(capabilitiesOf({ api_version: "3.13.0", capabilities: [] }).workspaces.setLocked).toBe(false);
  await support(true); await vi.waitFor(() => expect(button()).not.toBeNull());
  await support(false); await vi.waitFor(() => expect(button()).toBeNull());
});
it("waits for the replacement greeting and retires cached support on an older bridge", async () => {
  await support(true); await initToolbar(); await vi.waitFor(() => expect(button()).not.toBeNull());
  const context = adoptDeviceSession(session());
  button().click();
  await Promise.resolve();
  expect(rpc).not.toHaveBeenCalled();
  expect(button().disabled).toBe(true);
  await greetBridge(async (method, params) => method === "session.hello"
    ? { api_version: "3.13.0", capabilities: [] } : rpc(method, params), {
    deviceId: "dev-398", strict: true,
    install: (selection) => {
      const adapter = selection.create(rpc);
      adoptBridgeSelection(context, selection, adapter);
      return adapter;
    },
  });
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Update the bridge to lock workspaces."));
  expect(rpc).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(button()).toBeNull());
});
it("sends a pending lock only after the replacement greeting advertises it", async () => {
  await support(true); await initToolbar(); await vi.waitFor(() => expect(button()).not.toBeNull());
  const context = adoptDeviceSession(session());
  button().click();
  await Promise.resolve();
  expect(rpc).not.toHaveBeenCalled();
  adoptBridgeSelection(context, { version: "3.14.0" }, { capabilities: { workspaces: { setLocked: true } } });
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledExactlyOnceWith("workspace.set_locked", { workspace_id: "ws-398", locked: true }));
  await vi.waitFor(() => expect(button().disabled).toBe(false));
  expect(button().getAttribute("aria-label")).toBe("Lock workspace");
});
it("refuses a downgraded session before the cached control has been retired", async () => {
  await support(true); await initToolbar(); await vi.waitFor(() => expect(button()).not.toBeNull());
  const context = adoptDeviceSession(session());
  await greetBridge(async (method, params) => method === "session.hello"
    ? { api_version: "3.13.0", capabilities: [] } : rpc(method, params), {
    deviceId: "dev-398", strict: true,
    install: (selection) => {
      const adapter = selection.create(rpc);
      adoptBridgeSelection(context, selection, adapter);
      button().click();
      return adapter;
    },
  });
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Update the bridge to lock workspaces."));
  expect(rpc).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(button()).toBeNull());
});
it("uses each device's capability and reports lock mutation failures", async () => {
  await support(true); await initToolbar(); await vi.waitFor(() => expect(button()).not.toBeNull());
  rpc.mockRejectedValue(new Error("Workspace is locked. Unlock it to delete it."));
  button().click();
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Workspace is locked. Unlock it to delete it."));
  expect(button().getAttribute("aria-label")).toBe("Lock workspace");
  App.route = { ...App.route, deviceId: "older" }; toolbarRouteChanged();
  expect(button()).toBeNull();
});

it("gives a brief retry hint when the bridge has a filesystem job in flight", async () => {
  await support(true); await initToolbar(); await vi.waitFor(() => expect(button()).not.toBeNull());
  rpc.mockRejectedValue(Object.assign(new Error("another filesystem operation is still running"), { code: "busy" }));
  button().click();
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Try again in a moment"));
  expect(button().getAttribute("aria-label")).toBe("Lock workspace");
});
