/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

let support, session, watchers;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: support }),
  watchChanges: (options) => { watchers.push(options); return { dispose() {} }; },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const scope = { deviceId: "choice-device", projectId: "choice-project" };
const nulActor = { kind: "agent", agent_id: "AgentA\0suffix" };
const replacementActor = { kind: "agent", agent_id: "AgentA\uFFFDsuffix" };
const nulLabel = "remote\0label";
const replacementLabel = "remote\uFFFDlabel";
const page = (dimension, choices, next_cursor = null) => ({ project_id: scope.projectId, dimension, choices, next_cursor });
let host, pane, call, tasks, choices, remote;
const menu = (dimension) => host.querySelector(`[data-filter-menu="${dimension}"]`);
const choiceCalls = (dimension) => call.mock.calls.filter(([method, params]) =>
  method === "tasks.filterChoices" && (!dimension || params.dimension === dimension));
const rows = (dimension) => [...menu(dimension).querySelectorAll('[role="option"]')];
const labels = (dimension) => rows(dimension).map((row) => row.querySelector(".fmenu-label").textContent);
const more = (dimension) => menu(dimension).querySelector("[data-task-choices-more]");
const titles = () => [...host.querySelectorAll(".task-title")].map((row) => row.textContent);
const open = async (dimension) => {
  if (menu(dimension).querySelector(".fmenu-pop").hidden) menu(dimension).querySelector(".fmenu-press").click();
  await vi.waitFor(() => expect(menu(dimension).querySelector(".fmenu-pop").hidden).toBe(false), { timeout: 10_000 });
};
const select = (dimension, index, gesture) => {
  if (gesture === "click") return rows(dimension)[index].click();
  const search = menu(dimension).querySelector(".fmenu-search");
  for (let at = 0; at < index; at += 1) search.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown" }));
  search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
};
const mount = async (options = {}) => {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, { ...scope, projectKey: "choice-device|choice-project", callRpc: call,
    feed: () => ({ projects: [], items: [], workspaces: [] }), catalog: () => ({ providers: [] }),
    defaultView: "list", navigate() {}, ...options });
  await vi.waitFor(() => expect(call.mock.calls.some(([method]) => method === "tasks.list")).toBe(true), { timeout: 10_000 });
};
const refreshChoices = async () => {
  const before = choiceCalls().length;
  watchers.find(({ entity }) => entity === scope.projectId).refresh();
  await vi.waitFor(() => expect(choiceCalls().length).toBe(before + 4), { timeout: 10_000 });
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  support = { listSummaries: true, listOrder: true, listAttention: true, filterChoices: true };
  session = {}; watchers = [];
  document.body.innerHTML = '<div id="host"></div>';
  host = document.querySelector("#host");
  tasks = Array.from({ length: 30 }, (_unused, index) => task({ id: `task-${30 - index}`, number: 30 - index,
    title: `Task ${30 - index}`, labels: [index === 0 ? nulLabel : index === 1 ? replacementLabel : "cached-only"],
    assignee: index === 0 ? nulActor : index === 1 ? replacementActor : null }));
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks, columns: columns() });
  choices = await import("../src/core/taskFilterChoices.js");
  remote = async ({ dimension, cursor }) => {
    if (dimension === "label") {
      if (cursor === "label-next") return page(dimension, [], "label-empty-next");
      if (cursor === "label-empty-next") return page(dimension, ["later"]);
      return page(dimension, [nulLabel, replacementLabel], "label-next");
    }
    if (dimension === "assignee") return page(dimension, ["any", nulActor, replacementActor], "assignee-next");
    return page(dimension, dimension === "state" ? ["open"] : ["unknown-column"]);
  };
  call = vi.fn(async (method, params) => {
    if (method === "tasks.filterChoices") return remote(params);
    if (method === "tasks.get") return { task: tasks.find(({ id }) => id === params.task_id), timeline: [] };
    if (method !== "tasks.list") return {};
    return { tasks: tasks.filter((row) => (!params.label || row.labels.includes(params.label))
      && (!params.assignee || row.assignee?.agent_id === params.assignee.agent_id)) };
  });
});
afterEach(() => pane?.dispose());

