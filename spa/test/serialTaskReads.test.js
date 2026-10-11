import { expect, it, vi } from "vitest";
import { createTaskReadQueue } from "../src/core/serialTaskReads.js";
import { trailingRead } from "../src/core/trailingRead.js";

const deferred = () => {
  let resolve;
  let reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

it("keeps a push reread between a held first page and its queued continuation", async () => {
  const serialize = createTaskReadQueue();
  const held = deferred();
  const events = [];
  const pushRead = trailingRead((key) => serialize(key, "session1", async () => { events.push("push reread"); }));
  const first = serialize("p1", "session1", () => { events.push("first page"); return held.promise; });
  const push = pushRead("p1");
  const rest = serialize("p1", "session1", async () => { events.push("remaining pages"); });
  expect(events).toEqual(["first page"]);
  held.resolve();
  await Promise.all([first, push, rest]);
  expect(events).toEqual(["first page", "push reread", "remaining pages"]);
});

it("keeps coalesced push debt even when a phase queues while the first push reread is held", async () => {
  const serialize = createTaskReadQueue();
  const held = deferred();
  const events = [];
  let reads = 0;
  const pushRead = trailingRead((key) => serialize(key, "s1", () => {
    reads += 1;
    events.push(`push ${reads}`);
    return reads === 1 ? held.promise : "latest push";
  }));
  const push = pushRead("p1");
  const trailing = pushRead("p1");
  const again = pushRead("p1");
  const rest = serialize("p1", "s1", () => { events.push("remaining pages"); });
  held.resolve("first push");
  await expect(push).resolves.toBe("first push");
  await expect(trailing).resolves.toBe("latest push");
  await expect(again).resolves.toBe("latest push");
  await rest;
  expect(events).toEqual(["push 1", "remaining pages", "push 2"]);
});

it("drains queued work after rejection and settles each caller with its own result", async () => {
  const serialize = createTaskReadQueue();
  const held = deferred();
  const first = serialize("p1", "s1", () => held.promise);
  const firstRejected = expect(first).rejects.toThrow("failed first page");
  const second = serialize("p1", "s1", () => { throw new Error("failed reread"); });
  const secondRejected = expect(second).rejects.toThrow("failed reread");
  const third = serialize("p1", "s1", () => "remaining pages");
  held.reject(new Error("failed first page"));
  await Promise.all([firstRejected, secondRejected]);
  await expect(third).resolves.toBe("remaining pages");
  await expect(serialize("p1", "s1", () => "after drain")).resolves.toBe("after drain");
});

it("starts a replacement session immediately and discards the old queued reads", async () => {
  const serialize = createTaskReadQueue();
  const old = deferred();
  const current = deferred();
  const oldQueued = vi.fn();
  const newQueued = vi.fn(() => "new rest");
  const newFirst = vi.fn(() => current.promise);
  const first = serialize("p1", "old", () => old.promise);
  const discarded = serialize("p1", "old", oldQueued);
  const next = serialize("p1", "new", newFirst);
  const nextRest = serialize("p1", "new", newQueued);
  expect(newFirst).toHaveBeenCalledTimes(1);
  old.resolve("old first");
  await first;
  await expect(discarded).resolves.toBeUndefined();
  expect(oldQueued).not.toHaveBeenCalled();
  expect(newQueued).not.toHaveBeenCalled();
  current.resolve("new first");
  await expect(next).resolves.toBe("new first");
  await expect(nextRest).resolves.toBe("new rest");
});

it("does not block a separate project's tasks behind a held queue", async () => {
  const serialize = createTaskReadQueue();
  const held = deferred();
  const first = serialize("p1", "s1", () => held.promise);
  await expect(serialize("p2", "s1", () => "p2 first page")).resolves.toBe("p2 first page");
  held.resolve();
  await first;
});
