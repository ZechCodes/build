// @vitest-environment jsdom
// The review plug every "All changes" entry mounts: one stack, one comment
// layer, one actionbar the surface fills with its own verbs.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createReviewPlug, reviewBarHtml, emptyStackHtml, REVIEW_POLL_MS } from "../src/core/changesReview.js";

const patchOf = (line) =>
  [
    "diff --git a/a.txt b/a.txt",
    "index 0000000..1111111 100644",
    "--- a/a.txt",
    "+++ b/a.txt",
    "@@ -1 +1 @@",
    "-old",
    `+${line}`,
    "",
  ].join("\n");

describe("reviewBarHtml", () => {
  const files = [
    { path: "a.js", add: 3, del: 1 },
    { path: "b.js", add: 2, del: 0 },
  ];

  it("counts every file in the diff, not only what is drawn", () => {
    const html = reviewBarHtml(files);
    expect(html).toContain("2 files");
    expect(html).toContain("+5");
    expect(html).toContain("−1");
  });

  it("offers the changed-only filter only once there is a baseline", () => {
    expect(reviewBarHtml(files)).not.toContain("changedonly");
    const offered = reviewBarHtml(files, { offerChangedOnly: true, changedOnly: true });
    expect(offered).toContain("Only changes since my review");
    expect(offered).toContain("checked");
  });

  it("carries the surface's own live claim", () => {
    expect(reviewBarHtml(files, { statusHtml: '<span class="live-claim">working</span>' })).toContain("live-claim");
  });
});

describe("emptyStackHtml", () => {
  it("distinguishes a filtered-away stack from an empty diff", () => {
    expect(emptyStackHtml(4, true)).toContain("Nothing changed since your review.");
    expect(emptyStackHtml(0, true)).toContain("No file changes yet.");
    expect(emptyStackHtml(4, false)).toContain("No file changes yet.");
  });
});

describe("the review plug (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => vi.useRealTimers());

  const mountPlug = (options = {}) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({ fetchDiff: async () => ({ patch: patchOf("new") }), ...options });
    plug.mount(host);
    return { host, plug };
  };

  const commentOnTheFile = async (host) => {
    host.querySelector(".fcmt").click();
    await vi.advanceTimersByTimeAsync(0);
    document.querySelector(".cp-add").click();
    document.querySelector(".cp-input").value = "split this up";
    document.querySelector(".cp-save").click();
    await vi.advanceTimersByTimeAsync(0);
  };

  it("draws the stack and lets the surface own the actionbar while nothing is pending", async () => {
    const { host, plug } = mountPlug({
      submit: async () => {},
      renderIdleActions: (actions, hintHost) => {
        hintHost.textContent = "finish the worktree";
        actions.innerHTML = '<button class="mine">Merge</button>';
        return true;
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".file")).toBeTruthy();
    expect(host.querySelector(".csactions .mine")).toBeTruthy();
    expect(host.querySelector(".cshint").textContent).toBe("finish the worktree");
    plug.unmount();
  });

  it("hands the actionbar to Clear and Send while a comment is pending, and back after", async () => {
    const sent = [];
    const { host, plug } = mountPlug({
      submit: async (messages) => sent.push(messages),
      renderIdleActions: (actions) => {
        actions.innerHTML = '<button class="mine">Merge</button>';
        return true;
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    await commentOnTheFile(host);
    expect(host.querySelector(".csactions .mine")).toBe(null);
    expect(host.querySelector(".cssend")).toBeTruthy();

    host.querySelector(".cssend").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(sent[0][0].body).toBe("split this up");
    expect(host.querySelector(".pcomment")).toBe(null);
    expect(host.querySelector(".csactions .mine")).toBeTruthy();
    plug.unmount();
  });

  it("marks what moved since the comments went out, and can show only that", async () => {
    let line = "new";
    const { host, plug } = mountPlug({
      fetchDiff: async () => ({ patch: patchOf(line) }),
      submit: async () => {},
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".changedonly")).toBe(null); // no baseline yet

    await commentOnTheFile(host);
    host.querySelector(".cssend").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".changedonly")).toBeTruthy();
    expect(host.querySelector(".fchanged")).toBe(null); // nothing moved yet

    line = "newer";
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.querySelector(".fchanged").textContent).toContain("changed since your review");

    // Narrowing to what moved keeps the totals honest: the bar counts them all.
    host.querySelector(".changedonly-box").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelectorAll(".file")).toHaveLength(1);
    expect(host.querySelector(".diffbar").textContent).toContain("1 files");
    plug.unmount();
  });

  it("leaves the diff alone while a comment is pending", async () => {
    let line = "new";
    const { host, plug } = mountPlug({ fetchDiff: async () => ({ patch: patchOf(line) }), submit: async () => {} });
    await vi.advanceTimersByTimeAsync(0);
    await commentOnTheFile(host);
    line = "moved underneath";
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.querySelector(".pcomment").textContent).toContain("split this up");
    expect(host.textContent).not.toContain("moved underneath");
    plug.unmount();
  });

  // A drag is a comment that has not been said yet: the popover opens on the
  // pointerup (or after the handles settle), and until then nothing else knows
  // the reviewer is holding a range over these rows.
  it("leaves the diff alone while the reviewer is selecting code on it", async () => {
    let line = "new";
    const { host, plug } = mountPlug({ fetchDiff: async () => ({ patch: patchOf(line) }), submit: async () => {} });
    await vi.advanceTimersByTimeAsync(0);
    const code = host.querySelector("tr[data-ln] .code");
    const range = document.createRange();
    range.selectNodeContents(code);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);

    line = "moved underneath";
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.contains(code), "the rows the selection points into were replaced").toBe(true);
    expect(host.textContent).not.toContain("moved underneath");

    // Letting go hands the surface back: the next tick draws what moved.
    window.getSelection().removeAllRanges();
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.textContent).toContain("moved underneath");
    plug.unmount();
  });

  it("offers no comment affordances on a surface with nowhere to post", async () => {
    const { host, plug } = mountPlug({ fetchDiff: async () => ({ patch: patchOf("new"), commentable: false }) });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".fcmt")).toBe(null);
    expect(host.querySelector(".csgeneral")).toBe(null);
    expect(host.querySelector(".csactions")).toBeTruthy(); // the surface still has verbs
    plug.unmount();
  });
});
