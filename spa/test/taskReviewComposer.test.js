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
  plug.mount(host);
  return { calls, host, plug };
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
    expect(host.querySelector("#dgeneral")).not.toBeNull();
    plug.unmount();
  });

  it("still sends selected diff and whole-change-set comments as user messages", async () => {
    const { calls, host, plug } = mountReview("building");
    await vi.advanceTimersByTimeAsync(0);
    host.querySelector(".file").classList.remove("capped");
    host.querySelector('tr.add[data-ln="1"]').dispatchEvent(new MouseEvent("click", { bubbles: true }));
    document.querySelector(".cp-add").click();
    document.querySelector(".cp-input").value = "rename this";
    document.querySelector(".cp-save").click();
    host.querySelector("#dgeneral").value = "tighten the whole change set";
    host.querySelector("#dgeneral").dispatchEvent(new Event("input"));
    host.querySelector("#requestChanges").click();
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
