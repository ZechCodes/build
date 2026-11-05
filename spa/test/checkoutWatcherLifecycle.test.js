// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const ownership = vi.hoisted(() => ({ lazy: true }));
vi.mock("../src/core/scopedSyncRuntime.js", async (original) => ({ ...await original(), lazyViewReadsEnabled: () => ownership.lazy }));
const deviceId = "synthetic-checkout-lifecycle", entityId = "run-1";
const address = (kind, sub = "") => ({ deviceId, entityId, kind, sub });
const listing = (name = "held.txt") => ({ path: "", entries: [{ name, kind: "file", size: 1 }] });
const status = { branch: "dev", status_key: "held", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
const log = { commits: [], more: false };
const unpushed = { base: { kind: "empty", label: null }, commits: [], files: [], diff_key: "held-diff" };
let cache, events, host, mounted, wire, current, read;
const heldSubscriptions = new Map();
const hidden = (value) => Object.defineProperty(document, "hidden", { configurable: true, get: () => value });
const changeVisibility = (value) => { hidden(value); document.dispatchEvent(new Event("visibilitychange")); };
const capabilities = (version) => ["changes.subscriptions", "errors.codes", ...(version === "3.35.0" ? ["changes.filesHints", "fs.treePaged"] : [])];
const liveKinds = () => [...heldSubscriptions.values()].flatMap((spec) => spec.kinds || []);
beforeEach(async () => {
  vi.resetModules(); ownership.lazy = true; hidden(false); heldSubscriptions.clear(); mounted = null;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); events = await import("../src/core/changeEvents.js");
  host = document.createElement("div"); document.body.appendChild(host);
  current = "3.34.0";
  read = vi.fn(async (method) => ({ "fs.tree": listing(), "git.status": status, "git.log": log, "git.unpushed": unpushed })[method] || {});
  wire = vi.fn(async (method, params) => {
    if (method === "session.hello") return { api_version: current, push_events: true, capabilities: capabilities(current),
      events: ["changes"], changes: { subscriptions: true, mode: "subscriptions", kinds: ["state", "git", "files", "files.hints"], batch_ms: { min: 1000, max: 600000 } } };
    if (method === "changes.subscribe") { heldSubscriptions.set(params.subscription_id, params); return { subscription_id: params.subscription_id, watch: "live" }; }
    if (method === "changes.unsubscribe") { heldSubscriptions.delete(params.subscription_id); return { ok: true }; }
    return read(method, params);
  });
  await cache.writeCached(address("tree"), listing());
  await cache.writeCached(address("status"), status); await cache.writeCached(address("log"), log); await cache.writeCached(address("unpushed"), unpushed);
});
afterEach(() => { mounted?.dispose(); events.resetChangeEvents(); host.remove(); vi.restoreAllMocks(); hidden(false); });
const greet = async (version) => { current = version; await events.greetBridge(wire, { deviceId }); await events.subscriptionsSettled(); };
const settle = () => events.subscriptionsSettled();
const mountFiles = async () => {
  const written = cache.recordWriteOf(await cache.readCached(address("tree")));
  const { renderFilesTab } = await import("../src/views/files.js");
  const { scopeFor } = await import("../src/core/cacheScope.js");
  mounted = renderFilesTab(host, { roots: [{ id: "code", label: "Code", scope: { run_id: entityId } }],
    layoutEntityId: "synthetic-layout", callRpc: wire, cacheScope: scopeFor(deviceId) });
  await vi.waitFor(() => expect(host.querySelector('[data-path="held.txt"]')).not.toBeNull());
  if (ownership.lazy) await vi.waitFor(async () => expect(cache.recordWriteOf(await cache.readCached(address("tree")))).not.toBe(written));
  await settle();
};
const toggleRoot = async () => { host.querySelector('[data-root-head="code"]').click(); await settle(); };
const mountGit = async (scope = { run_id: entityId }) => {
  const { mountGitPane } = await import("../src/core/gitPane.js");
  const { scopeFor } = await import("../src/core/cacheScope.js");
  mounted = mountGitPane(host, { scope, callRpc: wire, cacheScope: scopeFor(deviceId) });
  // A clean bare project offers neither comments nor a commit composer.
  await vi.waitFor(() => expect(host.querySelector(".gitpane .changes2")).not.toBeNull()); await settle();
};

it("removes scoped 3.34 legacy body subscriptions when a Files root folds and reopens from cache", async () => {
  await greet("3.34.0"); await mountFiles(); expect(liveKinds()).toContain("files");
  await toggleRoot(); await vi.waitFor(() => expect(liveKinds()).not.toContain("files"));
  let release; const pending = new Promise((resolve) => { release = resolve; });
  read.mockClear(); read.mockImplementation(() => pending);
  await toggleRoot(); await vi.waitFor(() => expect(liveKinds()).toContain("files"));
  await vi.waitFor(() => expect(read.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1));
  expect(host.querySelector('[data-path="held.txt"]')).not.toBeNull();
  expect(host.querySelector('.throbber[aria-label="loading"]')).toBeNull();
  release(listing("returned.txt"));
  await vi.waitFor(() => expect(host.querySelector('[data-path="returned.txt"]')).not.toBeNull());
});

