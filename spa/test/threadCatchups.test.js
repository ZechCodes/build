import { expect, it, vi } from "vitest";
import { createThreadCatchups } from "../src/core/threadCatchups.js";

const gate = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const request = (sequence, generation = "one") => ({ sequence, generation, active: () => true });

it("shares identical active goals and coalesces one trailing high-water round", async () => {
  const first = gate(), second = gate();
  let covered = 0;
  const read = vi.fn(async (goal) => {
    await (goal.sequence === 1 ? first.promise : second.promise);
    covered = goal.sequence;
  });
  const jobs = createThreadCatchups({ needed: (goal) => goal.sequence > covered, read });
  const one = jobs.enqueue("thread", request(1));
  expect(jobs.enqueue("thread", request(1))).toBe(one);
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  const two = jobs.enqueue("thread", request(2));
  expect(jobs.enqueue("thread", request(9))).toBe(two);
  expect(jobs.snapshot()).toEqual({ running: 1, pending: 1 });
  first.resolve();
  await one;
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(read.mock.calls[1][0].sequence).toBe(9);
  expect(jobs.snapshot()).toEqual({ running: 1, pending: 0 });
  second.resolve();
  await two;
  expect(jobs.snapshot()).toEqual({ running: 0, pending: 0 });
});

it("skips queued work already covered by a carried append", async () => {
  let needed = true;
  const held = gate(), read = vi.fn(async () => held.promise);
  const jobs = createThreadCatchups({ needed: () => needed, read });
  const first = jobs.enqueue("thread", request(1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  const trailing = jobs.enqueue("thread", request(2));
  needed = false; held.resolve();
  await Promise.all([first, trailing]);
  expect(read).toHaveBeenCalledTimes(1);
});

it.each(["failed", "unchanged"])("bounds %s reads without renewing the same goal's budget", async (outcome) => {
  const read = vi.fn(async () => { if (outcome === "failed") throw new Error("offline"); });
  const jobs = createThreadCatchups({ needed: () => true, read });
  await jobs.enqueue("thread", request(4));
  expect(read).toHaveBeenCalledTimes(2);
  await jobs.enqueue("thread", request(4));
  await jobs.enqueue("thread", request(3));
  expect(read).toHaveBeenCalledTimes(2);
  await jobs.enqueue("thread", request(5));
  expect(read).toHaveBeenCalledTimes(4);
});

it("settles abandoned generation promises and admits the replacement immediately", async () => {
  const held = gate();
  const read = vi.fn((goal) => goal.generation === "one" ? held.promise : Promise.resolve());
  const needed = vi.fn((goal) => goal.generation === "one" || read.mock.calls.filter(([item]) => item.generation === "two").length === 0);
  const jobs = createThreadCatchups({ needed, read });
  const old = jobs.enqueue("thread", request(1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  const trailing = jobs.enqueue("thread", request(2));
  await jobs.enqueue("thread", request(1, "two"));
  await Promise.all([old, trailing]);
  expect(read).toHaveBeenCalledTimes(2);
  expect(read.mock.calls[0][1]()).toBe(false);
  held.resolve();
});

it("clear settles active and trailing callers even when the read never answers", async () => {
  const held = gate(), read = vi.fn(() => held.promise);
  const jobs = createThreadCatchups({ needed: () => true, read });
  const first = jobs.enqueue("thread", request(1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  const second = jobs.enqueue("thread", request(2));
  jobs.clear();
  await Promise.all([first, second]);
  expect(jobs.snapshot()).toEqual({ running: 0, pending: 0 });
  expect(read.mock.calls[0][1]()).toBe(false);
  held.resolve();
});

it("starts distinct conversations without an extra enrichment worker cap", async () => {
  const held = gate(), read = vi.fn(() => held.promise);
  const jobs = createThreadCatchups({ needed: () => true, read });
  const pending = ["cold-1", "cold-2", "visible"].map((key) => jobs.enqueue(key, request(1)));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(3));
  jobs.clear(); await Promise.all(pending); held.resolve();
});

it("checks canonical coverage and lifetime before admitting a read", async () => {
  const needed = vi.fn(() => false), read = vi.fn();
  const jobs = createThreadCatchups({ needed, read });
  await jobs.enqueue("covered", request(2));
  await jobs.enqueue("inactive", { ...request(2), active: () => false });
  expect(needed).toHaveBeenCalledTimes(1);
  expect(read).not.toHaveBeenCalled();
});

it("renews an exhausted budget only when the generation changes", async () => {
  const read = vi.fn(async () => {});
  const jobs = createThreadCatchups({ needed: () => true, read });
  await jobs.enqueue("thread", request(1));
  await jobs.enqueue("thread", request(1));
  expect(read).toHaveBeenCalledTimes(2);
  await jobs.enqueue("thread", request(1, "two"));
  expect(read).toHaveBeenCalledTimes(4);
});
