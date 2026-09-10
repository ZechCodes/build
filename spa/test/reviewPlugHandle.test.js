// @vitest-environment jsdom
// A plug's handle must not be an enumerated copy of it.
//
// The Changes pane drives its aggregate through a handle it is given, and three
// layers hand that handle on: createReviewPlug makes it, views/taskReview and
// views/worktreeReview wrap it, and views/branchView wraps those again. Each
// wrapper exists to add one thing the layer below cannot answer.
//
// Every one of them was written as a hand-listed subset, and every one of them
// silently dropped whatever the plug learned to do next. It cost, in order: the
// merge verb's host and `commentOffer` (which threw mid-render and took the git
// toolbar, the merge verb and the composer off the screen), and then `refresh`
// (which left every checkbox ticked after Clear). Spreading fixes the instance;
// this fixes the class.

import { describe, expect, it, vi } from "vitest";
import { createReviewPlug } from "../src/core/changesReview.js";
import { createTaskReview } from "../src/views/taskReview.js";
import { createWorktreeReview } from "../src/views/worktreeReview.js";

/** Every method a handle offers. */
const methodsOf = (handle) =>
  Object.keys(handle)
    .filter((name) => typeof handle[name] === "function")
    .sort();

const bare = () => createReviewPlug({ fetchDiff: async () => null });

const taskHandle = () =>
  createTaskReview({
    taskId: "run-1",
    callRpc: async () => ({}),
    getTask: () => null,
    isOffline: () => false,
    onMerged: () => {},
  });

const worktreeHandle = () =>
  createWorktreeReview({
    projectId: "p1",
    worktreeId: "wt-1",
    callRpc: async () => ({}),
    adopting: { setAdoptParams: () => {}, adoptedRunId: () => null, runCall: async () => ({}) },
    isOffline: () => false,
    onAdopted: () => {},
    onFinished: () => {},
    onGone: () => {},
  });

describe("the handle every layer hands on", () => {
  it("the plug offers the pane more than one thing", () => {
    // A guard on the guard: an empty surface would make every check below pass
    // by comparing nothing.
    expect(methodsOf(bare()).length).toBeGreaterThan(3);
  });

  it.each([
    ["views/taskReview.js", taskHandle],
    ["views/worktreeReview.js", worktreeHandle],
  ])("%s passes on everything the plug can do", (_name, make) => {
    const handle = make();
    const missing = methodsOf(bare()).filter((method) => typeof handle[method] !== "function");
    expect(missing, "methods the wrapper dropped").toEqual([]);
  });

  // branchView's wrapper is made inside the view, so it is checked through the
  // shape it wraps rather than by reaching in: whatever a view plug offers, the
  // spread it uses keeps.
  it("branchView's spread keeps every method of whatever it wraps", () => {
    const plug = taskHandle();
    const wrapped = { ...plug, getBase: () => "main" };
    expect(methodsOf(wrapped)).toEqual(expect.arrayContaining(methodsOf(plug)));
    expect(typeof wrapped.getBase).toBe("function");
  });

  it("forwards the options a mount is given, rather than swallowing them", () => {
    const handle = taskHandle();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const gitActions = vi.fn(() => null);
    handle.mount(host, { gitActions });
    // The plug asks its host for somewhere to put the surface's git verbs on
    // every paint; a wrapper that dropped the options would never have one.
    handle.refreshActions();
    expect(gitActions).toHaveBeenCalled();
    handle.unmount();
  });
});
