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

// What `git.show` answers a caller that named `max_bytes` for a commit over
// it: the file headers, which say which files moved without saying how.
const headersOnly = () => ({
  ...show(),
  patch: "diff --git a/src/b.js b/src/b.js\nindex 1111111..2222222 100644\n--- a/src/b.js\n+++ b/src/b.js\n",
  truncated: true,
  patch_bytes: 400000,
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

// The whole of a mount, against a cache the sync layer has filled: the pane
// draws four records and asks the machine for nothing at all, and what moves
// it afterwards is a record moving.
describe("a pane over a filled cache", () => {
  const unpushed = () => ({ base: { kind: "push_target", label: "origin/main" }, diff_key: "d1" });

  const fill = async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "unpushed" }, unpushed());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, show());
  };

  it("paints the status, the commits and a held patch without asking the machine anything", async () => {
    await fill();
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).toContain("earlier work"); // the log
    expect(container.textContent).toContain("src/a.js"); // the status
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.textContent).toContain("why it happened"); // the held patch
    expect(never.mock.calls.filter(([method]) => method !== "git.diff")).toEqual([]);
    pane.dispose();
  });

  it("repaints when the commit record moves under it", async () => {
    await fill();
    const { container, pane } = await mountPane(vi.fn(() => new Promise(() => {})));
    expect(container.textContent).not.toContain("what landed since");
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { ...log(), commits: [{ ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "what landed since" }] },
    );
    await settle();
    expect(container.textContent).toContain("what landed since");
    pane.dispose();
  });

  it("reads exactly one patch for a commit nothing holds one for, and keeps it", async () => {
    await fill();
    await cache.deleteCached([{ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }]);
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(callRpc.mock.calls.filter(([method]) => method === "git.show")).toHaveLength(1);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) })).value.body)
      .toBe("why it happened");
    pane.dispose();
  });

  it("asks nothing of the machine for an hour of sitting there", async () => {
    // The clock is faked BEFORE the mount, so an interval the pane started
    // would be a fake one and the hour below would run it. Faking afterwards
    // leaves a real interval real, and an hour of fake time costs a
    // millisecond of the wall clock — which no poll would tick in.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await fill();
      const callRpc = liveRpc();
      const { pane } = await mountPane(callRpc);
      callRpc.mockClear();
      await vi.advanceTimersByTimeAsync(60 * 60 * 1000);
      expect(callRpc.mock.calls.filter(([method]) => method !== "git.diff")).toEqual([]);
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
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

  it("serves the held patch without asking the bridge for what cannot change", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, show());
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    const commitRow = container.querySelector(".crow");
    commitRow.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.textContent).toContain("why it happened");
    expect(callRpc.mock.calls.some(([method]) => method === "git.show")).toBe(false);
    pane.dispose();
  });

  it("writes the patch it read for the reader through for the next visit", async () => {
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    const commitRow = container.querySelector(".crow");
    commitRow.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    // Naming a cap is what a cache does: past it the bridge answers the file
    // headers, which say which files moved without saying how. A reader
    // opening a commit is asking how, so this read names none and is answered
    // the patch itself.
    expect(callRpc.mock.calls.find(([method]) => method === "git.show")[1].max_bytes).toBeUndefined();
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) });
    expect(record.value.body).toBe("why it happened");
    pane.dispose();
  });

  it("reads the patch behind a record the sync layer could only keep headers for", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, headersOnly());
    const callRpc = liveRpc();
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(callRpc.mock.calls.filter(([method]) => method === "git.show")).toHaveLength(1);
    expect(container.textContent).toContain("committed line"); // the diff, not the file list
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) });
    expect(record.value.truncated).toBe(false);
    pane.dispose();
  });

  it("asks once for a patch no record can take, however often the records move", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, headersOnly());
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      if (method === "git.show") return { ...show(), patch: "x".repeat(262145), truncated: true, patch_bytes: 262145 };
      return {};
    });
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(callRpc.mock.calls.filter(([method]) => method === "git.show")).toHaveLength(1);
    // The record still holds the headers — the cap is the cache's rule — but
    // the patch this mount read is not displaced by them.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await settle();
    container.querySelector('.rrow[data-sel="uncommitted"]').dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(callRpc.mock.calls.filter(([method]) => method === "git.show")).toHaveLength(1);
    pane.dispose();
  });

  it("keeps a patch too big for a record in hand and off the disk", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      if (method === "git.show") return { ...show(), patch: "x".repeat(262145), truncated: true };
      return {};
    });
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.textContent).toContain("why it happened");
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) })).toBeUndefined();
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

