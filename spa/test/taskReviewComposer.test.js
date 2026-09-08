// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createTaskReview } from "../src/views/taskReview.js";

const patch = [
  "diff --git a/a.txt b/a.txt",
  "index 0000000..1111111 100644",
  "--- a/a.txt",
  "+++ b/a.txt",
  "@@ -1 +1 @@",
  "-old",
  "+new",
  "",
].join("\n");

function mountReview(state = "review") {
  const calls = [];
  const task = {
    run_id: "r1",
    state,
    base_branch: "main",
    branch: "feature/x",
    thread: { items: [{ type: "message", data: { role: "agent", body: "Lives elsewhere" } }], revisions: [] },
  };
  const plug = createTaskReview({
    taskId: "r1",
    callRpc: async (method, params) => {
      calls.push({ method, params });
      if (method === "run.diff") return { patch, stat: {}, files: [] };
      return {};
    },
    getTask: () => task,
    isOffline: () => false,
    onMerged: () => {},
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  // The note rides in the surface's box under the diff, so the test supplies
  // one, as gitPane does.
  const note = { text: "" };
  plug.mount(host, { readNote: () => note.text });
  return { calls, host, plug, note };
}

describe("dedicated conversation separation from diff review", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  afterEach(() => vi.useRealTimers());

  it("does not tack the conversation or freeform composer onto Changes", async () => {
    const { host, plug } = mountReview();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".review-thread")).toBeNull();
    expect(host.querySelector("#diffthreadinput")).toBeNull();
    expect(host.textContent).not.toContain("Lives elsewhere");
    // The diff takes comments; where they are WRITTEN is the surface's box
    // below the stack, not a field inside it.
    expect(host.querySelector(".fcmt")).not.toBeNull();
    plug.unmount();
  });

  it("still sends selected diff and whole-change-set comments as user messages", async () => {
    const { calls, host, plug, note } = mountReview("building");
    await vi.advanceTimersByTimeAsync(0);
    host.querySelector(".file").classList.remove("capped");
    host.querySelector('tr.add[data-ln="1"]').dispatchEvent(new MouseEvent("click", { bubbles: true }));
    // A press on a line offers the Comment button; the field is behind it.
    document.querySelector(".cp-add").click();
    document.querySelector(".cp-input").value = "rename this";
    document.querySelector(".cp-save").click();
    note.text = "tighten the whole change set";
    await plug.sendComments();
    await vi.advanceTimersByTimeAsync(0);

    const request = calls.find((call) => call.method === "run.request_changes");
    expect(request.params.messages[0]).toMatchObject({
      body: "rename this",
      anchor: { artifact: "diff", path: "a.txt", line_start: 1, line_end: 1 },
    });
    expect(request.params.messages[1]).toEqual({ body: "tighten the whole change set", anchor: null });
    expect(calls.some((call) => call.method === "thread.post")).toBe(false);
    plug.unmount();
  });
});
