// @vitest-environment jsdom
// The review plug every "All changes" entry mounts: one stack, one comment
// layer, one actionbar the surface fills with its own verbs.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createReviewPlug, reviewBarHtml, emptyStackText } from "../src/core/changesReview.js";

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

const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => (resolve = yes));
  return { promise, resolve };
};

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

describe("emptyStackText", () => {
  it("distinguishes a filtered-away stack from an empty diff", () => {
    expect(emptyStackText(4, true)).toContain("Nothing changed since your review.");
    expect(emptyStackText(0, true)).toContain("No file changes yet.");
    expect(emptyStackText(4, false)).toContain("No file changes yet.");
  });
});

describe("the review plug (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => vi.useRealTimers());

  /** The plug, and the git toolbar the surface hosts its verbs in — a separate
   *  bar above the stack, which is what gitPane hands it. */
  const mountPlug = (options = {}) => {
    const host = document.createElement("div");
    const toolbar = document.createElement("div");
    document.body.append(toolbar, host);
    const plug = createReviewPlug({ fetchDiff: async () => ({ patch: patchOf("new") }), ...options });
    plug.mount(host, { gitActions: () => toolbar });
    return { host, toolbar, plug };
  };

  const commentOnTheFile = async (host) => {
    host.querySelector(".fcmt").click();
    await vi.advanceTimersByTimeAsync(0);
    // The press IS the ask: the field is already there, no button in between.
    document.querySelector(".cp-input").value = "split this up";
    document.querySelector(".cp-save").click();
    await vi.advanceTimersByTimeAsync(0);
  };

  it("draws the stack and puts the surface's own verbs in the git toolbar", async () => {
    const { host, toolbar, plug } = mountPlug({
      submit: async () => {},
      renderIdleActions: (actions) => {
        actions.innerHTML = '<button class="mine">Merge</button>';
        return true;
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".file")).toBeTruthy();
    expect(toolbar.querySelector(".mine")).toBeTruthy();
    plug.unmount();
  });

  it("renders aggregate file timestamps and refreshes their text without replacing file nodes", async () => {
    const editedAt = Date.now() - 30_000;
    const { host, plug } = mountPlug({
      fetchDiff: async () => ({ patch: patchOf("new"), file_edited_at: { "a.txt": editedAt } }),
    });
    await vi.advanceTimersByTimeAsync(0);
    const file = host.querySelector(".file");
    const timestamp = host.querySelector(".fedited");
    expect(timestamp?.dataset.editedAt).toBe(String(editedAt));
    expect(timestamp?.textContent).toBe("Just Now");

    await vi.advanceTimersByTimeAsync(30_000);
    expect(host.querySelector(".file")).toBe(file);
    expect(host.querySelector(".fedited")).toBe(timestamp);
    expect(timestamp.textContent).toBe("1 minute ago");
    plug.unmount();
  });

  // Sending is the surface's box under the diff, not the plug's — so the plug
  // says what is waiting and sends when asked, and the merge verb in the
  // toolbar is untouched by a comment being written.
  it("keeps the pending comments, says how many, and sends them with the note", async () => {
    const sent = [];
    const { host, toolbar, plug } = mountPlug({
      submit: async (messages) => sent.push(messages),
      renderIdleActions: (actions) => {
        actions.innerHTML = '<button class="mine">Merge</button>';
        return true;
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    await commentOnTheFile(host);
    expect(plug.commentOffer()).toEqual({ commentable: true, pending: 1 });
    expect(host.querySelector(".pcomment")).toBeTruthy();
    expect(toolbar.querySelector(".mine")).toBeTruthy();

    await plug.sendComments();
    await vi.advanceTimersByTimeAsync(0);
    expect(sent[0][0].body).toBe("split this up");
    expect(host.querySelector(".pcomment")).toBe(null);
    expect(plug.commentOffer().pending).toBe(0);
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
    await plug.sendComments();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".changedonly")).toBeTruthy();
    expect(host.querySelector(".fchanged")).toBe(null); // nothing moved yet

    line = "newer";
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".fchanged").textContent).toContain("changed since your review");

    // Narrowing to what moved keeps the totals honest: the bar counts them all.
    host.querySelector(".changedonly-box").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelectorAll(".file")).toHaveLength(1);
    expect(host.querySelector(".diffbar").textContent).toContain("1 files");
    plug.unmount();
  });

  it("updates the diff without losing a pending comment", async () => {
    let line = "new";
    const requestedKeys = [];
    const { host, plug } = mountPlug({
      fetchDiff: async (ifDiffKey) => {
        requestedKeys.push(ifDiffKey);
        return { patch: patchOf(line), diff_key: line };
      },
      submit: async () => {},
    });
    await vi.advanceTimersByTimeAsync(0);
    await commentOnTheFile(host);
    line = "moved underneath";
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".pcomment").textContent).toContain("split this up");
    expect(host.textContent).toContain("moved underneath");
    expect(requestedKeys.at(-1)).toBe("new");
    await plug.sendComments();
    plug.unmount();
  });

  it("updates presentation metadata from an unchanged patch response", async () => {
    let commentable = true;
    const { host, plug } = mountPlug({
      fetchDiff: async (ifDiffKey) =>
        ifDiffKey
          ? { unchanged: true, diff_key: "same", key: commentable, commentable }
          : { patch: patchOf("new"), diff_key: "same", key: commentable, commentable },
      submit: async () => {},
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".fcmt")).toBeTruthy();
    commentable = false;
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".fcmt")).toBe(null);
    plug.unmount();
  });

  it("serializes overlapping refreshes and observes one queued invalidation", async () => {
    const first = deferred();
    const second = deferred();
    const fetchDiff = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { host, plug } = mountPlug({ fetchDiff });
    await vi.advanceTimersByTimeAsync(0);
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchDiff).toHaveBeenCalledTimes(1);

    first.resolve({ patch: patchOf("old"), diff_key: "old" });
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchDiff).toHaveBeenCalledTimes(2);
    expect(fetchDiff.mock.calls[1][0]).toBe("old");
    second.resolve({ patch: patchOf("new"), diff_key: "new" });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.textContent).toContain("new");
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
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.contains(code), "the rows the selection points into were replaced").toBe(true);
    expect(host.textContent).not.toContain("moved underneath");

    // Letting go hands the surface back: the next tick draws what moved.
    window.getSelection().removeAllRanges();
    plug.refreshDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(host.textContent).toContain("moved underneath");
    plug.unmount();
  });

  it("offers no comment affordances on a surface with nowhere to post", async () => {
    const { host, toolbar, plug } = mountPlug({
      fetchDiff: async () => ({ patch: patchOf("new"), commentable: false }),
      renderIdleActions: (actions) => {
        actions.innerHTML = '<button class="mine">Merge</button>';
        return true;
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(host.querySelector(".fcmt")).toBe(null);
    expect(plug.commentOffer().commentable).toBe(false);
    expect(toolbar.querySelector(".mine")).toBeTruthy(); // the surface still has verbs
    plug.unmount();
  });
});
