// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { cacheFileBody } = await import("../src/core/cacheLifetime.js");
const { mountTaskReviewFiles } = await import("../src/core/taskReviewFiles.js");

const snapshot = { id: "snap-1" };
const git = { id: "dir-1", source_id: "source-1", is_git: true, status: "git", head: "abc" };
const live = { id: "dir-live", source_id: "source-live", is_git: false, status: "not_git" };
const textFile = (path, text) => ({ path, mime: "text/plain", size: text.length, content_b64: btoa(text), truncated: false, editable: false });
const tick = async () => { for (let i = 0; i < 8; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };
const address = (directory, path, kind = "task-review-tree", snap = snapshot) => ({
  deviceId: "device", entityId: "project", kind,
  sub: JSON.stringify(["task", snap.id, directory.id, path, "head"]),
});

let hosts = [];
const mount = (directory, callRpc, options = {}) => {
  const host = document.createElement("div");
  document.body.append(host);
  const view = mountTaskReviewFiles(host, { deviceId: "device", projectId: "project", taskId: "task", workspaceId: "workspace", snapshot, directory, callRpc, ...options });
  hosts.push({ host, view });
  return { host, view };
};

beforeEach(async () => { await wipeCache(); await wipeUiRecords(); });
afterEach(() => { hosts.forEach(({ host, view }) => { view.dispose(); host.remove(); }); hosts = []; });

describe("task review Files", () => {
  it("lists the saved full head and opens unchanged blobs without filesystem RPC", async () => {
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      expect(params).toMatchObject({ task_id: "task", snapshot_id: "snap-1", directory_id: "dir-1" });
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "unchanged.txt", kind: "file", size: 9 }] };
      return textFile(params.path, "unchanged");
    });
    const { host, view } = mount(git, callRpc);
    await tick();
    expect(host.textContent).toContain("unchanged.txt");
    await view.open("unchanged.txt");
    await tick();
    expect(host.textContent).toContain("unchanged");
    expect(await readCached(address(git, ""))).toMatchObject({ value: { entries: [{ name: "unchanged.txt" }] } });
    expect(await readCached(address(git, "unchanged.txt", "task-review-file"))).toMatchObject({ value: { file: { path: "unchanged.txt" } } });
  });

  it("keeps equal paths in separate directories and makes live reads exact", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (method === "fs.read") return textFile(params.path, params.source_id);
      throw new Error("unexpected RPC");
    });
    const other = { ...live, id: "dir-other", source_id: "source-other" };
    const first = mount(live, callRpc);
    const second = mount(other, callRpc);
    await first.view.open("same.txt");
    await second.view.open("same.txt");
    await tick();
    expect(first.host.textContent).toContain("dir-live");
    expect(second.host.textContent).toContain("dir-other");
    expect(callRpc.mock.calls.filter(([method]) => method.startsWith("fs.")).map(([, params]) => params.source_id)).toEqual(expect.arrayContaining(["dir-live", "dir-other"]));
    expect(await readCached(address(live, "same.txt", "task-review-live-file"))).toBeTruthy();
    expect(await readCached(address(other, "same.txt", "task-review-live-file"))).toBeTruthy();
  });

  it("shows failed refresh after cached paint and confines late reads to old keys", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (params.snapshot_id === "snap-1" && params.mode === "blob") return new Promise((resolve) => { callRpc.resolveOld = resolve; });
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (params.mode === "blob") return textFile(params.path, "new body");
      throw new Error("Source unavailable");
    });
    const old = mount(git, callRpc);
    await old.view.open("same.txt");
    await tick();
    old.view.dispose();
    const next = mount(git, callRpc, { snapshot: { id: "snap-2" } });
    await next.view.open("same.txt");
    await tick();
    callRpc.resolveOld(textFile("same.txt", "old body"));
    await tick();
    expect(next.host.textContent).toContain("new body");
    expect(next.host.textContent).not.toContain("old body");
    expect(await readCached(address(git, "same.txt", "task-review-file"))).toBeTruthy();
    expect(await readCached(address(git, "same.txt", "task-review-file", { id: "snap-2" }))).toBeTruthy();
  });

  it("marks a missing live source unavailable while preserving cached rows and bytes", async () => {
    await writeCached(address(live, "", "task-review-live-tree"), { path: "", entries: [{ name: "same.txt", kind: "file" }] });
    await cacheFileBody({ ...address(live, "same.txt", "task-review-live-file"), kind: "task-review-live-file",
      path: address(live, "same.txt").sub, file: textFile("same.txt", "held bytes") });
    const callRpc = vi.fn(async () => { throw new Error("Source unavailable"); });
    const { host, view } = mount(live, callRpc);
    await view.open("same.txt");
    await tick();
    expect(host.textContent).toContain("held bytes");
    expect(host.querySelector('.frow[data-path="same.txt"]')).toBeTruthy();
    expect(host.textContent).toContain("Source unavailable");
    expect(callRpc.mock.calls.every(([, params]) => params.source_id === live.id)).toBe(true);
  });

  it("keeps a large review blob as pages at its own cache key", async () => {
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "large.txt", kind: "file", size: 1_048_577 }] };
      if (!params.range) return { ...textFile(params.path, "first line\n"), size: 1_048_577, truncated: true };
      return { ...textFile(params.path, "first line\n"), size: 1_048_577,
        range: { offset: 0, end: 11, total: 11, version: "blob-oid" } };
    });
    const { host, view } = mount(git, callRpc);
    await view.open("large.txt");
    await tick();
    expect(host.textContent).toContain("first line");
    expect((await readCached(address(git, "large.txt", "task-review-file"))).value.file).toMatchObject({ paged: true, of: "blob-oid" });
    expect(callRpc.mock.calls.some(([, params]) => params.range?.offset === 0)).toBe(true);
    view.dispose();
    const remount = mount(git, vi.fn(async () => { throw new Error("offline"); }), { path: "large.txt" });
    await tick();
    expect(remount.host.textContent).toContain("first line");
    expect(remount.host.textContent).toContain("offline");
  });

  it("shows an empty committed tree when a saved Git head has no commits", async () => {
    const unborn = { ...git, head: null, status: "no_commits" };
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      expect(params.mode).toBe("tree");
      return { path: params.path, entries: [] };
    });
    const { host } = mount(unborn, callRpc);
    await tick();
    expect(host.textContent).toContain("Empty directory.");
    expect(callRpc).toHaveBeenCalledTimes(1);
  });
});
