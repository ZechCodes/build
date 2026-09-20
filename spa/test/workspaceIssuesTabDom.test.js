/** @vitest-environment jsdom */
// The workspace's Issues tab, mounted (#29): the whole tracker, showing only
// the issues the agents standing in THIS workspace are holding.
//
// It is the real list and the real board and the real issue page — the point of
// the tab is that it is not a summary — so this file mounts them for real and
// checks the narrowing, the routes out of it, and the stamp the issue page
// leaves for the agent beside it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, event, issue } from "./trackerWireFixture.js";

let watchers = [];
// The real module refuses a registration that names a cadence — nothing in
// this client polls — so this stand-in refuses one too.
const RETIRED = ["intervalMs", "keepPolling", "catchUpOnVisible"];
let apiVersion = "1.5.0";
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "issues"] } }),
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
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const PROJECT_KEY = "dev-1/proj-1";
const HERE = "agent-01M2HERE";
const ALSO = "agent-01M2ALSO";
const AWAY = "agent-01M2AWAY";

const freshFeed = () => ({
  workspaces: [
    { id: "ws-1", workspace_id: "ws-1", name: "issues-spa", projectKey: PROJECT_KEY, entity_id: "run-1" },
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

const held = (agentId, over = {}) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });

/** The project's whole list: two issues held here, one held in another
 *  workspace, one nobody holds. Only the first two belong on this tab. */
const PROJECT_ISSUES = [
  held(HERE, { number: 1, id: "i1", title: "Held by an agent here", status: "in_progress" }),
  held(ALSO, { number: 2, id: "i2", title: "Held by another agent here", status: "ready" }),
  held(AWAY, { number: 3, id: "i3", title: "Held in another workspace", status: "in_progress" }),
  issue({ number: 4, id: "i4", title: "Held by nobody", assignee: null, status: "backlog" }),
];

let body, trackerCache, mountWorkspaceIssuesTab, tab, navigated, stamped, call;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const route = (over = {}) => ({
  name: "workspace", deviceId: "dev-1", projectId: "proj-1", workspaceId: "ws-1", tab: "issues", ...over,
});

const mount = async (over = {}) => {
  tab = mountWorkspaceIssuesTab(body, {
    route: route(over.route),
    context: { deviceId: "dev-1", rpc: call, modelCatalog: () => ({ providers: [] }), refreshModelCatalog: async () => ({ providers: [] }) },
    feed: () => feed,
    selection: null,
    navigate: (to) => navigated.push(to),
    sayWhichIssue: (read) => stamped.push(read),
  });
  await flush();
  return tab;
};

const titles = () => [...body.querySelectorAll(".issue-title")].map((one) => one.textContent);
const numbers = () => [...body.querySelectorAll(".issue-number")].map((one) => one.textContent);

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
  ({ mountWorkspaceIssuesTab } = await import("../src/core/workspaceIssuesTab.js"));
  await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: PROJECT_ISSUES, columns: columns() });
  call = vi.fn(async (method) => {
    if (method === "issues.list") return { issues: PROJECT_ISSUES };
    if (method === "issues.get") {
      return {
        issue: held(HERE, { number: 1, id: "i1", title: "Held by an agent here", status: "in_progress" }),
        timeline: [event({ id: "ie-1", kind: "created", at: "2026-08-21T10:00:00Z" })],
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
  it("shows only the issues this workspace's agents are holding", async () => {
    await mount();
    // Newest number first, the same order the project's own Issues tab uses.
    expect(titles()).toEqual(["Held by another agent here", "Held by an agent here"]);
  });

  // The narrowing is not on the wire: `issues.list` takes one assignee and a
  // workspace has several, so the tab asks for the project's list and keeps
  // what belongs to it.
  it("asks the bridge for the project's list, not for one agent's", async () => {
    await mount();
    const [, params] = call.mock.calls.find(([method]) => method === "issues.list");
    expect(params.project_id).toBe("proj-1");
    expect(params.assignee).toBeUndefined();
  });

  it("opens each issue inside the workspace, not on the project's own page", async () => {
    await mount();
    const row = [...body.querySelectorAll(".issue-row")].find((one) => one.dataset.issue === "i1");
    const href = row.querySelector(".issue-row-open").getAttribute("href");
    expect(href).toBe("#/device/dev-1/project/proj-1/workspace/ws-1/issues/i1");
  });

  it("draws the board over the same narrowed set", async () => {
    await mount({ route: { view: "board" } });
    expect(numbers().sort()).toEqual(["#1", "#2"]);
    expect(body.querySelector(".issue-board")).not.toBeNull();
  });

  // A workspace gains and loses agents while the tab stands there, and the
  // issues follow them.
  it("re-reads the agents rather than capturing them", async () => {
    feed.items[0].agents = [{ id: HERE, ordinal: 1 }];
    await mount();
    expect(titles()).toEqual(["Held by an agent here"]);
  });

  it("shows nothing at all for a workspace whose agents hold nothing", async () => {
    await mount({ route: { workspaceId: "ws-9" } });
    expect(titles()).toEqual([]);
  });
});

describe("one issue, opened inside the tab", () => {
  it("is the tracker's own page, with its timeline and composer", async () => {
    await mount({ route: { issueId: "i1" } });
    expect(body.querySelector(".issue-page-title").textContent).toBe("Held by an agent here");
    expect(body.querySelector("[data-issue-composer]")).not.toBeNull();
    expect(body.querySelector(".issue-entry")).not.toBeNull();
  });

  // #21 + #29: the agent beside the issue is told which issue it is. The
  // workspace half of the stamp is the rail's — it is standing on this
  // workspace, so anything sent from here to the project's agent already wears
  // the workspace item (core/agentRail.js).
  it("says which issue is open, from the read rather than from the route", async () => {
    await mount({ route: { issueId: "i1" } });
    expect(stamped.map((one) => [one.number, one.title])).toEqual([[1, "Held by an agent here"]]);
  });
});
