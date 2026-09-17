// @vitest-environment jsdom
// Regression for the review surface's merge single-flight: paint() polls every
// 1.6s and re-runs updateActions() even when the diff key is unchanged. While a
// git action RPC is in flight, that repaint must NOT remount a fresh, enabled
// split button (which would carry a new latch — a second concurrent merge).

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createTaskReview, reviewMergeOptions, REVIEW_POLL_MS } from "../src/views/taskReview.js";
import { FIRST_PAGE_ITEMS } from "../src/core/thread.js";

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
    const toolbar = document.createElement("div");
    toolbar.className = "gtmerge";
    document.body.append(toolbar, host);
    plug.mount(host, { gitActions: () => toolbar });
    await vi.advanceTimersByTimeAsync(0); // flush the first paint

    const primary = toolbar.querySelector(".btn:not(.caret)");
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
    const after = toolbar.querySelector(".btn:not(.caret)");
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

  // The reviewer opens the merge menu to reach an option in it. The 1.6s poll
  // must not shut it before they get there.
  it("leaves the open merge menu standing through a poll tick, and still runs its item", async () => {
    const calls = [];
    const callRpc = (method, params) => {
      calls.push([method, params]);
      return method === "run.diff" ? Promise.resolve({ patch: PATCH, stat: {}, files: [] }) : Promise.resolve({});
    };
    const plug = createTaskReview({
      taskId: "r2",
      callRpc,
      getTask: () => ({ ...TASK, adopted: true }),
      isOffline: () => false,
      onMerged: () => {},
    });
    const host = document.createElement("div");
    const toolbar = document.createElement("div");
    toolbar.className = "gtmerge";
    document.body.append(toolbar, host);
    plug.mount(host, { gitActions: () => toolbar });
    await vi.advanceTimersByTimeAsync(0);

    toolbar.querySelector(".caret").click();
    const menu = toolbar.querySelector(".splitmenu");
    expect(menu.hidden).toBe(false);

    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS * 2 + 10);

    expect(toolbar.querySelector(".splitmenu"), "the poll replaced the menu").toBe(menu);
    expect(menu.hidden).toBe(false);

    menu.querySelector('[data-action="merge_release"]').click();
    await vi.advanceTimersByTimeAsync(0);
    document.querySelector("[data-confirm-ok]").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.find(([method]) => method === "run.git_action")[1]).toEqual({
      run_id: "r2",
      action: "merge",
      cleanup: "release",
      thread_limit: FIRST_PAGE_ITEMS,
    });
    plug.unmount();
  });
});

describe("reviewMergeOptions", () => {
  it("keeps the merge variants for a worktree run", () => {
    expect(reviewMergeOptions(true, "main").map((o) => o.id)).toEqual([
      "merge_prune",
      "merge_keep",
      "merge_release",
      "merge_push",
      "commit",
      "push",
    ]);
  });

  // Only an adopted run can be released back to the user; a run Build cut has
  // nothing to hand back.
  it("leaves out Merge & release on a run Build cut itself", () => {
    expect(reviewMergeOptions(false, "main").map((o) => o.id)).not.toContain("merge_release");
  });
});