it("loads independent first choice pages without changing legacy list grouping, totals, or pagination", async () => {
  await mount();
  await vi.waitFor(() => expect(choiceCalls()).toHaveLength(4), { timeout: 10_000 });
  await open("label");
  await vi.waitFor(() => expect(labels("label")).toEqual(["remotelabel", replacementLabel]), { timeout: 10_000 });
  expect(labels("label")).not.toContain("cached-only");
  expect(choiceCalls().every(([, params]) => params.limit === 100 && !params.cursor)).toBe(true);
  expect(call.mock.calls.filter(([method]) => method === "tasks.list").every(([, params]) => params.summary === undefined)).toBe(true);
  expect(host.querySelector(".task-group-count").textContent).toBe("0");
  expect(host.querySelector('[data-task-group="rest"] .task-group-count').textContent).toBe("30");
  expect(host.querySelector(".task-paging").textContent).toContain("Showing 25 of 30");
  host.querySelector("[data-task-more]").click();
  await vi.waitFor(() => expect(titles()).toHaveLength(30), { timeout: 10_000 });
  expect(choiceCalls()).toHaveLength(4);
});

it("advances only the chosen menu, including a sparse empty choice page", async () => {
  await mount(); await open("label");
  await vi.waitFor(() => expect(more("label")?.hidden).toBe(false), { timeout: 10_000 });
  more("label").click();
  await vi.waitFor(() => expect(choiceCalls("label")).toHaveLength(2), { timeout: 10_000 });
  await vi.waitFor(async () => expect((await choices.readFilterChoices(scope, "label")).next_cursor)
    .toBe("label-empty-next"), { timeout: 10_000 });
  await vi.waitFor(() => expect(more("label").hidden).toBe(false), { timeout: 10_000 });
  expect(choiceCalls("label")[1][1].cursor).toBe("label-next");
  expect(choiceCalls("assignee")).toHaveLength(1);
  more("label").click();
  await vi.waitFor(() => expect(labels("label")).toContain("later"), { timeout: 10_000 });
  expect(choiceCalls("label")[2][1].cursor).toBe("label-empty-next");
  expect(more("label").hidden).toBe(true);
});

describe.each(["click", "Enter"])("remote opaque choices with %s", (gesture) => {
  it("round-trips a NUL label to dispatch and the selected list", async () => {
    await mount(); await open("label");
    await vi.waitFor(() => expect(labels("label")).toEqual(["remotelabel", replacementLabel]), { timeout: 10_000 });
    select("label", 0, gesture);
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", {
      project_id: scope.projectId, state: "open", label: nulLabel,
    }), { timeout: 10_000 });
    await vi.waitFor(() => expect(titles()).toEqual(["Task 30"]), { timeout: 10_000 });
    expect(menu("label").querySelector(".fmenu-press-label").textContent).toBe(nulLabel);
    expect(choiceCalls()).toHaveLength(4);
  });

  it("round-trips a NUL actor distinctly from its literal replacement-character choice", async () => {
    await mount(); await open("assignee");
    await vi.waitFor(() => expect(rows("assignee").some((row) => row.dataset.key === `option:agent:${nulActor.agent_id}`)).toBe(true), { timeout: 10_000 });
    const index = rows("assignee").findIndex((row) => row.dataset.key === `option:agent:${nulActor.agent_id}`);
    select("assignee", index, gesture);
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", {
      project_id: scope.projectId, state: "open", assignee: nulActor,
    }), { timeout: 10_000 });
    await vi.waitFor(() => expect(titles()).toEqual(["Task 30"]), { timeout: 10_000 });
    expect(rows("assignee").find((row) => row.dataset.key === `option:agent:${replacementActor.agent_id}`)
      .getAttribute("aria-selected")).toBe("false");
  });
});

it("retains a selected value after a canonical refresh removes it from the offered page", async () => {
  await mount(); await open("label");
  await vi.waitFor(() => expect(labels("label")).toEqual(["remotelabel", replacementLabel]), { timeout: 10_000 });
  select("label", 0, "click");
  await vi.waitFor(() => expect(titles()).toEqual(["Task 30"]), { timeout: 10_000 });
  remote = async ({ dimension }) => page(dimension, dimension === "label" ? ["fresh-only"] : []);
  await refreshChoices();
  await vi.waitFor(() => expect(labels("label")).toContain("fresh-only"), { timeout: 10_000 });
  expect(labels("label")).toContain("remotelabel");
  expect(menu("label").querySelector(".fmenu-press-label").textContent).toBe(nulLabel);
  expect(rows("label").find((row) => row.dataset.key === `option:${nulLabel}`).getAttribute("aria-selected")).toBe("true");
});

