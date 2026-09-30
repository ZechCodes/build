/** @vitest-environment jsdom */
// #273, with nothing mocked between the greeting and the ask: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) names `workspace.measure_sizes`; the real greeting path writes
// that to the real cache, which the Workspaces tab reads to show a placeholder
// for a size still to come, and the tab's ask goes to a machine only once its
// greeting names the verb.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { askForWorkspaceSizes } = await import("../src/core/workspaceSizes.js");
const { readWorkspaceSizeSupport } = await import("../src/core/workspaceSizeSupport.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const olderGreeting = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "workspace.measure_sizes") };

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
});

afterEach(() => resetChangeEvents());

const greet = (hello) => greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });
const flush = () => new Promise((settle) => setTimeout(settle, 0));
const sizeAsks = (call) => call.mock.calls.filter(([method]) => method === "workspace.measure_sizes");

it("remembers that the machine measures sizes once its real greeting names the verb", async () => {
  expect(greeting.capabilities).toContain("workspace.measure_sizes");
  await greet(greeting);
  await vi.waitFor(async () => expect(await readWorkspaceSizeSupport("dev-1")).toBe(true));
});

it("remembers that an older machine does not", async () => {
  await greet(olderGreeting);
  await flush();
  expect(await readWorkspaceSizeSupport("dev-1")).toBe(false);
});

it("asks a greeted machine once for the project's workspaces", async () => {
  await greet(greeting);
  const call = vi.fn(async () => ({ queued: [] }));

  askForWorkspaceSizes("dev-1", call, "proj-1");
  await flush();
  await greet(greeting);
  await flush();

  expect(sizeAsks(call)).toEqual([["workspace.measure_sizes", { project_id: "proj-1" }]]);
});

it("never asks a machine whose greeting does not name the verb", async () => {
  await greet(olderGreeting);
  const call = vi.fn(async () => ({}));

  askForWorkspaceSizes("dev-1", call, "proj-1");
  await flush();

  expect(sizeAsks(call)).toEqual([]);
});

it("asks a machine not greeted yet when it greets", async () => {
  const call = vi.fn(async () => ({ queued: [] }));
  askForWorkspaceSizes("dev-1", call, "proj-1");
  await flush();
  expect(sizeAsks(call)).toEqual([]);

  await greet(greeting);
  await flush();

  expect(sizeAsks(call)).toHaveLength(1);
});

it("asks again at the next greeting when the ask did not reach the machine", async () => {
  await greet(greeting);
  const call = vi.fn().mockRejectedValueOnce(new Error("away")).mockResolvedValue({ queued: [] });

  askForWorkspaceSizes("dev-1", call, "proj-1");
  await flush();
  await greet(greeting);
  await flush();

  expect(sizeAsks(call)).toHaveLength(2);
});

it("does not ask once the tab that wanted the sizes has closed", async () => {
  const call = vi.fn(async () => ({ queued: [] }));
  askForWorkspaceSizes("dev-1", call, "proj-1").stop();

  await greet(greeting);
  await flush();

  expect(sizeAsks(call)).toEqual([]);
});
