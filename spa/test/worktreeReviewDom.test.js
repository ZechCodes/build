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
  // The surface's git verbs live in the git toolbar it owns, not inside the
  // stack — so the test supplies one, as gitPane does.
  const toolbar = document.createElement("div");
  toolbar.className = "gtmerge";
  document.body.append(toolbar, host);
  review.mount(host, { gitActions: () => toolbar });
  return { review, host, toolbar, calls };
}

describe("the worktree review actionbar under the poll", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => vi.useRealTimers());

  it("leaves the open merge menu standing through a tick, and still runs its item", async () => {
    const { review, toolbar, calls } = mountReview();
    await vi.advanceTimersByTimeAsync(0);

    toolbar.querySelector(".caret").click();
    const menu = toolbar.querySelector(".splitmenu");
    expect(menu.hidden).toBe(false);

    await vi.advanceTimersByTimeAsync(WORKTREE_REVIEW_POLL_MS * 2 + 10);

    expect(toolbar.querySelector(".splitmenu"), "the poll replaced the menu").toBe(menu);
    expect(menu.hidden).toBe(false);

    menu.querySelector('[data-action="merge_keep"]').click();
    await vi.advanceTimersByTimeAsync(0);
    document.querySelector("[data-confirm-ok]").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.find(([method]) => method === "run.git_action")[1]).toEqual({ action: "merge", cleanup: "keep" });
    review.unmount();
  });

  it("keeps the same buttons across a tick that read the same worktree", async () => {
    const { review, toolbar } = mountReview();
    await vi.advanceTimersByTimeAsync(0);
    const abandon = [...toolbar.querySelectorAll(".btn")].find((b) => b.textContent === "Abandon & delete");
    expect(abandon).toBeTruthy();

    await vi.advanceTimersByTimeAsync(WORKTREE_REVIEW_POLL_MS + 10);

    const after = [...toolbar.querySelectorAll(".btn")].find((b) => b.textContent === "Abandon & delete");
    expect(after).toBe(abandon);
    review.unmount();
  });

  it("says why a worktree cannot be adopted instead of offering the verbs", async () => {
    const { review, host, toolbar } = mountReview({ adoptable: false });
    await vi.advanceTimersByTimeAsync(0);
    // A fact about the checkout, so it rides the bar over the diff beside the
    // file counts rather than a line of copy under the stack.
    expect(host.querySelector(".diffbar").textContent).toContain("check out a feature branch");
    expect(toolbar.children).toHaveLength(0);
    review.unmount();
  });
});
