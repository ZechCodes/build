// @vitest-environment jsdom
// Regression for the review surface's merge single-flight: paint() polls every
// 1.6s and re-runs updateActions() even when the diff key is unchanged. While a
// git action RPC is in flight, that repaint must NOT remount a fresh, enabled
// split button (which would carry a new latch — a second concurrent merge).

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createTaskReview, REVIEW_POLL_MS } from "../src/views/taskReview.js";

const PATCH = [
  "diff --git a/a.txt b/a.txt",
  "index 0000000..1111111 100644",
  "--- a/a.txt",
  "+++ b/a.txt",
  "@@ -1 +1 @@",
  "-old",
  "+new",
  "",
].join("\n");

const TASK = { state: "review", base_branch: "main", branch: "feat/x", adopted: false, goal: "g" };

describe("taskReview merge flight vs the poll (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps the busy, disabled button across poll ticks while run.git_action is in flight", async () => {
    const rpcCalls = [];
    const callRpc = (method, params) => {
      rpcCalls.push(method);
      if (method === "run.diff") return Promise.resolve({ patch: PATCH, stat: {}, files: [] });
      if (method === "run.git_action") return new Promise(() => {}); // never settles — held in flight
      return Promise.resolve({});
    };
    const plug = createTaskReview({
      taskId: "r1",
      callRpc,
      getTask: () => TASK,
      isOffline: () => false,
      onMerged: () => {},
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    plug.mount(host);
    await vi.advanceTimersByTimeAsync(0); // flush the first paint

    const primary = host.querySelector("#diffactions .btn.primary:not(.caret)");
    expect(primary).toBeTruthy();
    expect(primary.textContent).toBe("Merge");

    primary.click(); // → confirm modal
    await vi.advanceTimersByTimeAsync(0);
    const ok = document.querySelector("[data-confirm-ok]");
    expect(ok).toBeTruthy();
    ok.click(); // → run.git_action dispatched, stays pending
    await vi.advanceTimersByTimeAsync(0);
    expect(rpcCalls.filter((m) => m === "run.git_action")).toHaveLength(1);

    // Two full poll ticks while the RPC is in flight: the button must stay the
    // same disabled busy button — not a fresh enabled remount.
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS * 2 + 10);
    const after = host.querySelector("#diffactions .btn.primary:not(.caret)");
    expect(after.disabled).toBe(true);
    expect(after.textContent).toBe("merging…");
    expect(document.querySelectorAll(".modal-scrim")).toHaveLength(0);

    // Even a synthetic click on whatever is mounted dispatches nothing new.
    after.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(rpcCalls.filter((m) => m === "run.git_action")).toHaveLength(1);
    expect(document.querySelectorAll(".modal-scrim")).toHaveLength(0);

    plug.unmount();
  });
});
