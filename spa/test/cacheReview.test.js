// @vitest-environment jsdom
// The All-changes review plug against the local cache. The `diff` record IS
// the changeset: it paints whole, the wire is reached for only where there is
// no record or a push said it could not carry one, and a record rewritten
// under an open plug moves the stack.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { createReviewPlug } = await import("../src/core/changesReview.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { worktreeOf } = await import("./gitWireFixture.js");

const tree = worktreeOf({ "src/a.js": "new line" });

const PATCH = `diff --git a/src/a.js b/src/a.js
index 1111111..2222222 100644
--- a/src/a.js
+++ b/src/a.js
@@ -1,2 +1,2 @@
-old
+cached line
`;

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

let host, plug;

beforeEach(async () => {
  document.body.innerHTML = "";
  resetChangeEvents();
  await wipeCache();
  host = document.createElement("div");
  document.body.appendChild(host);
  plug = null;
});

describe("the saved aggregate diff", () => {
  /** A plug as a surface makes one: the cache of the machine the diff is on,
   *  handed down, and whatever else the case in hand wants. */
  const plugOn = (deviceId, options) => createReviewPlug({ cacheScope: scopeFor(deviceId), ...options });

  it("tracks the files actually visible in the all-changes scroller", async () => {
    const viewingContext = { setVisibleDiffs: vi.fn(), captureDomSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: PATCH })), entity: "run-1", viewingContext });
    plug.mount(host);
    await settle();
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(host, "all");
    viewingContext.setVisibleDiffs.mockClear();
    host.dispatchEvent(new Event("scroll"));
    await settle();
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(host, "all");
    plug.unmount();
  });

  it("paints whole — tray included — while the live one is being fetched", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, commentable: true });
    plug = plugOn("dev-1", { fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    // The chrome is not a round trip late: the comment affordances the last
    // live paint had are on the cached stack too.
    expect(host.querySelector(".fcmt")).toBeTruthy();
    plug.unmount();
  });

  it("stays read-only when the last live paint said so", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, commentable: false });
    plug = plugOn("dev-1", { fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    expect(host.querySelector(".fcmt")).toBeNull();
    plug.unmount();
  });

  it("asks the surface for nothing while the cache holds the diff", async () => {
    // What a live paint leaves behind: the body, and the one thing about the
    // changeset that rides no push — whether comments are open on it. A record
    // saying both is the whole answer.
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { patch: PATCH, commentable: true },
    );
    const fetchDiff = vi.fn(async () => ({ patch: PATCH.replace("cached line", "live line") }));
    plug = plugOn("dev-1", { fetchDiff, entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    expect(fetchDiff).not.toHaveBeenCalled();
    plug.unmount();
  });

  // A record the sync layer wrote is a BODY and nothing else: `commentable`
  // is read off the entity, and no push carries it. Such a record is what a
  // cold boot leaves on disk, so a surface that read it as the whole answer
  // would never learn the changeset was closed to comments.
  it("reads the surface once for a record that carries the body alone", async () => {
    // Byte for byte what core/cacheSync.js `diffRecord` writes on a cold pass.
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { patch: PATCH, diff_key: "k1", stale: false, projectId: "proj-1" },
    );
    const fetchDiff = vi.fn(async () => ({ patch: PATCH, commentable: true, projectId: "proj-1" }));
    plug = plugOn("dev-1", { fetchDiff, entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();

    expect(fetchDiff).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain("cached line");
    plug.unmount();
  });

  it("offers no comment box over a body-only record the surface calls closed", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { patch: PATCH, diff_key: "k1", stale: false, projectId: "proj-1" },
    );
    const fetchDiff = vi.fn(async () => ({ patch: PATCH, commentable: false, projectId: "proj-1" }));
    plug = plugOn("dev-1", { fetchDiff, entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();

    expect(fetchDiff).toHaveBeenCalledTimes(1);
    expect(host.querySelector(".fcmt")).toBeNull();
    plug.unmount();
  });

  it("reads the surface once when a push said the diff was too big to carry", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, stale: true });
    const live = PATCH.replace("cached line", "live line");
    const fetchDiff = vi.fn(async () => ({ patch: live }));
    plug = plugOn("dev-1", { fetchDiff, entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("live line");
    expect(fetchDiff).toHaveBeenCalledTimes(1);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe(live);
    plug.unmount();
  });

  it("moves the stack when a push rewrites the record under it", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH });
    plug = plugOn("dev-1", { fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");

    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { patch: PATCH.replace("cached line", "pushed line") },
    );
    await settle();
    expect(host.textContent).toContain("pushed line");
    plug.unmount();
  });

  it("does not let a late aggregate pull overwrite a newer diff record", async () => {
    let answer;
    const fetchDiff = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    plug = plugOn("dev-1", { fetchDiff, entity: "run-1" });
    plug.mount(host);
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"));

    const newer = PATCH.replace("cached line", "newer cache write");
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { patch: newer, commentable: true },
    );
    answer({ patch: PATCH.replace("cached line", "late pull"), commentable: true });
    await settle();

    expect(host.textContent).toContain("newer cache write");
    expect(host.textContent).not.toContain("late pull");
    expect((await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" })).value.patch).toBe(newer);
    plug.unmount();
  });

  it("redraws file hunks from a changeset body cache write", async () => {
    const file = tree.status().files[0];
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff" },
      { files: [file], commentable: true, diff_key: "d1" },
    );
    plug = plugOn("dev-1", {
      fetchDiff: vi.fn(() => new Promise(() => {})),
      fetchFiles: vi.fn(() => new Promise(() => {})),
      entity: "run-1",
    });
    plug.mount(host);
    await settle();
    expect(host.textContent).not.toContain("hunk from cache write");

    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "changesetdiff", sub: file.path },
      { content_key: file.content_key, patch: tree.diff({ paths: [file.path] }).files[0].patch.replace("new line", "hunk from cache write") },
    );
    await settle();

    expect(host.textContent).toContain("hunk from cache write");
    plug.unmount();
  });

  it("writes what it read off the wire through for the next mount", async () => {
    const live = PATCH.replace("cached line", "live line");
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: live, key: "building" })), entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("live line");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe(live);
    plug.unmount();
  });

  // The plug is made for one machine's diff: the scope the surface handed it is
  // what it is filed under, whatever machine the rest of this suite works.
  it("caches under the cacheScope it is handed", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH });
    // dev-2 holds none, so this plug reads its own and files it there.
    const live = PATCH.replace("cached line", "their line");
    plug = plugOn("dev-2", { fetchDiff: vi.fn(async () => ({ patch: live })), entity: "run-1" });
    plug.mount(host);
    await settle();

    expect((await readCached({ deviceId: "dev-2", entityId: "run-1", kind: "diff" })).value.patch).toBe(live);
    expect((await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" })).value.patch).toBe(PATCH);
    plug.unmount();
  });

  it("caches nothing for a surface that names no entity", async () => {
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: PATCH })), entity: null });
    plug.mount(host);
    await settle();
    expect(await readCached({ deviceId: "dev-1", entityId: "", kind: "diff" })).toBeUndefined();
    plug.unmount();
  });
});
