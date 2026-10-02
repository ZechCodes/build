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
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier work", author: "Ada", email: "z@x", time: 1 }],
  more: false,
});

const show = () => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "earlier work",
  body: "why it happened",
  author: "Ada",
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

let mountGitPane, cache, uiStore, scopeOf;

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
  uiStore = await import("../src/core/localUiStore.js");
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
});

const mountPaneNow = (callRpc, options = {}) => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeOf("dev-1"), ...options });
  return { container, pane };
};

const mountPane = async (callRpc, options = {}) => {
  const mounted = mountPaneNow(callRpc, options);
  await settle();
  return mounted;
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
  it("keeps inline comments from the mounted Git pane across uncommitted and commit views", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "ui-draft", sub: "changes:inline-comments" };
    const first = mountPaneNow(liveRpc());
    await vi.waitFor(() => expect(first.container.querySelector(".fcmt")).not.toBeNull());
    first.container.querySelector(".fcmt").click();
    await vi.waitFor(() => expect(document.querySelector(".cp-input")).not.toBeNull());
    document.querySelector(".cp-input").value = "Keep this inline note";
    document.querySelector(".cp-save").click();
    await vi.waitFor(async () => expect((await uiStore.readUiRecord(address))?.value.comments).toHaveLength(1));
    expect(await uiStore.readUiRecord({ ...address, sub: "changes:comments" })).toBeUndefined();
    first.pane.dispose();

    const sendRpc = liveRpc();
    const second = mountPaneNow(sendRpc);
    await vi.waitFor(() => expect(second.container.querySelector(".pcomment")?.textContent).toContain("Keep this inline note"));
    const other = mountPaneNow(liveRpc(), { scope: { run_id: "run-2" } });
    await vi.waitFor(() => expect(other.container.querySelector(".fcmt")).not.toBeNull());
    expect(other.container.querySelector(".pcomment")).toBeNull();
    other.pane.dispose();
    second.container.querySelector(".crow").click();
    await vi.waitFor(() => expect(second.container.querySelector(".pcomment")?.textContent).toContain("Keep this inline note"));
    second.container.querySelector(".csbox-actions .btn:not(.caret)").click();
    await vi.waitFor(async () => expect((await uiStore.readUiRecord(address))?.value.comments).toEqual([]));
    expect(sendRpc.mock.calls.some(([method]) => method === "run.request_changes")).toBe(true);
    second.pane.dispose();
  });

  it("restores diff sort and noise folds from the UI record, then repaints a cache write", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "ui-presentation", sub: "changes" };
    await uiStore.writeUiRecord(address, { sortOrder: "alphabetical", noiseExpanded: ["uncommitted"], fileFolds: {}, fileMenuPath: null });
    const { container, pane } = mountPaneNow(vi.fn(() => new Promise(() => {})));
    await vi.waitFor(() => expect(container.querySelector(".diffsort-select")?.value).toBe("alphabetical"));
    await uiStore.writeUiRecord(address, { sortOrder: "latest", noiseExpanded: [], fileFolds: {}, fileMenuPath: null });
    await vi.waitFor(() => expect(container.querySelector(".diffsort-select")?.value).toBe("latest"));
    pane.dispose();
  });
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

  it("redraws an open uncommitted file when its body record is written", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const never = vi.fn(() => new Promise(() => {}));
    const { container, pane } = await mountPane(never);
    expect(container.textContent).not.toContain("body from cache write");

    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "src/a.js" },
      {
        content_key: status().files[0].content_key,
        patch: patchFor("src/a.js", "body from cache write"),
        truncated: false,
      },
    );
    await settle();

    expect(container.textContent).toContain("body from cache write");
    pane.dispose();
  });

  it("does not let late status and log pulls overwrite newer records", async () => {
    let answerStatus, answerLog;
    const callRpc = vi.fn((method) => {
      if (method === "git.status") return new Promise((resolve) => { answerStatus = resolve; });
      if (method === "git.log") return new Promise((resolve) => { answerLog = resolve; });
      return new Promise(() => {});
    });
    const { container, pane } = await mountPane(callRpc);
    await vi.waitFor(() => expect([answerStatus, answerLog].every(Boolean)).toBe(true));

    const newerStatus = status({ head: "b".repeat(40) });
    const newerLog = {
      ...log(),
      commits: [{ ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "newer cache write" }],
    };
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, newerStatus);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, newerLog);
    await settle();

    answerStatus(status());
    answerLog({ ...log(), commits: [{ ...log().commits[0], subject: "late pull" }] });
    await settle();

    expect(container.textContent).toContain("newer cache write");
    expect(container.textContent).not.toContain("late pull");
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head)
      .toBe("b".repeat(40));
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

  it("does not let a late older-log page replace newer cached history", async () => {
    const initial = { ...log(), more: true };
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, initial);
    let answerPage;
    const callRpc = vi.fn((method) => method === "git.log"
      ? new Promise((resolve) => { answerPage = resolve; })
      : new Promise(() => {}));
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".gitmore").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.waitFor(() => expect(answerPage).toBeTypeOf("function"));

    const newerCommit = { ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "newer older page" };
    const newerLog = { ...initial, commits: [...initial.commits, newerCommit], more: false };
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, newerLog);
    answerPage({ ...initial, commits: [{ ...newerCommit, hash: "c".repeat(40), short: "ccccccc", subject: "late older page" }], more: false });
    await settle();

    expect(container.textContent).toContain("newer older page");
    expect(container.textContent).not.toContain("late older page");
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" })).value.commits.at(-1).subject)
      .toBe("newer older page");
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
  it("reports hand-selected commits so the workspace can keep its URL current", async () => {
    const onCommitSelection = vi.fn();
    const { container, pane } = mountPaneNow(liveRpc(), { onCommitSelection });
    await vi.waitFor(() => expect(container.querySelector(".crow")).not.toBeNull());
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    expect(onCommitSelection).toHaveBeenLastCalledWith("a".repeat(40));
    container.querySelector('[data-sel="uncommitted"]').dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    expect(onCommitSelection).toHaveBeenLastCalledWith(null);
    pane.dispose();
  });

  it("opens a routed workspace commit from cached history and cached diff", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, show());
    const callRpc = vi.fn(() => new Promise(() => {}));
    const onCommitSelection = vi.fn();
    const { container, pane } = mountPaneNow(callRpc, {
      scope: { workspace_id: "ws-1", source_id: "src", entity_id: "run-1" },
      review: { getBase: () => "main", getRailSubtitle: () => "vs main" },
      requestedCommit: "aaaaaaaa", onCommitSelection,
    });
    await vi.waitFor(() => expect(container.querySelector(".crow.sel")?.dataset.hash).toBe("a".repeat(40)));
    await vi.waitFor(() => expect(container.textContent).toContain("committed line"));
    expect(callRpc).not.toHaveBeenCalled();
    pane.dispose();
  });

  it("pages cached history for a routed commit, and names a missing one", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, { ...log(), more: true });
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.log" && params.skip === 1) return {
        ...log(), commits: [{ ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "older commit" }], more: false,
      };
      if (method === "git.show") return { ...show(), hash: "b".repeat(40), short: "bbbbbbb" };
      return {};
    });
    const { container, pane } = mountPaneNow(callRpc, { requestedCommit: "bbbbbbbb" });
    await vi.waitFor(() => expect(container.querySelector(".crow.sel")?.dataset.hash).toBe("b".repeat(40)));
    await vi.waitFor(() => expect(container.textContent).toContain("committed line"));
    expect(callRpc.mock.calls.some(([method, params]) => method === "git.log" && params.skip === 1)).toBe(true);
    pane.dispose();

    const absent = mountPaneNow(callRpc, { requestedCommit: "cccccccc" });
    await vi.waitFor(() => expect(absent.container.textContent).toContain("This commit isn't in this workspace's history."));
    expect(absent.container.querySelector(".crow.sel")).toBeNull();
    absent.pane.dispose();
  });

  it("recovers a missing routed commit when later cache records supply its log and diff", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const callRpc = vi.fn(() => new Promise(() => {}));
    const { container, pane } = mountPaneNow(callRpc, { requestedCommit: "bbbbbbbb" });
    await vi.waitFor(() => expect(container.textContent).toContain("This commit isn't in this workspace's history."));

    const older = { ...log().commits[0], hash: "b".repeat(40), short: "bbbbbbb", subject: "arrived later" };
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, {
      ...log(), commits: [...log().commits, older],
    });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: older.hash }, {
      ...show(), hash: older.hash, short: older.short,
    });
    await vi.waitFor(() => expect(container.querySelector(".crow.sel")?.dataset.hash).toBe(older.hash));
    await vi.waitFor(() => expect(container.textContent).toContain("committed line"));
    expect(container.textContent).not.toContain("This commit isn't in this workspace's history.");
    // The diff still paints from its cache record while any git.show call is
    // unanswered; the log announcement may race ahead of the patch write.
    expect(callRpc.mock.calls.every(([method]) => method === "git.show")).toBe(true);
    pane.dispose();
  });

  it("loads the routed commit diff while an explicit workspace refresh is still pending", async () => {
    const entityId = 'workspace:["ws-1","src"]';
    await cache.writeCached({ deviceId: "dev-1", entityId, kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId, kind: "log" }, log());
    const callRpc = vi.fn((method) => {
      if (method === "git.show") return Promise.resolve(show());
      return new Promise(() => {});
    });
    const { container, pane } = mountPaneNow(callRpc, {
      scope: { workspace_id: "ws-1", source_id: "src" },
      review: { getBase: () => "main", getRailSubtitle: () => "vs main" },
      requestedCommit: "aaaaaaaa",
    });
    const { refetchEverything } = await import("../src/core/changeEvents.js");
    refetchEverything();
    await vi.waitFor(() => expect(container.querySelector(".crow.sel")?.dataset.hash).toBe("a".repeat(40)));
    await vi.waitFor(() => expect(callRpc.mock.calls.some(([method]) => method === "git.show")).toBe(true));
    await vi.waitFor(() => expect(container.textContent).toContain("committed line"));
    expect(callRpc.mock.calls.some(([method]) => method === "git.status")).toBe(true);
    expect(callRpc.mock.calls.some(([method]) => method === "git.log")).toBe(true);
    pane.dispose();
  });

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

  // The freeze key is over the status and the commit list, which a patch
  // landing does not move. The pane draws the patch all the same, so nothing
  // in that key may be read as saying the frame on screen is still right.
  it("draws a patch the sync layer writes under the commit the reader has open", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.diff") return tree.diff(params);
      return new Promise(() => {}); // the pane's own git.show never answers
    });
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.textContent).not.toContain("why it happened");

    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) }, show());
    await settle();

    expect(container.textContent).toContain("why it happened");
    pane.dispose();
  });

  it("does not let a late git.show replace a newer patch record", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
    let answerShow;
    const callRpc = vi.fn((method, params) => {
      if (method === "git.diff") return Promise.resolve(tree.diff(params));
      if (method === "git.show") return new Promise((resolve) => { answerShow = resolve; });
      return new Promise(() => {});
    });
    const { container, pane } = await mountPane(callRpc);
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.waitFor(() => expect(answerShow).toBeTypeOf("function"));

    const newer = { ...show(), body: "newer explanation", patch: patchFor("src/b.js", "newer commit body") };
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) },
      newer,
    );
    answerShow({ ...show(), body: "late explanation", patch: patchFor("src/b.js", "late commit body") });
    await settle();

    expect(container.textContent).toContain("newer explanation");
    expect(container.textContent).toContain("newer commit body");
    expect(container.textContent).not.toContain("late explanation");
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) })).value.body)
      .toBe("newer explanation");
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
    // The patch read replaces the headers with a head kept beside its pages
    // (#95), and a record moving elsewhere does not send the pane back for it.
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) })).value)
      .toMatchObject({ paged: true });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, status());
    await settle();
    container.querySelector('.rrow[data-sel="uncommitted"]').dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(callRpc.mock.calls.filter(([method]) => method === "git.show")).toHaveLength(1);
    pane.dispose();
  });

  it("keeps a patch too big for a record in pages beside a head, not in hand", async () => {
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
    const head = (await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "a".repeat(40) })).value;
    expect(head).toMatchObject({ paged: true, truncated: true, body: "why it happened" });
    expect(head.patch).toBeUndefined();
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
// the ones the cache holds are the last mount's own work. Mounts reuse them;
// a push or an explicit action asks for fresh records.
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

  it("paints held status and history without reading either again", async () => {
    await fill();
    const callRpc = sourceRpc();
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    expect(callRpc.mock.calls.filter(([method]) => ["git.status", "git.log"].includes(method))).toHaveLength(0);
    expect(container.textContent).toContain("earlier work");
    pane.dispose();
  });

  it.each(["status", "log"])("reads only the missing %s record on mount", async (kind) => {
    await fill();
    await cache.deleteCached([{ deviceId: "dev-1", entityId: ENTITY, kind }]);
    const callRpc = sourceRpc();
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    expect(callRpc.mock.calls.filter(([method]) => ["git.status", "git.log"].includes(method)).map(([method]) => method)).toEqual([`git.${kind}`]);
    expect(container.querySelector(".crail-host")).not.toBeNull();
    pane.dispose();
  });

  it("refreshes status and history marked stale while the pane is mounted", async () => {
    await fill();
    const callRpc = sourceRpc();
    const { container, pane } = await mountPane(callRpc, { scope: SOURCE });
    await cache.writeCached({ deviceId: "dev-1", entityId: ENTITY, kind: "status" }, { ...status(), stale: true });
    await cache.writeCached({ deviceId: "dev-1", entityId: ENTITY, kind: "log" }, { ...log(), stale: true });
    await vi.waitFor(() => expect(container.textContent).toContain("landed since"));
    await vi.waitFor(async () => expect((await cache.readCached({ deviceId: "dev-1", entityId: ENTITY, kind: "status" }))?.value.stale).toBeUndefined());
    expect(callRpc.mock.calls.find(([method]) => method === "git.status")[1]).not.toHaveProperty("if_status_key");
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
  // itself refreshes after a push — a first page put over the record wholesale
  // would throw that history away every time.
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
    const { refetchEverything } = await import("../src/core/changeEvents.js");
    refetchEverything();
    await settle();

    expect(callRpc.mock.calls.filter(([method]) => method === "git.log")).toHaveLength(1);

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
