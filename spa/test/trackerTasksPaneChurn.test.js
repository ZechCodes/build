/** @vitest-environment jsdom */
// #119, the cause the maintainer saw: pushes arriving faster than an
// `tasks.list` round trip. Each push used to start a fresh read and drop every
// answer a newer read had overtaken, so while the agents kept commenting no
// answer ever landed and a Done task stayed under "Needs you". Real pane and
// cache; the push is the pane's own registration, fired the way
// core/changeEvents.js fires it.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: (registration) => {
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-119";
const PROJECT = "proj-1";
const reviewing = task({ id: "task-101", number: 101, title: "Wire 1.14.0", status: "in_review" });
const moved = { ...reviewing, status: "done" };

let host, pane, answers, reads;
const push = () => watchers.filter((one) => one.entity === PROJECT).forEach((one) => one.refresh());
const needsYouTitles = () => [...host.querySelectorAll('[data-task-group="needsYou"] .task-title')]
  .map((one) => one.textContent);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  watchers = [];
  reads = 0;
  // Each read waits until the case answers it, as a slow wire does.
  answers = [];
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
  const trackerCache = await import("../src/core/trackerCache.js");
  await trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord([reviewing], columns()));
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectName: "Build",
    projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: "list",
    callRpc: (method) => {
      if (method !== "tasks.list") return new Promise(() => {});
      reads += 1;
      return new Promise((resolve) => answers.push(resolve));
    },
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
});

afterEach(() => pane?.dispose());

it("lands the read that is out while pushes keep coming, and reads once more after it", async () => {
  await vi.waitFor(() => expect(answers).toHaveLength(1)); // the mount's own read
  answers[0]({ tasks: [reviewing] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual(["Wire 1.14.0"]));

  push(); // the move to Done
  await vi.waitFor(() => expect(answers).toHaveLength(2));
  push(); // and the comments after it, each before the read has come back
  push();
  push();
  expect(reads).toBe(2);

  answers[1]({ tasks: [moved] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual([]));

  // One read after the burst, not one per push.
  await vi.waitFor(() => expect(answers).toHaveLength(3));
  expect(reads).toBe(3);
  answers[2]({ tasks: [moved] });
});
