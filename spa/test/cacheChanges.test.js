// @vitest-environment jsdom
// The Changes surface against the local cache: the synced status and commit
// list paint before the bridge answers, every live poll writes the shape
// through as received (each file's body is its own record), and a commit's
// detail is served from the cache without asking twice for what cannot change.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

import { patchFor, worktreeOf } from "./gitWireFixture.js";

const tree = worktreeOf({ "src/a.js": "new line" });
const status = (overrides = {}) => tree.status(overrides);

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

let mountGitPane, cache, scopeOf;

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = "";
  ({ scopeFor: scopeOf } = await import("../src/core/cacheScope.js"));
  cache = await import("../src/core/localCache.js");
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
});

const mountPane = async (callRpc, options = {}) => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeOf("dev-1"), ...options });
  await settle();
  return { container, pane };
};

const liveRpc = () =>
  vi.fn(async (method, params) => {
    if (method === "git.status") return status();
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return log();
    if (method === "git.show") return show();
    return {};
  });

describe("the cached first paint", () => {
  it("paints the synced status and commit list before the bridge answers", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("earlier work");
    expect(container.querySelector(".changes2")).not.toBe(null); // the pane, not its loading placeholder
    pane.dispose();
  });

  it("stays on the loading state when nothing was ever synced", async () => {
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("loading…");
    pane.dispose();
  });
});

// The pane is mounted for one machine while the rest of this suite works
// another: what it syncs is filed under the machine the view handed it.
describe("the scope the view hands it", () => {
  it("caches under the cacheScope it is handed", async () => {
    const { pane } = await mountPane(liveRpc(), { cacheScope: scopeOf("dev-2") });
    await settle();

    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.head).toBe("f".repeat(40));
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
    pane.dispose();
  });
});

describe("the live write-through", () => {
  it("persists each poll's status and log — the shape as received, no patch to strip", async () => {
    const { pane } = await mountPane(liveRpc());
    await settle();
    const cachedStatus = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(cachedStatus.value.head).toBe("f".repeat(40));
    expect(cachedStatus.value.patch).toBeUndefined();
    expect(cachedStatus.value.status_key).toBe(status().status_key);
    expect(cachedStatus.value.files).toHaveLength(1);
    const cachedLog = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" });
    expect(cachedLog.value.commits[0].subject).toBe("earlier work");
    pane.dispose();
  });

  it("persists each file's body under its own path, so a revisit expands offline", async () => {
    const { pane } = await mountPane(liveRpc());
    await settle();
    const body = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "src/a.js" });
    expect(body.value.patch).toBe(patchFor("src/a.js", "new line"));
    expect(body.value.content_key).toBe(status().files[0].content_key);
    pane.dispose();
  });

  it("paints a cached body without asking the bridge for it again", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "src/a.js" },
      { content_key: status().files[0].content_key, patch: patchFor("src/a.js", "new line"), truncated: false },
    );
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("new line");
    expect(never.mock.calls.some(([method]) => method === "git.diff")).toBe(false);
    pane.dispose();
  });
});

describe("a commit's detail", () => {
  it("addresses a selected commit by its full SHA", async () => {
    const viewingContext = { set: vi.fn(), setVisibleDiffs: vi.fn(), captureDomSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    const { container, pane } = await mountPane(liveRpc(), { viewingContext });
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(viewingContext.set).toHaveBeenLastCalledWith({ kind: "commit", sha: "a".repeat(40) });
    pane.dispose();
  });

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

describe("uncommitted viewing context", () => {
  it("remeasures visible diff paths after paint and scroll", async () => {
    const viewingContext = { set: vi.fn(), setVisibleDiffs: vi.fn(), captureDomSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    const { container, pane } = await mountPane(liveRpc(), { viewingContext });
    const scroller = container.querySelector(".cdetail-host");
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(scroller, "uncommitted");
    viewingContext.setVisibleDiffs.mockClear();
    scroller.dispatchEvent(new Event("scroll"));
    await settle();
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(scroller, "uncommitted");
    pane.dispose();
  });
});
