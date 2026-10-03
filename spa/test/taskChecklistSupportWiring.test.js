/** @vitest-environment jsdom */
// The real greeting and adapter write the capability the cold task page uses.
import { afterEach, beforeEach, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readFileSync } from "node:fs";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { readTaskChecklistSupport } = await import("../src/core/taskChecklistSupport.js");
const greeting = JSON.parse(readFileSync("../fixtures/api/v1/session.hello.json", "utf8")).result;
const greet = (hello) => greetBridge(async (method) => method === "session.hello" ? hello : {}, { deviceId: "dev-347", strict: true });

beforeEach(() => { resetChangeEvents(); globalThis.indexedDB = new IDBFactory(); });
afterEach(() => resetChangeEvents());

it("remembers safe body writes from the real bridge greeting", async () => {
  expect(greeting.capabilities).toContain("tasks.bodyPrecondition");
  await greet(greeting);
  await expect.poll(() => readTaskChecklistSupport("dev-347")).toBe(true);
  // Session state going away doesn't remove the previously cached capability.
  resetChangeEvents();
  expect(await readTaskChecklistSupport("dev-347")).toBe(true);
});

it("revokes the cached feature when the bridge greeting no longer names it", async () => {
  await greet(greeting);
  await expect.poll(() => readTaskChecklistSupport("dev-347")).toBe(true);
  await greet({ ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "tasks.bodyPrecondition") });
  await expect.poll(() => readTaskChecklistSupport("dev-347")).toBe(false);
});
