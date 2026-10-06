// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { mountTaskReviewChanges } = await import("../src/core/taskReviewChanges.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const uiState = await import("../src/core/localUiState.js");
const watchUiState = vi.spyOn(uiState, "watchUiState");
const { readUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");

const snapshot = { id: "snap-1" };
const directory = { id: "repo-a" };
const files = [{ path: "src/a.js", status: "Modified", additions: 1, deletions: 1, content_key: "v1" }];
const patch = "diff --git a/src/a.js b/src/a.js\nindex 111..222 100644\n--- a/src/a.js\n+++ b/src/a.js\n@@ -1 +1 @@\n-old\n+new\n";
const list = { files, stat: { files_changed: 1, insertions: 1, deletions: 1 }, diff_key: "base:head", files_truncated: false };
const waitForFile = (host) => vi.waitFor(() => expect(host.querySelector("[data-review-expand]")).not.toBeNull());
const waitForPatch = (host) => vi.waitFor(() => expect(host.querySelector('tr[data-side="new"]')?.dataset.newLine).toBe("1"));
const viewedAddress = uiState.uiAddress({ deviceId: "dev", entityId: "project", view: "task-review-changes", kind: "review", sub: JSON.stringify(["task", "repo-a"]) });

beforeEach(async () => { await wipeCache(); await wipeUiRecords(); watchUiState.mockClear(); document.body.innerHTML = ""; });

describe("snapshot Changes", () => {
  it("paints a cached project record, then reports a missing source without erasing it", async () => {
    const address = { deviceId: "dev", entityId: "project", kind: "task-review-changes", sub: JSON.stringify(["task", "snap-1", "repo-a"]) };
    await writeCached(address, list);
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn().mockRejectedValue(new Error("Source unavailable"));
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await vi.waitFor(() => {
      expect(host.textContent).toContain("src/a.js");
      expect(host.textContent).toContain("Source unavailable");
    });
    expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ mode: "changes", patch: false, directory_id: "repo-a" }));
    pane.dispose();
  });

  it("isolates same paths by directory and sends an exact anchored line", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const onComment = vi.fn();
    const onOpenFile = vi.fn();
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : list);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc, onOpenFile, onComment });
    await waitForFile(host);
    host.querySelector("[data-review-expand]").click();
    await vi.waitFor(() => expect(host.querySelector('[data-review-comment][data-side="new"]')).not.toBeNull());
    host.querySelector('[data-review-comment][data-side="new"]').click();
    expect(onComment).toHaveBeenCalledWith({ snapshot_id: "snap-1", directory_id: "repo-a", path: "src/a.js", side: "new", line: 1 });
    host.querySelector("[data-open-file]").click();
    expect(onOpenFile).toHaveBeenCalledWith("src/a.js");
    const address = { deviceId: "dev", entityId: "project", kind: "task-review-patch", sub: JSON.stringify(["task", "snap-1", "repo-a", "src/a.js", "old..new"]) };
    expect((await readCached(address)).value.patch).toContain("+new");
    pane.dispose();
  });

  it("announces a capped listing and leaves a late patch in its original directory", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    let answerPatch;
    const pending = new Promise((resolve) => { answerPatch = resolve; });
    const callRpc = vi.fn((_method, params) => params.paths ? pending : Promise.resolve({ ...list, files_truncated: true }));
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await vi.waitFor(() => expect(host.textContent).toContain("first 1,000 changed files"));
    host.querySelector("[data-review-expand]").click();
    // Join the patch consumer so the negative cache checks run after it finishes.
    const reading = pane.reveal({ path: "src/a.js", side: "new", line: 1 });
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ paths: ["src/a.js"] })));
    pane.dispose();
    answerPatch({ ...list, patch });
    await reading;
    const oldAddress = { deviceId: "dev", entityId: "project", kind: "task-review-patch", sub: JSON.stringify(["task", "snap-1", "repo-a", "src/a.js", "old..new"]) };
    const otherAddress = { ...oldAddress, sub: JSON.stringify(["task", "snap-1", "repo-b", "src/a.js", "old..new"]) };
    expect(await readCached(oldAddress)).toBeUndefined();
    expect(await readCached(otherAddress)).toBeUndefined();
  });

  it("keeps a cut patch in body pages and reads the next range on demand", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const first = patch.slice(0, 72);
    const second = patch.slice(72);
    const callRpc = vi.fn(async (_method, params) => {
      if (!params.paths) return list;
      if (!params.range) return { ...list, patch: first, truncated: true };
      const offset = params.range.offset;
      return { ...list, patch: offset ? second : first, range: { offset, end: offset ? patch.length : first.length, total: patch.length, version: "patch-v1" } };
    });
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await waitForFile(host);
    host.querySelector("[data-review-expand]").click();
    await vi.waitFor(() => expect(host.querySelector("[data-review-more]")?.textContent).toContain(`${first.length} of ${patch.length}`));
    const address = { deviceId: "dev", entityId: "project", kind: "task-review-patch", sub: JSON.stringify(["task", "snap-1", "repo-a", "src/a.js", "old..new"]) };
    expect((await readCached(address)).value).toMatchObject({ paged: true, of: "patch-v1" });
    expect((await readCached(address)).value.patch).toBeUndefined();
    host.querySelector("[data-review-more]").click();
    await vi.waitFor(() => expect(host.querySelector("[data-review-more]")).toBeNull());
    expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ range: { offset: first.length, bytes: expect.any(Number) } }));
    pane.dispose();
  });

  it("opens and highlights an initial old-side anchor", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : list);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc,
      anchor: { path: "src/a.js", side: "old", line: 1 } });
    // The file opens before the patch's cache write and readback apply the anchor.
    await vi.waitFor(() => {
      expect(host.querySelector("tr.task-review-anchor")?.dataset.oldLine).toBe("1");
      expect(host.querySelector('[data-review-path="src/a.js"] [data-review-expand]')?.getAttribute("aria-expanded")).toBe("true");
    });
    pane.dispose();
  });

  it("reveals a path beyond the truncated listing, but returns false for an unchanged path", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const short = { ...list, files: [], files_truncated: true };
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : short);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await vi.waitFor(() => expect(host.textContent).toContain("first 1,000 changed files"));
    const revealed = await pane.reveal({ path: "src/a.js", side: "new", line: 1 });
    expect(revealed).toBe(true);
    expect(host.querySelector("tr.task-review-anchor").dataset.newLine).toBe("1");
    pane.dispose();

    const completeHost = document.createElement("div");
    document.body.append(completeHost);
    const complete = mountTaskReviewChanges(completeHost, { deviceId: "dev", projectId: "project", taskId: "other-task", snapshot, directory,
      callRpc: async () => ({ ...list, files: [], files_truncated: false }) });
    await vi.waitFor(() => expect(completeHost.textContent).toContain("No committed changes"));
    expect(await complete.reveal({ path: "README.md", side: "new", line: 1 })).toBe(false);
    complete.dispose();
  });

  it("waits for an initial anchor read when reveal is called again", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    let answerPatch;
    const pending = new Promise((resolve) => { answerPatch = resolve; });
    const callRpc = vi.fn(async (_method, params) => params.paths ? pending : list);
    const anchor = { path: "src/a.js", side: "new", line: 1 };
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc, anchor });
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ paths: ["src/a.js"] })));
    const resolved = vi.fn();
    const again = pane.reveal(anchor).then(resolved);
    // Complete a listing read while the patch is deliberately still pending.
    await pane.refresh();
    expect(resolved).not.toHaveBeenCalled();
    answerPatch({ ...list, patch });
    await again;
    expect(resolved).toHaveBeenCalledWith(true);
    expect(callRpc.mock.calls.filter(([, params]) => params.paths)).toHaveLength(1);
    pane.dispose();
  });

  it("rechecks an open cached patch error after the source recovers", async () => {
    const listAddress = { deviceId: "dev", entityId: "project", kind: "task-review-changes", sub: JSON.stringify(["task", "snap-1", "repo-a"]) };
    const patchAddress = { ...listAddress, kind: "task-review-patch", sub: JSON.stringify(["task", "snap-1", "repo-a", "src/a.js", "old..new"]) };
    await writeCached(listAddress, { ...list, read_error: "Source unavailable" });
    await writeCached(patchAddress, { patch, content_key: "v1", read_error: "Source unavailable" });
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch: "" } : list);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await waitForFile(host);
    host.querySelector("[data-review-expand]").click();
    await waitForPatch(host);
    await pane.refresh();
    await vi.waitFor(() => expect(host.textContent).not.toContain("Source unavailable"));
    expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ paths: ["src/a.js"], patch: false }));
    expect((await readCached(patchAddress)).value.read_error).toBeUndefined();
    expect(host.textContent).not.toContain("Source unavailable");
    pane.dispose();
  });

  it("keeps keyboard focus on the same file control through repaint", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : list);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await waitForFile(host);
    const expand = host.querySelector("[data-review-expand]");
    expand.focus();
    expand.click();
    await waitForPatch(host);
    expect(document.activeElement).toBe(host.querySelector('[data-review-path="src/a.js"] [data-review-expand]'));
    const viewed = host.querySelector("[data-review-viewed]");
    viewed.focus();
    viewed.click();
    await watchUiState.mock.results.at(-1).value.flush();
    expect(document.activeElement).toBe(host.querySelector('[data-review-path="src/a.js"] [data-review-viewed]'));
    pane.dispose();
  });

  it("keeps focus on Load more while pages remain, then moves to the file toggle", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const cuts = [patch.slice(0, 42), patch.slice(42, 83), patch.slice(83)];
    const ends = [42, 83, patch.length];
    const callRpc = vi.fn(async (_method, params) => {
      if (!params.paths) return list;
      if (!params.range) return { ...list, patch: cuts[0], truncated: true };
      const index = [0, 42, 83].indexOf(params.range.offset);
      return { ...list, patch: cuts[index], range: { offset: params.range.offset, end: ends[index], total: patch.length, version: "patch-v1" } };
    });
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await waitForFile(host);
    host.querySelector("[data-review-expand]").click();
    await vi.waitFor(() => expect(host.querySelector("[data-review-more]")?.textContent).toContain(`42 of ${patch.length}`));
    const firstMore = host.querySelector("[data-review-more]");
    firstMore.focus();
    firstMore.click();
    await vi.waitFor(() => expect(host.querySelector("[data-review-more]")?.textContent).toContain(`83 of ${patch.length}`));
    expect(document.activeElement).toBe(host.querySelector("[data-review-more]"));
    document.activeElement.click();
    await vi.waitFor(() => expect(host.querySelector("[data-review-more]")).toBeNull());
    expect(document.activeElement).toBe(host.querySelector("[data-review-expand]"));
    pane.dispose();
  });

  it("keeps a viewed mark across snapshots only for the same file content", async () => {
    const mountSnapshot = async (id, contentKey, source = directory) => {
      const host = document.createElement("div");
      document.body.append(host);
      const answer = { ...list, files: [{ ...files[0], content_key: contentKey }] };
      const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot: { id }, directory: source,
        callRpc: async () => answer });
      await waitForFile(host);
      // Negative viewed checks must run after saved UI state has hydrated.
      await watchUiState.mock.results.at(-1).value.ready;
      return { host, pane };
    };
    const first = await mountSnapshot("snap-1", "content-a");
    first.host.querySelector("[data-review-viewed]").click();
    await vi.waitFor(async () => expect((await readUiRecord(viewedAddress))?.value.viewed["src/a.js"]).toBe("content-a"));
    expect(first.host.querySelector("[data-review-viewed]").getAttribute("aria-pressed")).toBe("true");
    first.pane.dispose();

    const unchanged = await mountSnapshot("snap-2", "content-a");
    expect(unchanged.host.querySelector("[data-review-viewed]").getAttribute("aria-pressed")).toBe("true");
    unchanged.pane.dispose();

    const edited = await mountSnapshot("snap-3", "content-b");
    expect(edited.host.querySelector("[data-review-viewed]").getAttribute("aria-pressed")).toBe("false");
    edited.pane.dispose();

    const otherDirectory = await mountSnapshot("snap-4", "content-a", { id: "repo-b" });
    expect(otherDirectory.host.querySelector("[data-review-viewed]").getAttribute("aria-pressed")).toBe("false");
    otherDirectory.pane.dispose();
  });
});
