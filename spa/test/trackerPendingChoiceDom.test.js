/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task as wireTask } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: { waitingOn: true } }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const task = (over = {}) => wireTask({ project_id: "project", ...over });
const actor = { kind: "agent", agent_id: "AgentA\0suffix" };
const feed = {
  projects: [{ projectKey: "device|project", projectRoster: { agents: [
    { id: actor.agent_id, name: "Opaque target", project_role: "worker" },
  ] } }],
  items: [], workspaces: [],
};
const selectors = [
  { label: "Anyone assigned", wire: "any", visible: [true, true, false] },
  { label: "Unassigned", wire: "none", visible: [false, false, true] },
  { label: "Opaque target", wire: actor, visible: [false, true, false] },
];
let host, pane, cache, listCache, readOrder, held, call, reply, committed;
const titles = () => [...host.querySelectorAll(".task-title")].map((row) => row.textContent);
const menu = () => host.querySelector('[data-filter-menu="assignee"]');
const key = (value) => menu().querySelector(".fmenu-search").dispatchEvent(
  new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true }),
);
const expectShown = async (shown) => {
  await vi.waitFor(() => expect(titles()).toEqual(shown ? [held.title] : []), { timeout: 10_000 });
};
const store = async () => {
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const contentRead = await captureTaskContentRead(listCache.tasksAddress("device", "project"), { revalidate: "full" });
  await listCache.writeTasksRecord("device", "project", listCache.tasksRecord([held], columns(), contentRead.readOrder), { contentRead });
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="host"></div>';
  host = document.querySelector("#host");
  cache = await import("../src/core/localCache.js");
  listCache = await import("../src/core/trackerCache.js");
  readOrder = await import("../src/core/taskReadOrder.js");
  held = task({ id: "pending", title: "A task with a saved delivery", deferred_assignment: { state: "waiting", target: actor } });
  await store();
  committed = false;
  reply = null;
  call = vi.fn(async (method, params) => {
    if (method === "tasks.get") return { task: held, timeline: [] };
    if (method !== "tasks.list") return {};
    if (params.assignee === undefined) return { tasks: [held] };
    return new Promise((resolve) => { reply = resolve; });
  });
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, { deviceId: "device", projectId: "project", projectKey: "device|project",
    callRpc: call, feed: () => feed, catalog: () => ({ providers: [] }), defaultView: "list", navigate() {} });
  await expectShown(true);
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", { project_id: "project", state: "open" }), { timeout: 10_000 });
});

afterEach(() => {
  pane?.dispose();
  reply?.({ tasks: [] });
});

describe.each(["click", "Enter"])("a rendered %s choice through the Tasks pane", (gesture) => {
  it.each(selectors)("keeps $label canonical through pending, assigned and unassigned cache writes", async (selector) => {
    menu().querySelector(".fmenu-press").click();
    await vi.waitFor(() => expect(menu().querySelector(".fmenu-pop").hidden).toBe(false), { timeout: 10_000 });
    const options = [...menu().querySelectorAll('[role="option"]')];
    const index = options.findIndex((row) => row.querySelector(".fmenu-label").textContent === selector.label);
    expect(index).toBeGreaterThanOrEqual(0);
    if (gesture === "click") options[index].click();
    else {
      for (let at = 0; at < index; at += 1) key("ArrowDown");
      key("Enter");
    }
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", {
      project_id: "project", state: "open", assignee: selector.wire,
    }), { timeout: 10_000 });
    expect(reply).toBeTypeOf("function");
    await expectShown(selector.visible[0]);
    expect(menu().querySelector(".fmenu-press-label").textContent).toBe(selector.label);

    held = { ...held, assignee: actor, deferred_assignment: null, updated_at: "2026-08-21T10:01:00Z" };
    await store();
    await expectShown(selector.visible[1]);
    held = { ...held, assignee: null, updated_at: "2026-08-21T10:02:00Z" };
    await store();
    await expectShown(selector.visible[2]);

    const stop = cache.subscribeCache(listCache.tasksQueryAddress("device", "project", {
      project_id: "project", state: "open", assignee: selector.wire,
    }), () => { committed = true; });
    // Complete the narrowed read after the newer whole-list cache updates.
    reply({ tasks: selector.visible[0] ? [task({ id: "pending", title: held.title,
      deferred_assignment: { state: "waiting", target: actor } })] : [] });
    await vi.waitFor(() => expect(committed).toBe(true), { timeout: 10_000 });
    stop();
    await expectShown(selector.visible[2]);
  });
});
