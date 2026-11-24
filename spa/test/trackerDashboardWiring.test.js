/** @vitest-environment jsdom */
// Real pane, dashboard projection, cache, and change watcher wiring. Only the
// remote RPC is held open so every visible change has to come from cache.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTasksPane } from "../src/core/trackerTasksPane.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { adoptBridgeSelection, adoptDeviceSession, knownDeviceContext, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { selectAdapter } from "../src/core/bridgeApi/index.js";
import { createHttpRoutes } from "../src/core/httpRoutes.js";
import { openHttpSession } from "../src/core/httpSession.js";
import { readCached, writeCached } from "../src/core/localCache.js";
import { deviceModelsAddress } from "../src/core/settingsRecords.js";
import manifest from "../../fixtures/api/http/resource-manifest.json";
import { fakeSession } from "./deviceSessionFixture.js";
import { tasksRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { columns, task } from "./trackerWireFixture.js";

let pane;
let host;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
});

afterEach(() => {
  pane?.dispose();
  pane = null;
  resetDeviceContexts();
  resetChangeEvents();
});

it("wires a mounted tasks pane to cached dashboard tabs and live counts", async () => {
  const review = task({ id: "review", number: 7, title: "Review changes", status: "in_review" });
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([review], columns()));
  pane = mountTasksPane(host, {
    deviceId: "dashboard-device",
    projectId: "dashboard-project",
    projectName: "Build",
    projectKey: "dashboard-project",
    defaultView: "dashboard",
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });

  await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("needsYou"));
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-title")?.textContent).toBe("Review changes"));
  expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count').textContent).toBe("1");

  host.querySelector('[data-dashboard-tab="active"]').click();
  await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("active"));
  expect(host.querySelector(".task-dashboard-empty")?.textContent).toBe("No one holds a task right now.");

  const assigned = task({ id: "assigned", number: 8, title: "Take a look", assignee: { kind: "user" } });
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([assigned, review], columns()));
  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count')?.textContent).toBe("2"));
  host.querySelector('[data-dashboard-tab="needsYou"]').click();
  await vi.waitFor(() => expect([...host.querySelectorAll(".task-dashboard-title")].map((one) => one.textContent))
    .toEqual(["Take a look", "Review changes"]));
  expect(host.querySelector('[data-dashboard-tab="active"] .task-dashboard-count')?.textContent).toBe("0");
  expect(host.querySelector('[data-dashboard-tab="backlog"] .task-dashboard-count')?.textContent).toBe("1");
});

it("moves a mounted task between Active groups when the cached feed changes", async () => {
  const held = task({ id: "task", number: 8, title: "Review the draft", assignee: { kind: "agent", agent_id: "agent-1" } });
  let feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: false }] }] };
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([held], columns()));
  pane = mountTasksPane(host, {
    deviceId: "dashboard-device",
    projectId: "dashboard-project",
    projectName: "Build",
    projectKey: "dashboard-project",
    defaultView: "dashboard",
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => feed,
  });

  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-tab="active"]')).not.toBeNull());
  host.querySelector('[data-dashboard-tab="active"]').click();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Assigned"));
  expect(host.querySelector(".task-dashboard-detail")?.textContent).toContain("Backlog");

  feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: true }] }] };
  pane.feedMoved();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Working"));
  expect(host.querySelector(".task-dashboard-detail")?.textContent).toContain("working now");

  feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: false }] }] };
  pane.feedMoved();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Assigned"));
});

const offer = {
  default_provider: "claude_adk",
  providers: [
    { id: "claude_adk", models: [
      { id: "opus", label: "Opus", supports_effort: true, efforts: ["high"] },
      { id: "sonnet", label: "Sonnet", supports_effort: false },
    ], efforts: ["low", "high"] },
    { id: "codex", models: [{ id: "gpt", label: "GPT", supports_effort: true }], efforts: ["medium"] },
  ],
};
const greeting = { api_version: "3.24.0", capabilities: ["bridge.epoch"], bridge_epoch: "epoch", journal_incarnation: "journal" };

