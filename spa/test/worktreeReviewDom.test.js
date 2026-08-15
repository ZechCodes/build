// @vitest-environment jsdom
// The worktree review surface's actionbar under its own poll: the merge menu a
// reviewer opened has to survive a tick that read the same worktree, and the
// Abandon button beside it has to be the same element it was.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createWorktreeReview, WORKTREE_REVIEW_POLL_MS } from "../src/views/worktreeReview.js";

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

/** An adopting caller that records what it dispatched. */
function stubAdopting(calls) {
  let runId = null;
  return {
    setAdoptParams: () => {},
    adoptedRunId: () => runId,
    runCall: async (method, params) => {
      runId = "run-adopted";
      calls.push([method, params]);
      return {};
    },
  };
}

function mountReview({ calls = [], adoptable = true } = {}) {
  const callRpc = async (method, params) => {
    calls.push([method, params]);
    if (method === "worktree.diff")
      return { patch: PATCH, branch: "feat/x", base_branch: "main", path: "/wt/x", adoptable };
    return {};
  };
  const review = createWorktreeReview({
    projectId: "p1",
    worktreeId: "wt-1",
    callRpc,
    adopting: stubAdopting(calls),
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  review.mount(host);
  return { review, host, calls };
}

describe("the worktree review actionbar under the poll", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => vi.useRealTimers());

  it("leaves the open merge menu standing through a tick, and still runs its item", async () => {
    const { review, host, calls } = mountReview();
    await vi.advanceTimersByTimeAsync(0);

    host.querySelector(".csactions .caret").click();
    const menu = host.querySelector(".csactions .splitmenu");
    expect(menu.hidden).toBe(false);

    await vi.advanceTimersByTimeAsync(WORKTREE_REVIEW_POLL_MS * 2 + 10);

    expect(host.querySelector(".csactions .splitmenu"), "the poll replaced the menu").toBe(menu);
    expect(menu.hidden).toBe(false);

    menu.querySelector('[data-action="merge_keep"]').click();
    await vi.advanceTimersByTimeAsync(0);
    document.querySelector("[data-confirm-ok]").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.find(([method]) => method === "run.git_action")[1]).toEqual({ action: "merge", cleanup: "keep" });
    review.unmount();
  });

  it("keeps the same buttons across a tick that read the same worktree", async () => {
    const { review, host } = mountReview();
    await vi.advanceTimersByTimeAsync(0);
    const abandon = [...host.querySelectorAll(".csactions .btn")].find((b) => b.textContent === "Abandon & delete");
    expect(abandon).toBeTruthy();

    await vi.advanceTimersByTimeAsync(WORKTREE_REVIEW_POLL_MS + 10);

    const after = [...host.querySelectorAll(".csactions .btn")].find((b) => b.textContent === "Abandon & delete");
    expect(after).toBe(abandon);
    review.unmount();
  });

  it("says why a worktree cannot be adopted instead of offering the verbs", async () => {
    const { review, host } = mountReview({ adoptable: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".cshint").textContent).toContain("check out a feature branch");
    expect(host.querySelector(".csactions").children).toHaveLength(0);
    review.unmount();
  });
});
