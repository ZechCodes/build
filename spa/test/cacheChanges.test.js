// @vitest-environment jsdom
// The Changes surface against the local cache: the synced status and commit
// list paint before the bridge answers, every live poll writes through (with
// the uncommitted patch emptied — it loads on demand), and a commit's detail
// is served from the cache without asking twice for what cannot change.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n`;

const status = (overrides = {}) => ({
  branch: "main",
  path: "/repo",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [{ path: "src/a.js", staged: "none", index_status: "M", worktree_status: "M" }],
  files_truncated: false,
  stat: { files_changed: 1, insertions: 6, deletions: 3 },
  patch: patchFor("src/a.js", "new line"),
  truncated: false,
  ...overrides,
});

const log = () => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier work", author: "Zech", email: "z@x", time: 1 }],
  more: false,
});

const show = () => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "earlier work",
  body: "why it happened",
  author: "Zech",
  email: "z@x",
  stat: { files_changed: 1, insertions: 1, deletions: 1 },
  patch: patchFor("src/b.js", "committed line"),
  truncated: false,
});

let mountGitPane, cache;

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = "";
  const { setCacheDevice } = await import("../src/core/cacheScope.js");
  setCacheDevice("dev-1");
  cache = await import("../src/core/localCache.js");
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
});

const mountPane = async (callRpc) => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
  await settle();
  return { container, pane };
};

const liveRpc = () =>
  vi.fn(async (method) => {
    if (method === "git.status") return status();
    if (method === "git.log") return log();
    if (method === "git.show") return show();
    return {};
  });

describe("the cached first paint", () => {
  it("paints the synced status and commit list before the bridge answers", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { ...status(), patch: "" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("earlier work");
    expect(container.textContent).not.toContain("loading…");
    pane.dispose();
  });

  it("stays on the loading state when nothing was ever synced", async () => {
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("loading…");
    pane.dispose();
  });
});

describe("the live write-through", () => {
  it("persists each poll's status and log, the uncommitted patch emptied", async () => {
    const { pane } = await mountPane(liveRpc());
    await settle();
    const cachedStatus = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(cachedStatus.value.head).toBe("f".repeat(40));
    expect(cachedStatus.value.patch).toBe("");
    expect(cachedStatus.value.files).toHaveLength(1);
    const cachedLog = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" });
    expect(cachedLog.value.commits[0].subject).toBe("earlier work");
    pane.dispose();
  });
});

describe("a commit's detail", () => {
  it("serves the cached payload without asking the bridge for what cannot change", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "a".repeat(40) }, show());
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    const commitRow = container.querySelector(".crow");
    commitRow.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.textContent).toContain("why it happened");
    expect(callRpc.mock.calls.some(([method]) => method === "git.show")).toBe(false);
    pane.dispose();
  });

  it("writes a freshly fetched detail through for the next visit", async () => {
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    const commitRow = container.querySelector(".crow");
    commitRow.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "a".repeat(40) });
    expect(record.value.body).toBe("why it happened");
    pane.dispose();
  });
});
