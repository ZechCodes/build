// @vitest-environment jsdom
// The diff conversation's composer: a freeform thread.post ("ask / tell the
// agent something") — a durable conversation write that never dispatches a
// revision pass or moves run state, distinct from the request-changes box
// (#dgeneral → run.request_changes). The bridge refuses thread.post only once
// the run is terminal, so the composer is offered everywhere else — including
// the review gate.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createTaskReview, REVIEW_POLL_MS } from "../src/views/taskReview.js";
import { createThreadCache } from "../src/core/thread.js";
import { RUN_TERMINAL_STATES } from "../src/core/board.js";

const patchFor = (file) =>
  [
    `diff --git a/${file} b/${file}`,
    "index 0000000..1111111 100644",
    `--- a/${file}`,
    `+++ b/${file}`,
    "@@ -1 +1 @@",
    "-old",
    "+new",
    "",
  ].join("\n");

// The run view thread.post returns: the same task, its thread grown by the
// posted user message (a FULL uncursored thread, as the bridge ships from a
// mutation — no thread_total).
function runViewAfterPost(task, body, sequence) {
  const items = [...((task.thread && task.thread.items) || []), { type: "message", data: { role: "user", body, sequence } }];
  return { ...task, thread: { items, sessions: [], revisions: [] } };
}

