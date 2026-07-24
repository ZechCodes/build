// @vitest-environment jsdom
// The diff conversation's composer: a freeform run.message ("ask / tell the
// agent something") that never dispatches a revision pass — distinct from the
// request-changes box (#dgeneral → run.request_changes). Gated on the run
// states the bridge's message_run actually accepts (building + parked); the
// review gate refuses run.message, so no composer is offered there.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createTaskReview, REVIEW_POLL_MS, RUN_MESSAGEABLE_STATES } from "../src/views/taskReview.js";

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

function mountReview(task) {
  const rpcCalls = [];
  let patch = patchFor("a.txt");
  const callRpc = (method, params) => {
    rpcCalls.push({ method, params });
    if (method === "run.diff") return Promise.resolve({ patch, stat: {}, files: [] });
    return Promise.resolve({});
  };
  const plug = createTaskReview({
    taskId: "r1",
    callRpc,
    getTask: () => task,
    isOffline: () => false,
    onMerged: () => {},
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  plug.mount(host);
  return { plug, host, rpcCalls, setPatch: (p) => (patch = p) };
}

describe("diff conversation composer (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("gates on exactly the states the bridge's message_run accepts", () => {
    expect(RUN_MESSAGEABLE_STATES).toEqual(["building", "blocked", "failed", "idle_unreported", "interrupted"]);
  });

  it("renders a composer with diff-scoped ids (no collision with the plan composer) while building", async () => {
    const { plug, host } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    expect(input).not.toBeNull();
    expect(host.querySelector("#diffthreadsend")).not.toBeNull();
    expect(host.querySelector("#planthreadinput")).toBeNull(); // plan ids stay free for a coexisting plan composer
    // The copy distinguishes the two verbs: composer = message, box = revision.
    expect(input.placeholder).toContain("message");
    expect(input.placeholder).not.toBe(host.querySelector("#dgeneral").placeholder);
    // The request-changes box is untouched alongside it.
    expect(host.querySelector("#dgeneral").placeholder).toBe(
      "Add a general comment about the changes and request updates…",
    );
    plug.unmount();
  });

  it("offers no composer at the review gate (run.message is refused there) but keeps request-changes", async () => {
    const { plug, host } = mountReview({ state: "review", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector("#diffthreadinput")).toBeNull();
    expect(host.querySelector("#dgeneral")).not.toBeNull();
    plug.unmount();
  });

  it("offers no composer on a terminal run", async () => {
    const { plug, host } = mountReview({ state: "merged", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector("#diffthreadinput")).toBeNull();
    plug.unmount();
  });

  it("sends through run.message with the run id and message, then clears the draft", async () => {
    const { plug, host, rpcCalls } = mountReview({ state: "blocked", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    input.value = "why is the build blocked?";
    input.dispatchEvent(new Event("input"));
    host.querySelector("#diffthreadsend").click();
    await vi.advanceTimersByTimeAsync(0);

    const messageCalls = rpcCalls.filter((c) => c.method === "run.message");
    expect(messageCalls).toHaveLength(1);
    expect(messageCalls[0].params).toEqual({ run_id: "r1", message: "why is the build blocked?" });
    expect(rpcCalls.filter((c) => c.method === "run.request_changes")).toHaveLength(0);
    expect(host.querySelector("#diffthreadinput").value).toBe("");
    plug.unmount();
  });

  it("preserves a half-typed draft across a poll repaint", async () => {
    const { plug, host, setPatch } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const input = host.querySelector("#diffthreadinput");
    input.value = "half-typed question";
    input.dispatchEvent(new Event("input"));

    // The diff moves under the reviewer (the agent is working): the next tick
    // rebuilds the body — the draft must land in the fresh composer.
    setPatch(patchFor("b.txt"));
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.textContent).toContain("b.txt"); // the rebuild really happened
    expect(host.querySelector("#diffthreadinput").value).toBe("half-typed question");
    plug.unmount();
  });

  it("leaves the request-changes flow routing to run.request_changes", async () => {
    const { plug, host, rpcCalls } = mountReview({ state: "building", base_branch: "main", branch: "feat/x" });
    await vi.advanceTimersByTimeAsync(0);

    const general = host.querySelector("#dgeneral");
    general.value = "tighten the error handling";
    general.dispatchEvent(new Event("input"));
    host.querySelector("#requestChanges").click();
    await vi.advanceTimersByTimeAsync(0);

    const requestCalls = rpcCalls.filter((c) => c.method === "run.request_changes");
    expect(requestCalls).toHaveLength(1);
    expect(requestCalls[0].params.run_id).toBe("r1");
    expect(requestCalls[0].params.messages).toEqual([{ body: "tighten the error handling", anchor: null }]);
    expect(rpcCalls.filter((c) => c.method === "run.message")).toHaveLength(0);
    plug.unmount();
  });
});
