import { expect, it } from "vitest";
import { mergeKnownTaskHeads, taskHeadReadPlan } from "../src/core/taskOrderedStreams.js";

const task = (number, id = `t${number}`) => ({ id, number });
const compare = (a, b) => b.number - a.number;
const stream = (id, items, complete = false) => ({ id, items, complete });

it("merges complete overlap once in exact order with independent consumption", () => {
  const result = mergeKnownTaskHeads([stream("a", [task(5), task(4)], true), stream("b", [task(5), task(4)], true)], compare, 10);
  expect(result.items.map(({ number }) => number)).toEqual([5, 4]);
  expect(result.consumed).toEqual({ a: 2, b: 2 });
  expect(result.complete).toBe(true);
});

it("never emits past an empty sparse or unknown branch without a next-item bound", () => {
  const result = mergeKnownTaskHeads([stream("known", [task(5)], true), stream("sparse", [])], compare, 10);
  expect(result.items).toEqual([]);
  expect(result.needsHeads).toEqual(["sparse"]);
  expect(result.complete).toBe(false);
});

it("stops at a newly unknown head even when another branch has buffered rows", () => {
  const result = mergeKnownTaskHeads([stream("a", [task(6)]), stream("b", [task(5), task(3)], true)], compare, 10);
  expect(result.items.map(({ number }) => number)).toEqual([6]);
  expect(result.consumed).toEqual({ a: 1, b: 0 });
  expect(result.needsHeads).toEqual(["a"]);
  expect(result.complete).toBe(false);
});

it("permits an exhausted branch, breaks equal-order ties stably, and honors the visible limit", () => {
  const result = mergeKnownTaskHeads([stream("empty", [], true), stream("b", [task(5, "B")], true), stream("a", [task(5, "A")], true)], compare, 1);
  expect(result.items.map(({ id }) => id)).toEqual(["B"]);
  expect(result.complete).toBe(false);
  expect(result.needsHeads).toEqual([]);
});

it("retains deduplication across explicit continuation operations", () => {
  const result = mergeKnownTaskHeads([stream("a", [task(5), task(3)], true), stream("b", [task(4), task(3)], true)], compare, 2, new Set(["t5"]));
  expect(result.items.map(({ number }) => number)).toEqual([4, 3]);
  expect(result.seen).toEqual(new Set(["t5", "t4", "t3"]));
});

it("bounds unknown project-head admission independently from the number of projects", () => {
  const streams = Array.from({ length: 1000 }, (_, index) => stream(`project-${index}`, []));
  expect(taskHeadReadPlan(streams, 3)).toEqual(["project-0", "project-1", "project-2"]);
  expect(mergeKnownTaskHeads(streams, compare, 25).items).toEqual([]);
  expect(taskHeadReadPlan([stream("known", [task(1)]), stream("done", [], true)], 3)).toEqual([]);
  expect(taskHeadReadPlan(streams, 1000)).toHaveLength(4);
});
