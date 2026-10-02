// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { mountTaskReviewChanges } = await import("../src/core/taskReviewChanges.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");

const snapshot = { id: "snap-1" };
const directory = { id: "repo-a" };
const files = [{ path: "src/a.js", status: "Modified", additions: 1, deletions: 1, content_key: "v1" }];
const patch = "diff --git a/src/a.js b/src/a.js\nindex 111..222 100644\n--- a/src/a.js\n+++ b/src/a.js\n@@ -1 +1 @@\n-old\n+new\n";
const list = { files, stat: { files_changed: 1, insertions: 1, deletions: 1 }, diff_key: "base:head", files_truncated: false };
const settle = async () => { for (let i = 0; i < 12; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };

beforeEach(async () => { await wipeCache(); document.body.innerHTML = ""; });

describe("snapshot Changes", () => {
  it("paints a cached project record, then reports a missing source without erasing it", async () => {
    const address = { deviceId: "dev", entityId: "project", kind: "task-review-changes", sub: JSON.stringify(["task", "snap-1", "repo-a"]) };
    await writeCached(address, list);
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn().mockRejectedValue(new Error("Source unavailable"));
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await settle();
    expect(host.textContent).toContain("src/a.js");
    expect(host.textContent).toContain("Source unavailable");
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
    await settle();
    host.querySelector("[data-review-expand]").click();
    await settle();
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
    await settle();
    expect(host.textContent).toContain("first 1,000 changed files");
    host.querySelector("[data-review-expand]").click();
    await settle();
    pane.dispose();
    answerPatch({ ...list, patch });
    await settle();
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
    await settle();
    host.querySelector("[data-review-expand]").click();
    await settle();
    const address = { deviceId: "dev", entityId: "project", kind: "task-review-patch", sub: JSON.stringify(["task", "snap-1", "repo-a", "src/a.js", "old..new"]) };
    expect((await readCached(address)).value).toMatchObject({ paged: true, of: "patch-v1" });
    expect((await readCached(address)).value.patch).toBeUndefined();
    host.querySelector("[data-review-more]").click();
    await settle();
    expect(callRpc).toHaveBeenCalledWith("tasks.review.diff", expect.objectContaining({ range: { offset: first.length, bytes: expect.any(Number) } }));
    pane.dispose();
  });

  it("opens and highlights an initial old-side anchor", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : list);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc,
      anchor: { path: "src/a.js", side: "old", line: 1 } });
    await settle();
    expect(host.querySelector('[data-review-path="src/a.js"] [data-review-expand]').getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelector("tr.task-review-anchor").dataset.oldLine).toBe("1");
    pane.dispose();
  });

  it("reveals a path beyond the truncated listing, but returns false for an unchanged path", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const short = { ...list, files: [], files_truncated: true };
    const callRpc = vi.fn(async (_method, params) => params.paths ? { ...list, patch } : short);
    const pane = mountTaskReviewChanges(host, { deviceId: "dev", projectId: "project", taskId: "task", snapshot, directory, callRpc });
    await settle();
    const revealed = await pane.reveal({ path: "src/a.js", side: "new", line: 1 });
    expect(revealed).toBe(true);
    expect(host.querySelector("tr.task-review-anchor").dataset.newLine).toBe("1");
    pane.dispose();

    const completeHost = document.createElement("div");
    document.body.append(completeHost);
    const complete = mountTaskReviewChanges(completeHost, { deviceId: "dev", projectId: "project", taskId: "other-task", snapshot, directory,
      callRpc: async () => ({ ...list, files: [], files_truncated: false }) });
    await settle();
    expect(await complete.reveal({ path: "README.md", side: "new", line: 1 })).toBe(false);
    complete.dispose();
  });
});
