// @vitest-environment jsdom
// A fold is the reader's, and it lives where the reader's other choices live:
// in the controller. The DOM is drawn from it, so a repaint under a working
// agent — or a trip to another changeset and back — finds the file exactly as
// it was left.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountGitPane } from "../src/core/gitPane.js";
import { createReviewPlug, REVIEW_POLL_MS } from "../src/core/changesReview.js";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n`;

const status = (over = {}) => ({
  branch: "main",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [{ path: "src/a.js", staged: "none", index_status: "M", worktree_status: "M" }],
  files_truncated: false,
  stat: { files_changed: 1, insertions: 1, deletions: 1 },
  patch: patchFor("src/a.js", "first"),
  truncated: false,
  ...over,
});

const log = () => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier", author: "Zech", email: "z@x", time: 1 }],
  more: false,
});

const show = () => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "earlier",
  body: "",
  author: "Zech",
  email: "z@x",
  stat: { files_changed: 1, insertions: 1, deletions: 1 },
  patch: patchFor("src/b.js", "committed"),
  truncated: false,
});

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

const fileOf = (root, path) => root.querySelector(`.file[data-key$=":${path}"]`);

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("the Changes pane's folds", () => {
  const mount = async (served) => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = vi.fn(async (method) => {
      if (method === "git.status") return served.status;
      if (method === "git.log") return log();
      if (method === "git.show") return show();
      return {};
    });
    const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
    await settle();
    return { container, pane };
  };

  it("keeps a file the reader opened open when the patch moves underneath it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const served = { status: status() };
    const { container, pane } = await mount(served);
    await click(fileOf(container, "src/a.js").querySelector("td.code"));
    expect(fileOf(container, "src/a.js").classList.contains("capped")).toBe(false);

    served.status = status({ patch: patchFor("src/a.js", "the agent moved on"), head: "e".repeat(40) });
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(container.textContent).toContain("the agent moved on");
    expect(fileOf(container, "src/a.js").classList.contains("capped")).toBe(false);
    pane.dispose();
  });

  it("leaves no mark saying a file is open on one the reader shut", async () => {
    const { container, pane } = await mount({ status: status() });
    const file = () => fileOf(container, "src/a.js");
    await click(file().querySelector("td.code"));
    expect(file().classList.contains("capped")).toBe(false);
    await click(file().querySelector(".fpath"));
    expect(file().classList.contains("collapsed")).toBe(true);
    expect(file().hasAttribute("data-expanded")).toBe(false);
    pane.dispose();
  });

  it("shuts a file on its head, and opens it again on the next press", async () => {
    const { container, pane } = await mount({ status: status() });
    const head = () => fileOf(container, "src/a.js").querySelector(".fpath");
    await click(head());
    expect(fileOf(container, "src/a.js").classList.contains("collapsed")).toBe(true);
    await click(head());
    expect(fileOf(container, "src/a.js").classList.contains("collapsed")).toBe(false);
    expect(fileOf(container, "src/a.js").classList.contains("capped")).toBe(false);
    pane.dispose();
  });

  it("remembers the fold of a changeset the reader left and came back to", async () => {
    const { container, pane } = await mount({ status: status() });
    await click(fileOf(container, "src/a.js").querySelector("td.code"));
    await click(container.querySelector(".crow[data-hash]"));
    expect(fileOf(container, "src/b.js").classList.contains("capped")).toBe(true);
    await click(container.querySelector('.rrow[data-sel="uncommitted"]'));
    expect(fileOf(container, "src/a.js").classList.contains("capped")).toBe(false);
    pane.dispose();
  });
});

describe("the review plug's folds", () => {
  const mountPlug = (options = {}) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({ fetchDiff: async () => ({ patch: patchFor("src/a.js", "first") }), ...options });
    plug.mount(host);
    return { host, plug };
  };

  it("keeps a file the reader opened open across a poll that moved the diff", async () => {
    vi.useFakeTimers();
    let line = "first";
    const { host, plug } = mountPlug({ fetchDiff: async () => ({ patch: patchFor("src/a.js", line) }) });
    await vi.advanceTimersByTimeAsync(0);
    host.querySelector(".file td.code").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(fileOf(host, "src/a.js").classList.contains("capped")).toBe(false);

    line = "moved underneath";
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(host.textContent).toContain("moved underneath");
    expect(fileOf(host, "src/a.js").classList.contains("capped")).toBe(false);
    plug.unmount();
  });

  it("leaves a file the reader ticked off collapsed when the diff moves", async () => {
    vi.useFakeTimers();
    let line = "first";
    const { host, plug } = mountPlug({
      fetchDiff: async () => ({ patch: patchFor("src/a.js", line) }),
      submit: async () => {},
    });
    await vi.advanceTimersByTimeAsync(0);
    const box = host.querySelector(".fviewed-box");
    box.checked = true;
    box.dispatchEvent(new window.Event("change", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(fileOf(host, "src/a.js").classList.contains("collapsed")).toBe(true);

    line = "moved underneath";
    await vi.advanceTimersByTimeAsync(REVIEW_POLL_MS + 10);
    expect(fileOf(host, "src/a.js").classList.contains("collapsed")).toBe(true);
    plug.unmount();
  });

  it("opens a file the reader ticked off and then asked to read again", async () => {
    vi.useFakeTimers();
    const { host, plug } = mountPlug({ submit: async () => {} });
    await vi.advanceTimersByTimeAsync(0);
    const box = () => host.querySelector(".fviewed-box");
    box().checked = true;
    box().dispatchEvent(new window.Event("change", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(fileOf(host, "src/a.js").classList.contains("collapsed")).toBe(true);

    host.querySelector(".fpath").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(fileOf(host, "src/a.js").classList.contains("collapsed")).toBe(false);
    expect(fileOf(host, "src/a.js").classList.contains("capped")).toBe(false);
    expect(box().checked).toBe(true);
    plug.unmount();
  });
});
