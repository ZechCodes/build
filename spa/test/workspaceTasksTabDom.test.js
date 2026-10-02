/** @vitest-environment jsdom */
// The workspace's Tasks tab, mounted (#29): the whole tracker, showing only
// the tasks the agents standing in THIS workspace are holding.
//
// It is the real list and the real board and the real task page — the point of
// the tab is that it is not a summary — so this file mounts them for real and
// checks the narrowing, the routes out of it, and the stamp the task page
// leaves for the agent beside it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, event, task } from "./trackerWireFixture.js";

let watchers = [];
// The real module refuses a registration that names a cadence — nothing in
// this client polls — so this stand-in refuses one too.
const RETIRED = ["intervalMs", "keepPolling", "catchUpOnVisible"];
let apiVersion = "1.5.0";
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "tasks"] } }),
  bridgeApiVersion: () => apiVersion,
  watchChanges: (registration) => {
    const named = RETIRED.filter((option) => option in registration);
    if (named.length) throw new TypeError(`watchChanges does not poll: remove ${named.join(", ")}`);
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));

vi.mock("../src/core/trackerAssigneePicker.js", () => ({ openAssigneePicker: vi.fn(() => ({ close: vi.fn(), setCatalog: vi.fn() })) }));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const PROJECT_KEY = "dev-1/proj-1";
const HERE = "agent-01M2HERE";
const ALSO = "agent-01M2ALSO";
const AWAY = "agent-01M2AWAY";

const freshFeed = () => ({
  workspaces: [
    { id: "ws-1", workspace_id: "ws-1", name: "tasks-spa", projectKey: PROJECT_KEY, entity_id: "run-1" },
    { id: "ws-2", workspace_id: "ws-2", name: "elsewhere", projectKey: PROJECT_KEY, entity_id: "run-2" },
  ],
  items: [
    { kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: HERE, ordinal: 1 }, { id: ALSO, ordinal: 2 }] },
    { kind: "branch", projectKey: PROJECT_KEY, run_id: "run-2", agents: [{ id: AWAY, ordinal: 1 }] },
  ],
});

/** This case's feed. Rebuilt every time, because one case moves the agents and
 *  a shared object would carry that into the next. */
let feed;

const held = (agentId, over = {}) => task({ assignee: { kind: "agent", agent_id: agentId }, ...over });

/** The project's whole list: two tasks held here, one held in another
 *  workspace, one nobody holds. Only the first two belong on this tab. */
const PROJECT_TASKS = [
  held(HERE, { number: 1, id: "i1", title: "Held by an agent here", status: "in_progress" }),
  held(ALSO, { number: 2, id: "i2", title: "Held by another agent here", status: "ready" }),
  held(AWAY, { number: 3, id: "i3", title: "Held in another workspace", status: "in_progress" }),
  task({ number: 4, id: "i4", title: "Held by nobody", assignee: null, status: "backlog" }),
];

let body, trackerCache, mountWorkspaceTasksTab, tab, navigated, stamped, call;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const route = (over = {}) => ({
  name: "workspace", deviceId: "dev-1", projectId: "proj-1", workspaceId: "ws-1", tab: "tasks", ...over,
});

/** The tab, mounted and not yet waited on: a case that can name what it is
 *  waiting for waits for that instead of a count of turns. */
const mountTab = (over = {}) => {
  tab = mountWorkspaceTasksTab(body, {
    route: route(over.route),
    context: { deviceId: "dev-1", rpc: call, modelCatalog: () => ({ providers: [] }), refreshModelCatalog: async () => ({ providers: [] }) },
    feed: () => feed,
    selection: null,
    navigate: (to) => navigated.push(to),
    sayWhichTask: (read) => stamped.push(read),
  });
  return tab;
};

const mount = async (over = {}) => {
  mountTab(over);
  await flush();
  return tab;
};

const titles = () => [...body.querySelectorAll(".task-title")].map((one) => one.textContent);
const numbers = () => [...body.querySelectorAll(".task-number")].map((one) => one.textContent);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  watchers = [];
  feed = freshFeed();
  navigated = [];
  stamped = [];
  apiVersion = "1.5.0";
  document.body.innerHTML = '<div id="tabbody"></div>';
  body = document.querySelector("#tabbody");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountWorkspaceTasksTab } = await import("../src/core/workspaceTasksTab.js"));
  await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: PROJECT_TASKS, columns: columns() });
  call = vi.fn(async (method) => {
    if (method === "tasks.list") return { tasks: PROJECT_TASKS };
    if (method === "tasks.get") {
      return {
        task: held(HERE, { number: 1, id: "i1", title: "Held by an agent here", status: "in_progress" }),
        timeline: [event({ id: "te-1", kind: "created", at: "2026-08-21T10:00:00Z" })],
      };
    }
    return {};
  });
});

afterEach(() => {
  tab?.dispose?.();
  tab = null;
});