function mountReview(initialTask, { deferThreadPost = false } = {}) {
  const rpcCalls = [];
  let patch = patchFor("a.txt");
  let task = initialTask;
  let nextSequence = 100;
  const pendingPosts = [];
  // Mirrors views/task.js: the view owns the cursor cache; the plug folds
  // thread.post's returned view back through it via absorbTaskView.
  const threadCache = createThreadCache();
  const callRpc = (method, params) => {
    rpcCalls.push({ method, params });
    if (method === "run.diff") return Promise.resolve({ patch, stat: {}, files: [] });
    if (method === "thread.post") {
      const view = runViewAfterPost(task, params.body, ++nextSequence);
      if (deferThreadPost) return new Promise((resolve) => pendingPosts.push(() => resolve(view)));
      return Promise.resolve(view);
    }
    return Promise.resolve({});
  };
  const plug = createTaskReview({
    taskId: "r1",
    callRpc,
    getTask: () => task,
    absorbTaskView: (view) => {
      task = { ...view, thread: threadCache.absorb(view.thread) };
      return task;
    },
    isOffline: () => false,
    onMerged: () => {},
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  plug.mount(host);
  return {
    plug,
    host,
    rpcCalls,
    threadCache,
    setPatch: (p) => (patch = p),
    releasePosts: () => pendingPosts.splice(0).forEach((resolve) => resolve()),
  };
}

/// A review surface whose thread items the test can swap between polls, to
/// stand in for the bridge mutating an item in place (seen_at/resolved_by).
function mountReviewWithThread(items) {
  let task = { run_id: "r1", state: "review", thread: { items, sessions: [], revisions: [] } };
  const callRpc = (method) =>
    method === "run.diff"
      ? Promise.resolve({ patch: patchFor("a.txt"), stat: {}, files: [] })
      : Promise.resolve({});
  const plug = createTaskReview({
    taskId: "r1",
    callRpc,
    getTask: () => task,
    absorbTaskView: (view) => (task = view),
    isOffline: () => false,
    onMerged: () => {},
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  plug.mount(host);
  return {
    plug,
    host,
    setTask: (nextItems) => {
      task = { ...task, thread: { ...task.thread, items: nextItems } };
    },
  };
}

/// A review surface whose run.diff resolution the test controls, so a poll
/// paint can be held mid-flight while the composer posts underneath it.
function mountReviewWithSlowDiff() {
  const rpcCalls = [];
  const pendingDiffs = [];
  let task = { run_id: "r1", state: "review", thread: { items: [], sessions: [], revisions: [] } };
  let nextSequence = 100;
  const callRpc = (method, params) => {
    rpcCalls.push({ method, params });
    if (method === "run.diff") {
      return new Promise((resolve) =>
        pendingDiffs.push(() => resolve({ patch: patchFor("a.txt"), stat: {}, files: [] })),
      );
    }
    if (method === "thread.post") return Promise.resolve(runViewAfterPost(task, params.body, ++nextSequence));
    return Promise.resolve({});
  };
  const plug = createTaskReview({
    taskId: "r1",
    callRpc,
    getTask: () => task,
    absorbTaskView: (view) => (task = view),
    isOffline: () => false,
    onMerged: () => {},
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  plug.mount(host);
  return { plug, host, rpcCalls, resolveDiff: () => pendingDiffs.splice(0).forEach((resolve) => resolve()) };
}

const cmdEnter = (input) => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true }));

const typeInto = (input, text) => {
  input.value = text;
  input.dispatchEvent(new Event("input"));
};

describe("diff conversation composer (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("gates the composer on the bridge's OWN thread.post refusal set (run terminal states)", () => {
    // thread.post refuses exactly RunState::is_terminal (bridge/src/run.rs).
    // Derive that set from the Rust source instead of restating a literal, so
    // this test detects the bridge diverging from RUN_TERMINAL_STATES.
    // (jsdom rewrites import.meta.url to an http scheme, so resolve from the
    // vitest root — spa/ — instead.)
    const runSource = readFileSync(resolve(process.cwd(), "../bridge/src/run.rs"), "utf8");
    const isTerminalArm = runSource.match(/fn is_terminal\(&self\) -> bool \{\s*matches!\(\s*self,([\s\S]*?)\)/);
    expect(isTerminalArm).not.toBeNull();
    const bridgeTerminalStates = [...isTerminalArm[1].matchAll(/RunState::(\w+)/g)].map(([, variant]) =>
      variant.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase(),
    );
    expect(bridgeTerminalStates.length).toBeGreaterThan(0);
    expect(new Set(bridgeTerminalStates)).toEqual(RUN_TERMINAL_STATES);
  });

  it("renders a composer with diff-scoped ids (no collision with the plan composer) while building", async () => {
    const { plug, host } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    expect(input).not.toBeNull();
    expect(host.querySelector("#diffthreadsend")).not.toBeNull();
    expect(host.querySelector("#planthreadinput")).toBeNull(); // plan ids stay free for a coexisting plan composer
    // The copy distinguishes the two verbs: composer = message, box = revision.
    expect(input.placeholder).toContain("without requesting a revision");
    expect(input.placeholder).not.toBe(host.querySelector("#dgeneral").placeholder);
    // The request-changes box is untouched alongside it.
    expect(host.querySelector("#dgeneral").placeholder).toBe(
      "Add a general comment about the changes and request updates…",
    );
    plug.unmount();
  });

  it("offers the composer at the review gate (thread.post is allowed there) alongside request-changes", async () => {
    const { plug, host } = mountReview({ state: "review", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector("#diffthreadinput")).not.toBeNull();
    expect(host.querySelector("#dgeneral")).not.toBeNull();
    plug.unmount();
  });

  it("offers no composer on any terminal run (the conversation is closed)", async () => {
    for (const state of RUN_TERMINAL_STATES) {
      document.body.innerHTML = "";
      const { plug, host } = mountReview({ state, base_branch: "main", branch: "feat/x" });
      await vi.advanceTimersByTimeAsync(0);
      expect(host.querySelector("#diffthreadinput"), state).toBeNull();
      plug.unmount();
    }
  });

  it("Cmd+Enter posts through thread.post, clears the input, and re-enables Send — even with focus held", async () => {
    const { plug, host, rpcCalls } = mountReview({ state: "blocked", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    typeInto(input, "why is the build blocked?");
    input.focus(); // Cmd+Enter leaves focus in the textarea — the wedge case
    cmdEnter(input);
    await vi.advanceTimersByTimeAsync(0);

    const postCalls = rpcCalls.filter((c) => c.method === "thread.post");
    expect(postCalls).toHaveLength(1);
    expect(postCalls[0].params).toEqual({ entity_id: "r1", body: "why is the build blocked?" });
    expect(rpcCalls.filter((c) => c.method === "run.message")).toHaveLength(0);
    expect(rpcCalls.filter((c) => c.method === "run.request_changes")).toHaveLength(0);
    // The composer recovered: empty input, Send enabled — not wedged at
    // "sending…" with the sent text still in the box.
    expect(host.querySelector("#diffthreadinput").value).toBe("");
    const send = host.querySelector("#diffthreadsend");
    expect(send.disabled).toBe(false);
    expect(send.textContent).toBe("Send");
    plug.unmount();
  });

  it("a second Cmd+Enter during an in-flight send posts only ONCE", async () => {
    const { plug, host, rpcCalls, releasePosts } = mountReview(
      { state: "building", base_branch: "main", branch: "feat/x" },
      { deferThreadPost: true },
    );
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    typeInto(input, "one question");
    input.focus();
    cmdEnter(input);
    cmdEnter(input); // the natural retry against a "sending…" button
    await vi.advanceTimersByTimeAsync(0);
    releasePosts();
    await vi.advanceTimersByTimeAsync(0);

    expect(rpcCalls.filter((c) => c.method === "thread.post")).toHaveLength(1);
    plug.unmount();
  });

  it("echoes the sent message immediately — no waiting for a poll — through the thread cache", async () => {
    const { plug, host, threadCache } = mountReview({ state: "review", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    typeInto(input, "did you handle the empty-file edge case?");
    input.focus();
    cmdEnter(input);
    // Flush only the microtasks of the send + repaint — NOT a poll tick.
    await vi.advanceTimersByTimeAsync(0);

    expect(host.textContent).toContain("did you handle the empty-file edge case?");
    expect(host.querySelector(".thread-message.user")).not.toBeNull();
    // The returned view was absorbed into the cache (not painted around it):
    // the next cursored poll starts after the echoed message's sequence, so
    // it can never disagree with what was just painted.
    expect(threadCache.cursorParam()).toEqual({ thread_after_sequence: 101 });
    plug.unmount();
  });

  it("preserves a half-typed draft across a poll repaint", async () => {
    const { plug, host, setPatch } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    typeInto(input, "half-typed question");

    // The diff moves under the reviewer (the agent is working): the next tick
    // rebuilds the body — the draft must land in the fresh composer.
    setPatch(patchFor("b.txt"));
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.textContent).toContain("b.txt"); // the rebuild really happened
    expect(host.querySelector("#diffthreadinput").value).toBe("half-typed question");
    plug.unmount();
  });

  it("carries unsent request-changes text across the echo's forced rebuild", async () => {
    const { plug, host } = mountReview({ state: "review", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    typeInto(host.querySelector("#dgeneral"), "tighten the parser");
    const input = host.querySelector("#diffthreadinput");
    typeInto(input, "quick question first");
    input.focus();
    cmdEnter(input);
    await vi.advanceTimersByTimeAsync(0);

    // The echo repainted the body — the request-changes box must not lose its
    // unsent text to that rebuild.
    expect(host.textContent).toContain("quick question first");
    expect(host.querySelector("#dgeneral").value).toBe("tighten the parser");
    plug.unmount();
  });

  it("leaves the request-changes flow routing to run.request_changes with its diff anchors intact", async () => {
    const { plug, host, rpcCalls } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    // A line comment through the real flow: expand the file (the fold handler
    // belongs to the mounting pane, so uncap directly), tap the line, open the
    // popover, type, save — it must reach the bridge as a diff-anchored message.
    host.querySelector(".file").classList.remove("capped");
    host.querySelector('tr.add[data-ln="1"]').dispatchEvent(new MouseEvent("click", { bubbles: true }));
    document.querySelector(".cp-add").click();
    const popInput = document.querySelector(".cp-input");
    popInput.value = "rename this";
    document.querySelector(".cp-save").click();

    typeInto(host.querySelector("#dgeneral"), "tighten the error handling");
    host.querySelector("#requestChanges").click();
    await vi.advanceTimersByTimeAsync(0);

    const requestCalls = rpcCalls.filter((c) => c.method === "run.request_changes");
    expect(requestCalls).toHaveLength(1);
    expect(requestCalls[0].params.run_id).toBe("r1");
    const [anchored, general] = requestCalls[0].params.messages;
    expect(anchored.body).toBe("rename this");
    expect(anchored.anchor).toMatchObject({ artifact: "diff", path: "a.txt", line_start: 1, line_end: 1, side: "new" });
    expect(general).toEqual({ body: "tighten the error handling", anchor: null });
    expect(rpcCalls.filter((c) => c.method === "thread.post")).toHaveLength(0);
    plug.unmount();
  });
});

describe("conversation freshness on the review surface", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("repaints when the agent marks a message seen, though no item is added", async () => {
    // read_unread stamps seen_at on an ALREADY-sequenced message and appends
    // nothing. A repaint key built from creation sequences alone is identical
    // before and after, so the badge stays "Unread" for the life of the page —
    // the regression the cursor protocol was supposed to close.
    const unread = { type: "message", data: { role: "user", body: "why this name?", sequence: 1, seen_at: null } };
    const { host, plug, setTask } = mountReviewWithThread([unread]);
    await vi.advanceTimersByTimeAsync(0);
    expect(host.textContent).toContain("Unread");

    setTask([{ ...unread, data: { ...unread.data, seen_at: "2026-07-24T12:05:00Z", updated_sequence: 2 } }]);
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);

    expect(host.textContent).toContain("Seen");
    expect(host.textContent).not.toContain("Unread");
    plug.unmount();
  });

  it("does not let a stale in-flight paint swallow the composer's forced rebuild", async () => {
    // paint() snapshots the task BEFORE awaiting run.diff. A timer paint still
    // in flight when the composer posts resolves holding the pre-post task,
    // consumes forceRebuild, and renders stale — and the composer's own paint
    // then early-returns because focus is still in the textarea.
    const { host, plug, rpcCalls, resolveDiff } = mountReviewWithSlowDiff();
    await vi.advanceTimersByTimeAsync(0);
    resolveDiff();
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    input.focus();
    typeInto(input, "does this cover the migration?");
    // A timer paint starts and blocks on run.diff...
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    // ...while the user sends. Both are now in flight.
    cmdEnter(input);
    await vi.advanceTimersByTimeAsync(0);
    resolveDiff();
    await vi.advanceTimersByTimeAsync(20);

    expect(rpcCalls.filter((c) => c.method === "thread.post")).toHaveLength(1);
    expect(host.textContent).toContain("does this cover the migration?");
    plug.unmount();
  });
});