// A workspace source is on no board row, so no sync pass writes its records:
// the ones the cache holds are the last mount's own work. They are painted —
// the reader gets a full surface on the first frame — and then read anyway,
// because nothing else is going to.
describe("a pane over a checkout nothing walks", () => {
  const SOURCE = { workspace_id: "w-1", source_id: "s-1" };
  const ENTITY = `workspace:${JSON.stringify(["w-1", "s-1"])}`;

  const sourceRpc = () =>
    vi.fn(async (method, params) => {
      if (method === "git.status") return status({ head: "e".repeat(40) });
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { ...log(), commits: [{ ...log().commits[0], subject: "landed since" }] };
      if (method === "git.unpushed") return { patch: "", base: { kind: "empty" } };
      return {};
    });

  const fill = async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: ENTITY, kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: ENTITY, kind: "log" }, log());
  };

  it("paints what it holds and reads the checkout anyway", async () => {
    await fill();
    const callRpc = sourceRpc();
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    expect(callRpc.mock.calls.filter(([method]) => method === "git.status")).toHaveLength(1);
    expect(container.textContent).toContain("landed since");
    pane.dispose();
  });

  // The fourth record a mount reads. Its one consumer is the review over a
  // workspace source, which is measured against what the unpushed record says
  // the base is — so the rail says what the diff is against on the first frame,
  // rather than one round trip later.
  it("names what the review is against off the unpushed record", async () => {
    await fill();
    await cache.writeCached(
      { deviceId: "dev-1", entityId: ENTITY, kind: "unpushed" },
      { base: { kind: "push_target", label: "origin/main" }, diff_key: "d1" },
    );
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.unpushed") return new Promise(() => {}); // still in flight
      return sourceRpc()(method, params);
    });
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    expect(container.querySelector('.rrow[data-sel="review"]').textContent).toContain("vs origin/main");
    pane.dispose();
  });

  // ...and the record it reads is the one this pane wrote: nothing else ever
  // writes an unpushed record under a source. The sync pass files its own under
  // the board's entities, which a workspace source is not one of.
  it("keeps what the review is measured against where the next mount reads it", async () => {
    await fill();
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.unpushed")
        return { patch: "diff --git a/x b/x", base: { kind: "push_target", label: "origin/main" }, diff_key: "d1" };
      return sourceRpc()(method, params);
    });
    const { pane } = await mountPane(callRpc, { scope: SOURCE });
    await settle();

    const record = await cache.readCached({ deviceId: "dev-1", entityId: ENTITY, kind: "unpushed" });
    expect(record.value.base).toEqual({ kind: "push_target", label: "origin/main" });
    expect(record.value.diff_key).toBe("d1");
    // The body is the diff record's, under its own cap. This record is the
    // base, the commit list and the key, exactly as the sync pass writes one.
    expect(Object.hasOwn(record.value, "patch")).toBe(false);
    pane.dispose();
  });

  // The record is where the paged-in history lives, so that the next mount
  // opens on what the reader walked back to. A checkout this pane reads for
  // itself reads its first page on every mount — and a first page put over the
  // record wholesale would throw that history away every time.
  it("keeps the older history the reader paged in when it reads the checkout again", async () => {
    const older = { ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "older still" };
    await cache.writeCached({ deviceId: "dev-1", entityId: ENTITY, kind: "status" }, status());
    await cache.writeCached(
      { deviceId: "dev-1", entityId: ENTITY, kind: "log" },
      { ...log(), commits: [log().commits[0], older], more: false },
    );
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.log") return { ...log(), more: true }; // the latest page, and only it
      return sourceRpc()(method, params);
    });
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    await settle();

    expect(container.textContent).toContain("older still");
    const record = await cache.readCached({ deviceId: "dev-1", entityId: ENTITY, kind: "log" });
    expect(record.value.commits.map((commit) => commit.subject)).toEqual(["earlier work", "older still"]);
    // What the record says about there being more is about the history it
    // holds, not about the window this read walked.
    expect(record.value.more).toBe(false);
    pane.dispose();
  });

  it("says the review is against nothing yet where no record says otherwise", async () => {
    await fill();
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.unpushed") return new Promise(() => {});
      return sourceRpc()(method, params);
    });
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    expect(container.querySelector('.rrow[data-sel="review"]').textContent).toContain("Not pushed yet");
    pane.dispose();
  });

  it("leaves a run's records alone — those are kept true for it", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const callRpc = liveRpc();
    const { pane } = await mountPane(callRpc);
    expect(callRpc.mock.calls.filter(([method]) => method === "git.status")).toHaveLength(0);
    pane.dispose();
  });
});
