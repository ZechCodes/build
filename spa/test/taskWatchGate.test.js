/** @vitest-environment jsdom */
// The watch gate, asked of a bridge that actually greeted (#65).
//
// The shell agent found this hole in their half and it was in mine too, in
// both halves:
//
//   test/trackerWatch.test.js mocks `bridgeApiVersion` wholesale, so it proves
//   the COMPARISON and nothing about the wiring — a renamed export, a greeting
//   that never reaches the store, or a version read off the wrong key would
//   all leave it green. That is exactly the break I hit when this module still
//   exported `carriesWatch` and the shell's call sites imported
//   `carriesWatching`: my own suite said nothing, and only their call sites
//   failed.
//
//   test/taskWatchDom.test.js drives the gate by mocking THIS module, so
//   "the page draws no switch without watching" is asserted through a stand-in and
//   would go on passing with the gate deleted.
//
// So nothing is mocked here but the reconnect watcher: a real greeting goes
// into the real store, the real gate reads it, and the real page is mounted on
// top.
//
// Since ea3de439 the gate is a capability rather than a version compare, and
// since API 2.0.0 a capability is a name in the greeting's `capabilities`
// list and nothing else: no minor stands in for it, and the old
// `tasks: { watching }` boolean is not read. The case that states the old
// boolean against the list is the only one that can tell a flag read where the
// bridge writes it from a flag read somewhere else.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, task } from "./trackerWireFixture.js";

vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeApiVersion, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { carriesWatching } = await import("../src/core/trackerWatch.js");
const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
const { writeTasksRecord } = await import("../src/core/trackerCache.js");

/** The capabilities a bridge that carries watching names, and one that does not. */
const WATCHING = ["tasks.watching"];
const NO_WATCHING = [];

/** One machine saying what it is, through the real greeting path.
 *
 *  `capabilities` is the list the greeting names; `states` is the old
 *  `tasks: { watching }` boolean, which a 2.x greeting may still carry but the
 *  adapter must not read. */
const greet = (capabilities, { deviceId = "dev-1", states } = {}) =>
  greetBridge(
    async () => ({
      push_events: true,
      api_version: "2.0.0",
      capabilities,
      ...(states === undefined ? {} : { tasks: { watching: states } }),
    }),
    { deviceId },
  );

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

let host, call, page;
const answer = () => ({
  task: task({ id: "task-1", number: 65, watched: false, trackers: ["agent-1"] }),
  timeline: [comment({ id: "tc-1", created_at: "2026-09-21T10:01:00Z", body: "The first word." })],
});

const mount = async (deviceId = "dev-1") => {
  page = mountTaskPage(host, {
    projectId: "proj-1", deviceId, projectKey: `${deviceId}|proj-1`, taskId: "task-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    navigate: vi.fn(),
  });
  await flush();
  return page;
};

const listed = (method) => call.mock.calls.filter(([name]) => name === method);

beforeEach(async () => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  await writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
  await writeTasksRecord("dev-old", "proj-1", { tasks: [], columns: columns() });
  call = vi.fn(async (method) => (method === "tasks.get" ? answer() : {}));
});

afterEach(() => {
  page?.dispose();
  resetChangeEvents();
});

describe("the gate, asked of a bridge that actually greeted", () => {
  it("offers watching to a machine whose greeting names tasks.watching", async () => {
    await greet(WATCHING);
    expect(bridgeApiVersion("dev-1")).toBe("2.0.0");
    expect(carriesWatching("dev-1")).toBe(true);
  });

  it("refuses a machine whose greeting does not name it", async () => {
    await greet(NO_WATCHING);
    expect(carriesWatching("dev-1")).toBe(false);
  });

  // Not a mocked null: "0.0.0" is what a machine really reads as before it has
  // ever been greeted, which is the state every one of them starts in.
  it("refuses a machine that has never greeted at all", () => {
    expect(bridgeApiVersion("dev-1")).toBe("0.0.0");
    expect(carriesWatching("dev-1")).toBe(false);
  });

  // The case that tells a flag read where the bridge writes it from one read
  // elsewhere: the old boolean says the opposite of the list, and only the
  // list may answer. Under the wrong key this case is the one that fails, and
  // every other case on this page passes either way.
  it("reads the capabilities list over the old tasks.watching flag, in both directions", async () => {
    await greet(WATCHING, { states: false });
    expect(carriesWatching("dev-1")).toBe(true);

    await greet(NO_WATCHING, { deviceId: "dev-stated", states: true });
    expect(carriesWatching("dev-stated")).toBe(false);
  });

  // A phone is paired to more than one machine. The answer is per machine, or
  // the switch appears on a bridge that cannot serve it.
  it("answers per machine when two are paired at once", async () => {
    await greet(WATCHING, { deviceId: "dev-new" });
    await greet(NO_WATCHING, { deviceId: "dev-old" });
    expect([carriesWatching("dev-new"), carriesWatching("dev-old")]).toEqual([true, false]);
  });
});

describe("the page, on top of that gate", () => {
  it("draws no switch and marks nothing read on a bridge that does not name watching", async () => {
    await greet(NO_WATCHING);
    await mount();
    expect(host.querySelector(".rail-watch")).toBeNull();
    expect(listed("tasks.read_through")).toHaveLength(0);
  });

  it("draws the switch and marks its read once the machine carries it", async () => {
    await greet(WATCHING);
    await mount();
    expect(host.querySelector(".rail-watch")).not.toBeNull();
    expect(listed("tasks.read_through")[0][1]).toEqual({ task_id: "task-1", event_id: "tc-1" });
  });

  it("and presses through to the verb itself", async () => {
    await greet(WATCHING);
    await mount();
    host.querySelector(".rail-watch").click();
    await flush();
    expect(listed("tasks.watch")[0][1]).toEqual({ task_id: "task-1" });
  });

  it("draws no switch for a bridge that states only the old tasks.watching flag", async () => {
    await greet(NO_WATCHING, { states: true });
    await mount();
    expect(host.querySelector(".rail-watch")).toBeNull();
  });

  // The same page mounted against the one of two paired machines without it.
  it("is dark on the machine that does not name watching while the other one does", async () => {
    await greet(WATCHING);
    await greet(NO_WATCHING, { deviceId: "dev-old" });
    await mount("dev-old");
    expect(host.querySelector(".rail-watch")).toBeNull();
  });
});
