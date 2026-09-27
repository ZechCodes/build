/** @vitest-environment jsdom */
// #104, unmocked: the real Tasks pane over the real cache. The remote RPC is
// held open, so every bubble on screen has to come from cached records — the
// list's `unread_count` first, then a cached timeline that says it was read.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTasksPane } from "../src/core/trackerTasksPane.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { taskRecord, tasksRecord, writeTaskRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { columns, comment, task } from "./trackerWireFixture.js";

const DEVICE = "wiring-device";
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
  resetChangeEvents();
});

const mount = (projectId, view) => {
  pane = mountTasksPane(host, {
    deviceId: DEVICE,
    projectId,
    projectName: "Build",
    projectKey: projectId,
    view,
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
};

const bubbleOf = (taskId) => host.querySelector(`[data-task="${taskId}"] .task-unread`)?.textContent ?? null;

describe.each(["list", "board", "dashboard"])("the %s", (view) => {
  const projectId = `wiring-${view}`;
  const asked = task({
    id: "asked", number: 4, title: "Look at this", status: "ready", assignee: { kind: "user" },
    watched: true, read_through: "tc-01", unread_count: 2,
  });
  const quiet = task({ id: "quiet", number: 5, title: "Nobody watches", status: "ready", assignee: { kind: "user" }, unread_count: 7 });

  it("wears the bubble a watched task's cached list row says, and loses it once a cached read says so", async () => {
    await writeTasksRecord(DEVICE, projectId, tasksRecord([asked, quiet], columns()));
    mount(projectId, view);
    await vi.waitFor(() => expect(bubbleOf("asked")).toBe("2"));
    expect(host.querySelector('[data-task="quiet"]')).not.toBeNull();
    expect(bubbleOf("quiet")).toBeNull();

    const timeline = [
      comment({ id: "tc-02", author: { kind: "agent", agent_id: "a1" } }),
      comment({ id: "tc-03", author: { kind: "agent", agent_id: "a1" } }),
    ];
    await writeTaskRecord(DEVICE, projectId, "asked", taskRecord({ ...asked, read_through: "tc-03" }, timeline));
    await vi.waitFor(() => expect(bubbleOf("asked")).toBeNull());
  });
});