it("uses the remote label representative for every ASCII-equivalent task without folding Unicode or whitespace", async () => {
  tasks = [task({ id: "upper", number: 4, title: "Upper API", labels: ["API"] }),
    task({ id: "lower", number: 3, title: "Lower api", labels: ["api"] }),
    task({ id: "unicode", number: 2, title: "Unicode", labels: ["É"] }),
    task({ id: "space", number: 1, title: "Whitespace", labels: [" api "] })];
  remote = async ({ dimension }) => page(dimension, dimension === "label" ? ["API", "É", " api "] : []);
  const filtered = new Promise(() => {});
  call.mockImplementation(async (method, params) => {
    if (method === "tasks.filterChoices") return remote(params);
    if (method === "tasks.get") return { task: tasks.find(({ id }) => id === params.task_id), timeline: [] };
    if (method === "tasks.list") return params.label ? filtered : { tasks };
    return {};
  });
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks, columns: columns() });
  await mount(); await open("label");
  await vi.waitFor(() => expect(labels("label")).toContain("API"), { timeout: 10_000 });
  rows("label")[labels("label").indexOf("API")].click();
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", {
    project_id: scope.projectId, state: "open", label: "API",
  }), { timeout: 10_000 });
  await vi.waitFor(() => expect(titles()).toEqual(["Upper API", "Lower api"]), { timeout: 10_000 });
});

it("paints cached choices and continuation without a live capability, while admitting no choice RPC", async () => {
  const ticket = await choices.beginFilterChoices(scope, "label", 1);
  await choices.landFilterChoices(ticket, page("label", ["cached-remote"], "cached-next"));
  support = {};
  await mount(); await open("label");
  await vi.waitFor(() => expect(labels("label")).toEqual(["cached-remote"]), { timeout: 10_000 });
  expect(more("label").hidden).toBe(false);
  more("label").click();
  expect(choiceCalls()).toHaveLength(0);
});

it("keeps a remote pending-only any choice canonical through assignment and unassignment", async () => {
  tasks = [task({ id: "pending", title: "Pending delivery", deferred_assignment: { state: "queued", target: nulActor } })];
  remote = async ({ dimension }) => page(dimension, dimension === "assignee" ? ["any"] : []);
  call.mockImplementation(async (method, params) => {
    if (method === "tasks.filterChoices") return remote(params);
    if (method === "tasks.get") return { task: tasks[0], timeline: [] };
    if (method === "tasks.list") return params.assignee ? new Promise(() => {}) : { tasks };
    return {};
  });
  const tracker = await import("../src/core/trackerCache.js");
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const save = async () => tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord(tasks, columns(), await nextTaskRead()));
  await save(); await mount(); await open("assignee");
  await vi.waitFor(async () => expect((await choices.readFilterChoices(scope, "assignee"))?.complete).toBe(true), { timeout: 10_000 });
  expect(rows("assignee").some((row) => row.dataset.key === `option:agent:${nulActor.agent_id}`)).toBe(false);
  rows("assignee")[labels("assignee").indexOf("Anyone assigned")].click();
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.list", {
    project_id: scope.projectId, state: "open", assignee: "any",
  }), { timeout: 10_000 });
  await vi.waitFor(() => expect(titles()).toEqual(["Pending delivery"]), { timeout: 10_000 });
  tasks = [{ ...tasks[0], assignee: nulActor, deferred_assignment: null, updated_at: "2026-08-21T10:01:00Z" }];
  await save();
  await vi.waitFor(() => expect(titles()).toEqual(["Pending delivery"]), { timeout: 10_000 });
  tasks = [{ ...tasks[0], assignee: null, updated_at: "2026-08-21T10:02:00Z" }];
  await save();
  await vi.waitFor(() => expect(titles()).toEqual([]), { timeout: 10_000 });
});

it("rejects an old-session choice continuation after a new first-page generation", async () => {
  await mount(); await open("label");
  await vi.waitFor(() => expect(more("label")?.hidden).toBe(false), { timeout: 10_000 });
  let settle;
  remote = async ({ dimension, cursor }) => cursor ? new Promise((resolve) => { settle = resolve; })
    : page(dimension, dimension === "label" ? ["new-session"] : []);
  more("label").click();
  await vi.waitFor(() => expect(settle).toBeTypeOf("function"), { timeout: 10_000 });
  session = {};
  await refreshChoices();
  await vi.waitFor(() => expect(labels("label")).toEqual(["new-session"]), { timeout: 10_000 });
  settle(page("label", ["retired-choice"]));
  await vi.waitFor(async () => expect((await choices.readFilterChoices(scope, "label")).choices).toEqual(["new-session"]), { timeout: 10_000 });
  expect(more("label").hidden).toBe(true);
});

it.each(["dashboard", "workspace"])("retains the %s compatibility path without bounded row or choice activation", async (mode) => {
  await mount(mode === "dashboard" ? { defaultView: "dashboard" } : { only: () => true });
  await open("label");
  await vi.waitFor(() => expect(labels("label")).toContain("cached-only"), { timeout: 10_000 });
  expect(choiceCalls()).toHaveLength(0);
  expect(call.mock.calls.filter(([method]) => method === "tasks.list").every(([, params]) => params.summary === undefined)).toBe(true);
});