async function catalogSession(transport, deviceId, models) {
  const selection = selectAdapter(greeting);
  if (transport === "WebRTC") {
    const adapter = selection.create((method, params) => method === "models.list" ? models(params) : Promise.resolve({}));
    return { session: { ...fakeSession(deviceId), call: adapter.call }, selection, adapter };
  }
  const endpoint = {
    identity: { deviceId },
    open: async () => ({ session_id: "session", opening_generation: "opening", greeting, transport: manifest.transport }),
    subscribe: async (_opening, listener) => {
      await listener({ event: "hello", data: { durable: false, recovery: "current_state_refresh", bridge_epoch: "epoch", opening_generation: "opening" } });
      return { binding: "stream", close() {} };
    },
    async request(_opening, route) {
      const result = route.path === "/v1/models" ? await models(route) : {};
      return { ok: true, bridge_epoch: "epoch", result };
    },
    close() {},
  };
  const session = await openHttpSession({ endpoint, routes: createHttpRoutes(manifest) });
  const adapter = session.installAdapter(selection);
  session.allowRefresh();
  return { session, selection, adapter };
}

const selectField = (field) => host.querySelector(`#task-new-${field}`);
const values = (field) => [...selectField(`choice-${field}`).options].map((option) => option.value);
const choose = (field, value) => {
  const select = selectField(field);
  select.value = value;
  select.dispatchEvent(new Event("change"));
};

function openSecondProjectComposer(context) {
  const projects = ["Build", "December Game Jam"].map((name, index) => ({
    id: `proj-${index + 1}`, name, projectKey: `${context.deviceId}|proj-${index + 1}`,
  }));
  const [, project] = projects;
  const callRpc = vi.fn(() => new Promise(() => {}));
  pane = mountTasksPane(host, {
    deviceId: context.deviceId, projectId: project.id, projectName: project.name, projectKey: project.projectKey,
    defaultView: "dashboard", callRpc,
    catalog: () => context.modelCatalog(),
    feed: () => ({ projects, items: [], workspaces: [] }),
  });
  host.querySelector("[data-task-new]").click();
  expect(host.querySelector(".task-compose").getAttribute("aria-label")).toBe("File a task in December Game Jam");
  choose("assignee", "new_workspace");
  expect(selectField("isolation").selectedOptions[0].textContent).toBe("Inherit project setting");
  host.querySelector("[data-agent-choice-toggle]").click();
  expect(selectField("choice-provider").selectedOptions[0].textContent).toBe("Claude Code");
  return callRpc;
}

it.each(["WebRTC", "HTTPS"])("fills the second project's open New task model and effort menus over %s", async (transport) => {
  const { promise, resolve } = Promise.withResolvers();
  const models = vi.fn(() => promise);
  const deviceId = "menu-device";
  const { session, selection, adapter } = await catalogSession(transport, deviceId, models);
  const context = adoptDeviceSession(session);
  const callRpc = openSecondProjectComposer(context);
  expect(values("model")).toEqual([""]);
  expect(selectField("choice-model").selectedOptions[0].textContent).toBe("Harness default");
  const title = selectField("summary");
  title.value = "December Game Jam";
  title.dispatchEvent(new Event("input"));
  title.focus();

  adoptBridgeSelection(context, selection, adapter);
  await vi.waitFor(() => expect(models).toHaveBeenCalledOnce());
  expect(models.mock.calls[0][0]).toEqual(transport === "HTTPS"
    ? expect.objectContaining({ method: "GET", path: "/v1/models" })
    : undefined);
  resolve(offer);
  await vi.waitFor(() => expect(values("model")).toEqual(["", "opus", "sonnet"]));
  expect((await readCached(deviceModelsAddress(deviceId)))?.value).toEqual(offer);
  expect(values("effort")).toEqual(["", "low", "high"]);
  expect(selectField("summary")).toBe(title);
  expect(title.value).toBe("December Game Jam");
  expect(document.activeElement).toBe(title);

  choose("choice-model", "opus");
  expect(values("effort")).toEqual(["", "high"]);
  choose("choice-effort", "high");
  choose("choice-provider", "codex");
  expect(values("model")).toEqual(["", "gpt"]);
  expect(values("effort")).toEqual(["", "medium"]);
  expect(selectField("choice-model").value).toBe("");
  expect(selectField("choice-effort").value).toBe("");

  choose("choice-model", "gpt");
  choose("choice-effort", "medium");
  host.querySelector("[data-compose-file]").click();
  expect(callRpc).toHaveBeenCalledWith("tasks.create", expect.objectContaining({
    project_id: "proj-2",
    assignee: { kind: "new_workspace", provider: "codex", model: "gpt", effort: "medium" },
  }));
});

it("shows the cached harness catalog when the device has no session", async () => {
  const context = knownDeviceContext("menu-device");
  await writeCached(deviceModelsAddress(context.deviceId), offer);
  openSecondProjectComposer(context);
  await vi.waitFor(() => expect(values("model")).toEqual(["", "opus", "sonnet"]));
  expect(values("effort")).toEqual(["", "low", "high"]);
});