it("removes a scoped legacy body subscription on document hide and restores it on return", async () => {
  await greet("3.34.0"); await mountFiles();
  changeVisibility(true); await settle(); expect(liveKinds()).not.toContain("files");
  read.mockClear(); changeVisibility(false); await settle();
  await vi.waitFor(() => expect(liveKinds()).toContain("files"));
  await vi.waitFor(() => expect(read.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1));
});

it("retains modern root-free hints while folded and marks held cache stale without reading", async () => {
  await greet("3.35.0"); await mountFiles(); await toggleRoot();
  expect(liveKinds()).toContain("files.hints"); expect(liveKinds()).not.toContain("files"); read.mockClear();
  changeVisibility(true); await settle();
  const subscription_id = [...heldSubscriptions.keys()][0];
  events.dispatchChangeEvent({ type: "changes", subscription_id, items: [{ entity_id: entityId, files: { paths: [], truncated: true } }] }, deviceId);
  await vi.waitFor(async () => expect((await cache.readCached(address("tree"))).value.stale).toBe(true));
  expect(read).not.toHaveBeenCalled();
  mounted.dispose(); mounted = null; await settle();
  changeVisibility(true); changeVisibility(false); await settle(); expect(liveKinds()).toEqual([]);
});

it("does not add scoped watchers or reopen reads to the unchanged 3.33 Files path", async () => {
  ownership.lazy = false; await greet("3.33.0"); await mountFiles();
  read.mockClear(); await toggleRoot(); await toggleRoot(); changeVisibility(true); changeVisibility(false); await settle();
  expect(liveKinds()).toEqual([]); expect(read).not.toHaveBeenCalled();
});

it("retires a legacy frame awaiting cache admission across a root hide/show binding change", async () => {
  await greet("3.34.0"); await mountFiles();
  let releaseAdmission; const heldAdmission = new Promise((resolve) => { releaseAdmission = resolve; });
  const merge = cache.mergeCachedRecordsTogether;
  const intercepted = vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementationOnce(async (...args) => { await heldAdmission; return merge(...args); });
  events.dispatchChangeEvent({ type: "changes", subscription_id: [...heldSubscriptions.keys()][0],
    items: [{ entity_id: entityId, files: { root: listing("retired-frame.txt"), paths: [], truncated: true } }] }, deviceId);
  await vi.waitFor(() => expect(intercepted).toHaveBeenCalledTimes(1));
  await toggleRoot(); await vi.waitFor(() => expect(liveKinds()).not.toContain("files"));
  let releaseRead; const heldRead = new Promise((resolve) => { releaseRead = resolve; });
  read.mockClear(); read.mockImplementation(() => heldRead);
  await toggleRoot(); await vi.waitFor(() => expect(read.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1));
  releaseAdmission();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect((await cache.readCached(address("tree"))).value.entries).toEqual(listing().entries);
  releaseRead(listing("new-binding.txt"));
  await vi.waitFor(() => expect(host.querySelector('[data-path="new-binding.txt"]')).not.toBeNull());
});

it.each(["3.34.0", "3.35.0"])("releases hidden scoped Git body subscriptions and refreshes owned metadata on show (%s)", async (version) => {
  await greet(version); await mountGit(); expect(liveKinds()).toContain("git");
  mounted.setVisible(false); await settle(); expect(liveKinds()).toEqual([]);
  read.mockClear(); changeVisibility(true); await settle(); expect(liveKinds()).toEqual([]); expect(read).not.toHaveBeenCalled();
  changeVisibility(false); await settle(); expect(liveKinds()).toEqual([]);
  let release; const pending = new Promise((resolve) => { release = resolve; });
  read.mockImplementation(async (method) => { await pending; return ({ "git.status": status, "git.log": log, "git.unpushed": unpushed })[method] || {}; });
  mounted.setVisible(true); await settle();
  expect(liveKinds()).toContain("git");
  await vi.waitFor(() => expect(new Set(read.mock.calls.map(([method]) => method))).toEqual(new Set(["git.status", "git.log", "git.unpushed"])));
  expect(host.querySelector(".csinput")).not.toBeNull();
  expect(read.mock.calls.some(([method]) => ["git.show", "git.diff", "git.changeset_diff"].includes(method))).toBe(false);
  release();
});

it("releases a scoped visible Git watcher when the document hides", async () => {
  await greet("3.34.0"); await mountGit(); changeVisibility(true); await settle(); expect(liveKinds()).toEqual([]);
  read.mockClear(); changeVisibility(false); await settle(); expect(liveKinds()).toContain("git");
  await vi.waitFor(() => expect(read.mock.calls.some(([method]) => method === "git.unpushed")).toBe(true));
});

it("preserves an uncovered 3.33 Git pane's existing subscription across hide and show", async () => {
  ownership.lazy = false; await greet("3.33.0"); await mountGit({ project_id: "synthetic-project" });
  const ids = [...heldSubscriptions.keys()]; expect(ids.length).toBeGreaterThan(0); read.mockClear();
  mounted.setVisible(false); changeVisibility(true); await settle(); expect([...heldSubscriptions.keys()]).toEqual(ids);
  changeVisibility(false); mounted.setVisible(true); await settle(); expect([...heldSubscriptions.keys()]).toEqual(ids);
  expect(read).not.toHaveBeenCalled();
});
