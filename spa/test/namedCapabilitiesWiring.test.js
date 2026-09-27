/** @vitest-environment jsdom */
// Keep the bridge's real session.hello and the SPA's greeting, adapter, gate,
// and task page on one path. A names-only greeting must reach the watch
// control without a minor-version guess making up a missing capability.

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, task } from "./trackerWireFixture.js";
import versions from "../../fixtures/api/versions.json";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeCapabilities, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { carriesWatching } = await import("../src/core/trackerWatch.js");
const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
const { writeTasksRecord } = await import("../src/core/trackerCache.js");

const run = promisify(execFile);
const bridgeRoot = resolve(process.cwd(), "../bridge");
const taskAnswer = {
  task: task({ id: "task-1", number: 122, watched: false }),
  timeline: [comment({ id: "tc-1", task_id: "task-1" })],
};
let realGreeting;
let host;
let page;
let calls;

beforeAll(async () => {
  // The Rust test calls the real FrameHandler and prints the wire envelope.
  // Generate it on demand so plain `npx vitest run` needs no prepared file.
  const { stdout } = await run("cargo", [
    "test", "--lib", "the_greeting_and_the_probe_report_the_api_version", "--", "--nocapture",
  ], {
    cwd: bridgeRoot,
    env: { ...process.env, BUILD_PRINT_HELLO_CAPABILITIES: "1" },
    maxBuffer: 2 * 1024 * 1024,
    timeout: 600_000,
  });
  const marked = stdout.split(/\r?\n/).find((line) => line.includes("BUILD_REAL_HELLO="));
  expect(marked, "Rust must emit the actual session.hello reply").toBeDefined();
  const envelope = JSON.parse(marked.slice(marked.indexOf("BUILD_REAL_HELLO=") + "BUILD_REAL_HELLO=".length));
  expect(envelope.ok).toBe(true);
  realGreeting = envelope.result;
  expect(realGreeting.api_version).toBe(versions.current);
  expect(realGreeting.capabilities).toContain("tasks.watching");
}, 610_000);

beforeEach(async () => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="task"></div>';
  host = document.querySelector("#task");
  await writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
  calls = [];
  page = null;
});

afterEach(() => {
  page?.dispose();
  resetChangeEvents();
});

async function mountWith(greeting) {
  const call = async (method, params) => {
    calls.push([method, params]);
    if (method === "session.hello") return greeting;
    if (method === "tasks.get") return taskAnswer;
    return {};
  };
  await greetBridge(call, { deviceId: "dev-1", strict: true });
  page = mountTaskPage(host, {
    projectId: "proj-1", deviceId: "dev-1", projectKey: "dev-1|proj-1", taskId: "task-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    navigate: () => {},
  });
  await vi.waitFor(() => expect(host.querySelector(".task-page-title")).not.toBeNull());
}

it("renders and wires watching from the real names-only bridge greeting", async () => {
  await mountWith(realGreeting);
  expect(bridgeCapabilities("dev-1").tasks.watching).toBe(true);
  expect(carriesWatching("dev-1")).toBe(true);
  const button = host.querySelector(".rail-watch");
  expect(button).not.toBeNull();
  button.click();
  await vi.waitFor(() => expect(calls).toContainEqual(["tasks.watch", { task_id: "task-1" }]));
});

it("hides watching when the same bridge version omits its name", async () => {
  const greeting = {
    ...realGreeting,
    capabilities: realGreeting.capabilities.filter((name) => name !== "tasks.watching"),
  };
  await mountWith(greeting);
  expect(bridgeCapabilities("dev-1").tasks.watching).toBe(false);
  expect(carriesWatching("dev-1")).toBe(false);
  expect(host.querySelector(".rail-watch")).toBeNull();
  expect(calls.some(([method]) => method === "tasks.read_through")).toBe(false);
});
