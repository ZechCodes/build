/** @vitest-environment jsdom */
// The Tasks face on the workspace's rail: its count, and whether it is drawn.
//
// The count reads the project's cached task list and listens to that record,
// so an `tasks` push moves the badge on the rail with nothing asked of the
// bridge.
//
// It no longer opens an overlay (#16 → #29), and it no longer owns its press
// (#174): it is a face of the rail, and the rail's own press goes to the
// workspace's Tasks tab. What that tab shows is tested in
// trackerWorkspaceTasksTab.test.js and workspaceViewDom.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

// What the bridge's greeting said. The face never reads it (#104 review), so a
// test that empties it proves the paint comes off the cache alone.
const CARRIES_TASKS = { changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } };
const NO_GREETING = { changes: { subscriptions: false, kinds: [] } };
let greeting = CARRIES_TASKS;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => greeting,
}));

const ONE = "agent-01M2ONE";
const TWO = "agent-01M2TWO";
const ELSEWHERE = "agent-01M2ELSE";

let button, trackerCache, mountWorkspaceTasks, block, agents;

const held = (agentId, over = {}) => task({ assignee: { kind: "agent", agent_id: agentId }, ...over });
const putTasks = (tasks) => trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });

// The badge settles on a cache read; every test waits for what it says rather
// than for a count of ticks.
const mount = (over = {}) => {
  block = mountWorkspaceTasks(button, {
    deviceId: "dev-1",
    projectId: "proj-1",
    workspaceId: "ws-1",
    agents: () => agents,
    ...over,
  });
  return block;
};

const badge = () => button.querySelector(".dirtab-count").textContent;
const cellHtml = '<button data-tab="tasks" hidden><span class="badge dirtab-count"></span></button>';
const overlay = () => document.querySelector(".modal-workspace-tasks");
const agentSections = () =>
  [...document.querySelectorAll("[data-tasks-agent]")].map((one) => one.querySelector("h3").textContent);
const cardsUnder = (label) => {
  const section = [...document.querySelectorAll("[data-tasks-agent]")]
    .find((one) => one.querySelector("h3").textContent === label);
  return [...(section?.querySelectorAll(".agent-task-card") || [])].map(
    (one) => one.querySelector(".task-number").textContent,
  );
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  greeting = CARRIES_TASKS;
  agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
  document.body.innerHTML = cellHtml;
  button = document.querySelector("[data-tab=tasks]");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountWorkspaceTasks } = await import("../src/core/trackerWorkspaceTasksView.js"));
});

afterEach(() => {
  block?.dispose();
  block = null;
  document.body.innerHTML = "";
});

describe("the badge", () => {
  // #104: the tab carries the unread of the watched tasks this workspace's
  // agents hold. A task nobody watches never shows a count, and neither does
  // a finished one (#183), though a 1.29 list carries it.
  it("counts the unread of the unfinished watched tasks this workspace's agents hold", async () => {
    await putTasks([
      held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 2 }),
      held(TWO, { number: 2, id: "i2", status: "done", watched: true, unread_count: 1 }),
      held(ONE, { number: 3, id: "i3", status: "ready", unread_count: 5 }),
      held(ELSEWHERE, { number: 4, id: "i4", status: "in_progress", watched: true, unread_count: 4 }),
    ]);
    mount();
    await vi.waitFor(() => expect(badge()).toBe("2"));
    expect(button.hidden).toBe(false);
    expect(button.title).toBe("2 unread · 2 open tasks in this workspace");
  });

  it("says nothing when nothing is unread, but keeps the way in and says what is open", async () => {
    await putTasks([held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 0 })]);
    mount();
    await vi.waitFor(() => expect(button.title).toBe("1 open task in this workspace"));
    expect(badge()).toBe("");
    expect(button.hidden).toBe(false);
  });

  // Paint from cache: a cold or offline start has no greeting yet, or a bridge
  // that is gone. The face and its count come off the cached list all the same.
  it("paints the cached count with no greeting from the bridge", async () => {
    greeting = NO_GREETING;
    await putTasks([held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 2 })]);
    mount();
    await vi.waitFor(() => expect(badge()).toBe("2"));
    expect(button.hidden).toBe(false);
    expect(button.title).toBe("2 unread · 1 open task in this workspace");
  });

  it("is not drawn on a route that names no project", () => {
    button.hidden = false;
    mount({ projectId: null });
    expect(button.hidden).toBe(true);
    expect(badge()).toBe("");
  });

  it("moves on the push, with nothing asked of the bridge", async () => {
    await putTasks([held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 1 })]);
    mount();
    await vi.waitFor(() => expect(badge()).toBe("1"));
    await putTasks([
      held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 0 }),
      held(TWO, { number: 2, id: "i2", status: "in_progress", watched: true, unread_count: 2 }),
    ]);
    await vi.waitFor(() => expect(badge()).toBe("2"));
  });

  // A workspace gains and loses agents while the bar stands there.
  it("re-reads the agents when the bar says they moved", async () => {
    await putTasks([
      held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 0 }),
      held(TWO, { number: 2, id: "i2", status: "in_progress", watched: true, unread_count: 1 }),
    ]);
    agents = [{ id: ONE, ordinal: 1 }];
    mount();
    await vi.waitFor(() => expect(button.title).toBe("1 open task in this workspace"));
    expect(badge()).toBe("");
    agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
    block.refresh();
    expect(badge()).toBe("1");
  });
});

describe("the rail's cell", () => {
  // The rail's own press goes to the Tasks tab; the badge block wires none.
  it("leaves the press to the rail", async () => {
    await putTasks([held(ONE, { number: 1, id: "i1", status: "in_progress" })]);
    mount();
    await vi.waitFor(() => expect(button.classList.contains("has-tasks")).toBe(true));
    expect(button.onclick).toBeNull();
    button.click();
    // The settle point: a cache round trip after the press, which the block
    // answers by repainting. Whatever the press set going has had one too.
    await putTasks([held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 1 })]);
    await vi.waitFor(() => expect(badge()).toBe("1"));
    expect(overlay()).toBeNull();
    expect(document.querySelector("dialog")).toBeNull();
  });

  // A paint of the rail rewrites its cells: the block moves onto the new one
  // and says the count it already holds, with nothing read again.
  it("follows the rail onto a repainted cell, keeping its count", async () => {
    await putTasks([held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 1 })]);
    mount();
    await vi.waitFor(() => expect(badge()).toBe("1"));
    document.body.innerHTML = cellHtml;
    button = document.querySelector("[data-tab=tasks]");
    block.retarget(button);
    expect(badge()).toBe("1");
    expect(button.hidden).toBe(false);
  });
});
