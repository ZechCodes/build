// @vitest-environment jsdom
// The diff stack is a keyed list, and a repaint patches it. A file the reader
// is looking at is the same element afterwards, a tick that found the same
// patch writes nothing at all, and a file that grew above the viewport moves
// the stack rather than the reader.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountGitPane } from "../src/core/gitPane.js";
import { createReviewPlug, REVIEW_POLL_MS } from "../src/core/changesReview.js";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n`;

const TWO_FILES = patchFor("src/a.js", "first") + patchFor("src/b.js", "second");

const status = (over = {}) => ({
  branch: "main",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [
    { path: "src/a.js", staged: "none", index_status: "M", worktree_status: "M" },
    { path: "src/b.js", staged: "none", index_status: "M", worktree_status: "M" },
  ],
  files_truncated: false,
  stat: { files_changed: 2, insertions: 2, deletions: 2 },
  patch: TWO_FILES,
  truncated: false,
  ...over,
});

const log = () => ({ branch: "main", commits: [], more: false });

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

/** Everything the DOM under `target` did while `act` ran. */
const churn = async (target, act) => {
  const seen = [];
  const observer = new MutationObserver((records) => seen.push(...records));
  observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
  await act();
  seen.push(...observer.takeRecords());
  observer.disconnect();
  return seen;
};

/** Every write to `scrollTop`, so a tick that wrote nothing can be told from
 *  one that wrote back the number it already held. */
function watchScrollTop(element) {
  const writes = [];
  let held = element.scrollTop;
  Object.defineProperty(element, "scrollTop", {
    configurable: true,
    get: () => held,
    set: (value) => {
      writes.push(value);
      held = value;
    },
  });
  return writes;
}

const fileOf = (root, path) => root.querySelector(`.file[data-file="${path}"]`);

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("the Changes pane's stack", () => {
  const mount = async (served, scope = { project_id: "p1" }) => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = vi.fn(async (method) => {
      if (method === "git.status") return served.status;
      if (method === "git.log") return log();
      return {};
    });
    const pane = mountGitPane(container, { scope, callRpc });
    await settle();
    return { container, pane };
  };

  it("marks the stack as a keyed list of files", async () => {
    const { container, pane } = await mount({ status: status() });
    const stack = container.querySelector(".dstack");
    expect(stack.hasAttribute("data-keyed-list")).toBe(true);
    expect([...stack.children].map((child) => child.dataset.file)).toEqual(["src/a.js", "src/b.js"]);
    pane.dispose();
  });

  it("keys a triaged stack by file too, so a file arriving above leaves the one being read standing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const served = { status: status() };
    const { container, pane } = await mount(served, { run_id: "run-1" });
    const stack = container.querySelector(".dstack");
    expect([...stack.children].map((child) => child.dataset.key)).toEqual([
      "triagebar",
      "EDIT:src/a.js",
      "EDIT:src/b.js",
    ]);
    const held = fileOf(container, "src/b.js");

    served.status = status({ patch: patchFor("src/0.js", "arrived") + TWO_FILES, head: "e".repeat(40) });
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect([...stack.children].map((child) => child.dataset.key)).toEqual([
      "triagebar",
      "EDIT:src/0.js",
      "EDIT:src/a.js",
      "EDIT:src/b.js",
    ]);
    expect(fileOf(container, "src/b.js")).toBe(held);
    pane.dispose();
  });

  it("writes nothing at all when a tick finds the same patch", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const served = { status: status() };
    const { container, pane } = await mount(served);
    const scroller = container.querySelector(".cdetail-host");
    const writes = watchScrollTop(scroller);
    const records = await churn(scroller, async () => {
      // The repo moved — a fetch shifted the head — but the diff did not.
      served.status = status({ head: "e".repeat(40) });
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
    });
    expect(records).toEqual([]);
    expect(writes).toEqual([]);
    pane.dispose();
  });

  it("keeps the file the reader is on when another one changes under a poll", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const served = { status: status() };
    const { container, pane } = await mount(served);
    const held = fileOf(container, "src/b.js");
    served.status = status({ patch: patchFor("src/a.js", "the agent moved on") + patchFor("src/b.js", "second") });
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(container.textContent).toContain("the agent moved on");
    expect(fileOf(container, "src/b.js")).toBe(held);
    pane.dispose();
  });

  it("holds the file being read still when the one above it grows", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const served = { status: status() };
    const { container, pane } = await mount(served);
    const scroller = container.querySelector(".cdetail-host");
    // jsdom has no layout, so the test states one: every file is 400px tall,
    // and the one the agent added a line to is 600.
    const heightOf = (file) => (file.textContent.includes("the agent moved on") ? 600 : 400);
    const original = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () {
      if (this === scroller) return { top: 0, bottom: 300, height: 300 };
      if (!this.classList || !this.classList.contains("file")) return { top: 0, bottom: 0, height: 0 };
      let above = 0;
      for (const sibling of this.parentElement.children) {
        if (sibling === this) break;
        above += heightOf(sibling);
      }
      const top = above - scroller.scrollTop;
      return { top, bottom: top + heightOf(this), height: heightOf(this) };
    };
    try {
      scroller.scrollTop = 400; // the reader is at the head of b
      served.status = status({ patch: patchFor("src/a.js", "the agent moved on") + patchFor("src/b.js", "second") });
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
      expect(scroller.scrollTop).toBe(600);
    } finally {
      Element.prototype.getBoundingClientRect = original;
    }
    pane.dispose();
  });
});

describe("the review plug's stack", () => {
  const mountPlug = (options = {}) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({ fetchDiff: async () => ({ patch: TWO_FILES }), ...options });
    plug.mount(host);
    return { host, plug };
  };

  it("marks the stack as a keyed list of files", async () => {
    vi.useFakeTimers();
    const { host, plug } = mountPlug();
    await vi.advanceTimersByTimeAsync(0);
    const stack = host.querySelector(".dstack");
    expect(stack.hasAttribute("data-keyed-list")).toBe(true);
    expect([...stack.children].map((child) => child.dataset.file)).toEqual(["src/a.js", "src/b.js"]);
    plug.unmount();
  });

  it("keeps the file the reader is on when another one changes under the poll", async () => {
    vi.useFakeTimers();
    let patch = TWO_FILES;
    const { host, plug } = mountPlug({ fetchDiff: async () => ({ patch }) });
    await vi.advanceTimersByTimeAsync(0);
    const held = fileOf(host, "src/b.js");
    patch = patchFor("src/a.js", "the agent moved on") + patchFor("src/b.js", "second");
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.textContent).toContain("the agent moved on");
    expect(fileOf(host, "src/b.js")).toBe(held);
    plug.unmount();
  });
});