describe("the list", () => {
  it("pages the workspace-scoped cached list", async () => {
    const tasks = Array.from({ length: 28 }, (_, index) => held(HERE, {
      id: `held-${28 - index}`, number: 28 - index, title: `Held ${28 - index}`,
    }));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    await mount();
    expect(titles()).toHaveLength(25);
    body.querySelector("[data-task-more]").click();
    await vi.waitFor(() => expect(titles()).toHaveLength(28));
    expect(titles()[27]).toBe("Held 1");
  });

  it("shows only the tasks this workspace's agents are holding", async () => {
    await mount();
    // Newest number first, the same order the project's own Tasks tab uses.
    expect(titles()).toEqual(["Held by another agent here", "Held by an agent here"]);
  });

  // The narrowing is not on the wire: `tasks.list` takes one assignee and a
  // workspace has several, so the tab asks for the project's list and keeps
  // what belongs to it.
  it("asks the bridge for the project's list, not for one agent's", async () => {
    await mount();
    const [, params] = call.mock.calls.find(([method]) => method === "tasks.list");
    expect(params.project_id).toBe("proj-1");
    expect(params.assignee).toBeUndefined();
  });

  it("opens each task inside the workspace, not on the project's own page", async () => {
    await mount();
    const row = [...body.querySelectorAll(".task-row")].find((one) => one.dataset.task === "i1");
    const href = row.querySelector(".task-row-open").getAttribute("href");
    expect(href).toBe("#/device/dev-1/project/proj-1/workspace/ws-1/tasks/i1");
  });

  it("draws the board over the same narrowed set", async () => {
    await mount({ route: { view: "board" } });
    expect(numbers().sort()).toEqual(["#1", "#2"]);
    expect(body.querySelector(".task-board")).not.toBeNull();
  });

  // A workspace gains and loses agents while the tab stands there, and the
  // tasks follow them.
  it("re-reads the agents rather than capturing them", async () => {
    feed.items[0].agents = [{ id: HERE, ordinal: 1 }];
    await mount();
    expect(titles()).toEqual(["Held by an agent here"]);
  });

  it("re-derives workspace tasks when an initially empty cached roster arrives", async () => {
    feed.items[0].agents = [];
    await mount();
    expect(titles()).toEqual([]);

    feed.items[0].agents = [{ id: HERE, ordinal: 1 }, { id: ALSO, ordinal: 2 }];
    tab.feedMoved();

    expect(titles()).toEqual(["Held by another agent here", "Held by an agent here"]);
  });

  it("shows nothing at all for a workspace whose agents hold nothing", async () => {
    await mount({ route: { workspaceId: "ws-9" } });
    expect(titles()).toEqual([]);
  });
});

describe("one task, opened inside the tab", () => {
  it("is the tracker's own page, with its timeline and composer", async () => {
    await mount({ route: { taskId: "i1" } });
    expect(body.querySelector(".task-page-title").textContent).toBe("Held by an agent here");
    expect(body.querySelector("[data-task-composer]")).not.toBeNull();
    expect(body.querySelector(".task-entry")).not.toBeNull();
  });

  // #21 + #29: the agent beside the task is told which task it is. The
  // workspace half of the stamp is the rail's — it is standing on this
  // workspace, so anything sent from here to the project's agent already wears
  // the workspace item (core/agentRail.js).
  it("says which task is open, from the read rather than from the route", async () => {
    await mount({ route: { taskId: "i1" } });
    expect(stamped.map((one) => [one.number, one.title])).toEqual([[1, "Held by an agent here"]]);
  });
});

// #117: the tab is narrowed to this workspace, so it offers the way out to the
// project's whole list — the same arrow the chat overview's workspace scope
// wears to reach every workspace.
describe("the way out to the project's tasks", () => {
  it("links the list to the project's Tasks view", async () => {
    const { routeFromHash } = await import("../src/core/router.js");
    mountTab();
    await vi.waitFor(() => expect(titles()).toEqual(["Held by another agent here", "Held by an agent here"]));
    const out = body.querySelector(".task-head a.scope-link.task-scope-out");
    expect(out.textContent.trim()).toBe("All project tasks");
    expect(out.querySelector("svg.lucide-arrow-up-right")).not.toBeNull();
    expect(out.nextElementSibling.matches("[data-task-new]")).toBe(true);
    expect(routeFromHash(out.getAttribute("href"))).toMatchObject({
      name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks",
    });
    const before = window.location.href;
    const moved = new Promise((done) => window.addEventListener("hashchange", done, { once: true }));
    out.click();
    await moved;
    expect(routeFromHash(window.location.hash)).toMatchObject({ name: "project", projectId: "proj-1", tab: "tasks" });
    window.history.replaceState({}, "", before);
  });

  it("is not on a task's own page inside the tab", async () => {
    mountTab({ route: { taskId: "i1" } });
    await vi.waitFor(() => expect(body.querySelector(".task-page-title")?.textContent).toBe("Held by an agent here"));
    await vi.waitFor(() => expect(body.querySelector("[data-task-composer]")).not.toBeNull());
    expect(body.querySelector(".scope-link")).toBeNull();
  });
});
