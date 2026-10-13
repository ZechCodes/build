import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let support;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  support = await import("../src/core/taskWaitingOnSupport.js");
});

it("remembers only announced support per device, including a later old greeting", async () => {
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  expect(await support.readTaskWaitingOnSupport("d")).toBe(true);
  expect(await support.readTaskWaitingOnSupport("other")).toBe(false);
  await support.rememberTaskWaitingOnSupport("d", { tasks: {} });
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
});

it("does not let a superseded greeting overwrite support", async () => {
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  await support.rememberTaskWaitingOnSupport("d", {}, () => false);
  expect(await support.readTaskWaitingOnSupport("d")).toBe(true);
});
