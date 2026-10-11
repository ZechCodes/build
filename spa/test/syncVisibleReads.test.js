// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { registerSyncRead, visibleSyncReads } = await import("../src/core/syncVisibleScopes.js");
const { mountFileTree } = await import("../src/core/fileTree.js");
const { createReviewPlug } = await import("../src/core/changesReview.js");
const { createWorkspaceReview, mountGitPane } = await import("../src/core/gitPane.js");
const { directoryCacheId } = await import("../src/core/directoryScope.js");
const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");

const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });
const settle = async () => { for (let turn = 0; turn < 20; turn++) await new Promise((done) => setTimeout(done, 0)); };
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
let disposers = [];
beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  document.body.innerHTML = "";
});
afterEach(async () => {
  vi.restoreAllMocks();
  disposers.forEach((dispose) => dispose());
  disposers = [];
  await settle();
  document.body.innerHTML = "";
});

async function mountedSourceRead(kind, { directDiff = false } = {}) {
  const scope = kind === "diff" ? { workspace_id: "w", source_id: "secondary" } : { project_id: "p", source_id: "secondary" };
  const entityId = directoryCacheId(scope);
  const key = address(entityId, kind);
  const status = { branch: "main", status_key: "held", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
  const held = {
    tree: { entries: [{ name: "held.txt", kind: "file" }] }, status,
    log: { branch: "main", commits: [], more: false },
    unpushed: { commits: [], diff_key: "held", base: { kind: "empty" } },
    diff: { patch: "", diff_key: "held", commentable: false },
  };
  for (const [recordKind, value] of Object.entries(held)) await writeCached(address(entityId, recordKind), value);
  const host = document.createElement("div");
  document.body.append(host);
  const response = deferred();
  let hold = false;
  const read = vi.fn(async (method) => {
    if (hold) return response.promise;
    return method === "git.status" ? status : held[method.slice(4)] || held.unpushed;
  });
  if (kind === "tree") {
    const tree = mountFileTree(host, {
      listingAddress: (sub) => address(entityId, "tree", sub), readsForItself: () => false,
      listDirectory: () => read("fs.tree"), finePointer: () => true, onOpen: () => {},
    });
    disposers.push(tree.dispose);
  } else if (kind === "diff") {
    const plug = directDiff
      ? createReviewPlug({ cacheScope: scopeFor("dev-1"), cacheEntity: entityId, fetchDiff: () => read("git.unpushed") })
      : createWorkspaceReview({ scope, callRpc: read, cacheScope: scopeFor("dev-1") });
    plug.mount(host);
    disposers.push(plug.unmount);
  } else {
    const pane = mountGitPane(host, { scope, callRpc: read, cacheScope: scopeFor("dev-1") });
    disposers.push(pane.dispose);
  }
  await settle();
  hold = true;
  read.mockClear();
  return { key, response, read, job: visibleSyncReads("dev-1").find(({ key: candidate }) => candidate.kind === kind) };
}

describe("visible sync read jobs", () => {
  it.each(["tree", "status", "log", "unpushed", "diff"])("fences a stopped pass's in-flight source %s answer", async (kind) => {
    const source = await mountedSourceRead(kind);
    const before = await readCached(source.key);
    let active = true;
    const reading = source.job.read(() => active);
    await vi.waitFor(() => expect(source.read).toHaveBeenCalled());
    active = false;
    source.response.resolve({ entries: [{ name: "late.txt", kind: "file" }], branch: "late", commits: [], diff_key: "late", status_key: "late", files: [], patch: "", base: { kind: "empty" } });
    await reading;
    expect(await readCached(source.key)).toEqual(before);
    if (kind === "diff") expect((await readCached({ ...source.key, kind: "unpushed" })).value.diff_key).toBe("held");
  });

  it.each(["tree", "status", "log", "unpushed", "diff"])("rechecks pass authority inside the source %s write transaction", async (kind) => {
    const source = await mountedSourceRead(kind, { directDiff: true });
    const before = await readCached(source.key);
    let active = true;
    const reading = source.job.read(() => active);
    await vi.waitFor(() => expect(source.read).toHaveBeenCalled());
    const transaction = IDBDatabase.prototype.transaction;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (this.name === "build-cache" && args[1] === "readwrite") active = false;
      return transaction.apply(this, args);
    });
    source.response.resolve({ entries: [{ name: "late.txt", kind: "file" }], branch: "late", commits: [], diff_key: "late", status_key: "late", files: [], patch: "", base: { kind: "empty" } });
    await reading;
    expect(active).toBe(false);
    expect(await readCached(source.key)).toEqual(before);
    if (kind === "diff") expect((await readCached({ ...source.key, kind: "unpushed" })).value.diff_key).toBe("held");
  });

  it("deduplicates addresses and rechecks the owning lifetime before a queued job reads", async () => {
    const key = address("directory", "status");
    const read = vi.fn().mockResolvedValue("landed");
    let active = true;
    disposers.push(registerSyncRead(() => key, read, () => active));
    disposers.push(registerSyncRead(() => ({ ...key }), read, () => active));
    expect(visibleSyncReads("dev-2")).toEqual([]);
    const jobs = visibleSyncReads("dev-1");
    expect(jobs).toHaveLength(1);
    expect(jobs[0].key).toEqual(key);
    active = false;
    await jobs[0].read();
    expect(read).not.toHaveBeenCalled();
    expect(visibleSyncReads("dev-1")).toEqual([]);
  });

  it.each([{ workspace_id: "w", source_id: "secondary" }, { project_id: "p", source_id: "secondary" }])(
    "awaits the mounted source tree's forced cache writes for %j", async (scope) => {
      const entityId = directoryCacheId(scope);
      const key = address(entityId, "tree");
      await writeCached(key, { entries: [{ name: "src", kind: "dir" }] });
      await writeCached(address(entityId, "tree", "src"), { entries: [{ name: "core", kind: "dir" }] });
      await writeCached(address(entityId, "tree", "src/core"), { entries: [{ name: "held.txt", kind: "file" }] });
      const host = document.createElement("div");
      document.body.append(host);
      const response = deferred();
      const listDirectory = vi.fn(() => response.promise);
      const tree = mountFileTree(host, {
        listingAddress: (sub) => address(entityId, "tree", sub), readsForItself: () => false,
        listDirectory, finePointer: () => true, onOpen: () => {},
      });
      disposers.push(tree.dispose);
      await tree.ready;
      await tree.reveal("src/core/held.txt");
      await settle();
      const job = visibleSyncReads("dev-1").find(({ key: candidate }) => candidate.entityId === entityId && candidate.kind === "tree");
      expect(job).toBeDefined();
      const reading = job.read();
      await settle();
      expect(listDirectory.mock.calls.map(([path]) => path)).toEqual(["", "src", "src/core"]);
      response.resolve({ entries: [{ name: "fresh.txt", kind: "file" }] });
      await reading;
      expect((await readCached(key)).value.entries[0].name).toBe("fresh.txt");
      host.hidden = true;
      expect(visibleSyncReads("dev-1")).toEqual([]);
    },
  );

  it("keeps sync-triggered source errors out of the mounted pane", async () => {
    const entityId = directoryCacheId({ project_id: "p", source_id: "secondary" });
    const treeAddress = address(entityId, "tree");
    await writeCached(treeAddress, { entries: [{ name: "held.txt", kind: "file" }] });
    const status = { branch: "main", status_key: "held", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
    await writeCached(address(entityId, "status"), status);
    await writeCached(address(entityId, "log"), { branch: "main", commits: [], more: false });
    const treeHost = document.createElement("div");
    const gitHost = document.createElement("div");
    document.body.append(treeHost, gitHost);
    const fail = () => { throw new Error("sync failure"); };
    const tree = mountFileTree(treeHost, {
      listingAddress: (sub) => address(entityId, "tree", sub), readsForItself: () => false,
      listDirectory: fail, finePointer: () => true, onOpen: () => {},
    });
    let failed = false;
    const callRpc = async (method) => {
      if (failed) return fail();
      return method === "git.status" ? status : { branch: "main", commits: [], more: false };
    };
    const pane = mountGitPane(gitHost, { scope: { project_id: "p", source_id: "secondary" }, callRpc, cacheScope: scopeFor("dev-1") });
    disposers.push(tree.dispose, pane.dispose);
    await settle();
    failed = true;
    for (const job of visibleSyncReads("dev-1")) await expect(Promise.resolve(job.read())).resolves.not.toBeInstanceOf(Error);
    await settle();
    expect(document.body.textContent).not.toContain("sync failure");
    expect((await readCached(treeAddress)).value.entries[0].name).toBe("held.txt");
  });

  it.each([{ workspace_id: "w", source_id: "secondary" }, { project_id: "p", source_id: "secondary" }])(
    "refreshes only status, history and unpushed metadata from the source pane for %j", async (scope) => {
      const entityId = directoryCacheId(scope);
      const status = { branch: "main", status_key: "held", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
      await writeCached(address(entityId, "status"), status);
      await writeCached(address(entityId, "log"), { branch: "main", commits: [], more: false });
      const callRpc = vi.fn(async (method) => {
        if (method === "git.status") return { ...status, status_key: "fresh" };
        if (method === "git.log") return { branch: "main", commits: [], more: false };
        if (method === "git.unpushed") return { commits: [], diff_key: "fresh", base: { kind: "empty" } };
        return {};
      });
      const host = document.createElement("div");
      document.body.append(host);
      const pane = mountGitPane(host, { scope, callRpc, cacheScope: scopeFor("dev-1") });
      disposers.push(pane.dispose);
      await settle();
      callRpc.mockClear();
      const jobs = visibleSyncReads("dev-1").filter(({ key }) => key.entityId === entityId && key.kind !== "diff");
      expect(jobs.map(({ key }) => key.kind)).toEqual(["status", "log", "unpushed"]);
      for (const job of jobs) await job.read();
      expect(callRpc.mock.calls.map(([method]) => method)).toEqual(["git.status", "git.log", "git.unpushed"]);
      expect(callRpc.mock.calls.at(-1)).toEqual(["git.unpushed", { ...scope, patch: false }]);
      expect((await readCached(address(entityId, "unpushed"))).value.diff_key).toBe("fresh");
      pane.setVisible(false);
      expect(visibleSyncReads("dev-1")).toEqual([]);
    },
  );

  it("awaits a mounted source Changes plug's conditional read and cache write", async () => {
    const entityId = directoryCacheId({ workspace_id: "w", source_id: "secondary" });
    const key = address(entityId, "diff");
    await writeCached(key, { patch: "", diff_key: "held", commentable: false });
    const response = deferred();
    const fetchDiff = vi.fn(() => response.promise);
    const host = document.createElement("div");
    document.body.append(host);
    const plug = createReviewPlug({ cacheScope: scopeFor("dev-1"), cacheEntity: entityId, fetchDiff });
    disposers.push(plug.unmount);
    plug.mount(host);
    await settle();
    const job = visibleSyncReads("dev-1").find(({ key: candidate }) => candidate.kind === "diff");
    expect(job).toBeDefined();
    let finished = false;
    const reading = Promise.resolve(job.read()).then(() => { finished = true; });
    await settle();
    expect(fetchDiff).toHaveBeenCalledExactlyOnceWith("held", { active: expect.any(Function) });
    expect(finished).toBe(false);
    response.resolve({ patch: "", diff_key: "fresh", commentable: false });
    await reading;
    expect((await readCached(key)).value.diff_key).toBe("fresh");
    plug.setVisible(false);
    expect(visibleSyncReads("dev-1")).toEqual([]);
  });
});
