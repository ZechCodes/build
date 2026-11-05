// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const deviceId = "synthetic-review-hints";
const projectId = "synthetic-project";
const workspaceId = "synthetic-workspace";
const snapshot = { id: "synthetic-snapshot" };
const directory = { id: "synthetic-directory", source_id: "synthetic-source",
  source_path: "/synthetic/source", path: "/synthetic/workspace/source",
  is_git: false, status: "not_git" };
const sourceScope = { kind: "project_source", project_id: projectId,
  source_id: directory.source_id, source_path: directory.source_path };
const file = (path, text) => ({ path, mime: "text/plain", size: text.length,
  content_b64: btoa(text), truncated: false, editable: false });
const entry = (name, kind = "file") => ({ name, kind, size: 1 });
const listing = (path) => ({ path, entries: path ? [entry("live.txt")] : [entry("src", "dir"), entry("docs", "dir")],
  has_more: false, next_cursor: null });
const address = (kind, path) => ({ deviceId, entityId: projectId, kind,
  sub: JSON.stringify(["synthetic-task", snapshot.id, directory.id, path, "head"]) });
const drain = async () => { for (let i = 0; i < 12; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
let cache, events, mountTaskReviewFiles, host, view, wire, read, subscriptions, modern, projectSources, text;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  Object.defineProperty(document, "hidden", { configurable: true, value: false });
  cache = await import("../src/core/localCache.js");
  events = await import("../src/core/changeEvents.js");
  ({ mountTaskReviewFiles } = await import("../src/core/taskReviewFiles.js"));
  modern = true; projectSources = true; text = "held selected bytes";
  subscriptions = new Map();
  read = vi.fn(async (method, params) => {
    if (method === "fs.tree") return listing(params.path);
    if (method === "fs.read") return file(params.path, text);
    throw new Error(`Unexpected read ${method}`);
  });
  wire = vi.fn(async (method, params) => {
    if (method === "session.hello") return { api_version: modern ? "3.35.0" : "3.34.0", push_events: true,
      capabilities: ["changes.subscriptions", "errors.codes", ...(projectSources ? ["changes.projectSources"] : []),
        ...(modern ? ["changes.filesHints", "fs.treePaged"] : [])],
      events: ["changes"], changes: { subscriptions: true, mode: "subscriptions",
        kinds: ["state", "files", "files.hints"], batch_ms: { min: 1000, max: 600000 } } };
    if (method === "changes.subscribe") { subscriptions.set(params.subscription_id, params); return { subscription_id: params.subscription_id, watch: "live" }; }
    if (method === "changes.unsubscribe") { subscriptions.delete(params.subscription_id); return { ok: true }; }
    return read(method, params);
  });
  host = document.createElement("div"); document.body.appendChild(host);
});
afterEach(() => { view?.dispose(); view = null; events.resetChangeEvents(); host.remove(); vi.restoreAllMocks(); });

const greet = async () => { await events.greetBridge(wire, { deviceId }); await events.subscriptionsSettled(); };
const mount = async (savedDirectory = directory) => {
  await greet();
  view = mountTaskReviewFiles(host, { deviceId, projectId, taskId: "synthetic-task", workspaceId,
    snapshot, directory: savedDirectory, callRpc: wire });
  await view.open("src/live.txt");
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain(text));
  await events.subscriptionsSettled(); await drain(); read.mockClear();
};
const statePush = () => events.dispatchChangeEvent({ type: "changes", items: [
  { entity_id: "board", state: { revision: 1, workspaces: [{ id: "unrelated-workspace" }] } },
  { entity_id: "unrelated-agent", state: { workspace_id: "unrelated-workspace" } },
] }, deviceId);
const hintPush = (paths, scope = sourceScope) => events.dispatchChangeEvent({ type: "changes",
  items: [{ project_source: scope, files: { paths, truncated: false } }] }, deviceId);

it("ignores unrelated board state under root-free hints", async () => {
  await mount();
  statePush(); await drain();
  expect(read).not.toHaveBeenCalled();
});

it("ignores original project source hints for a copied workspace directory", async () => {
  await mount();
  hintPush(["src/live.txt"], { ...sourceScope, source_id: "unrelated-source" });
  hintPush(["src/live.txt"], { ...sourceScope, source_path: "/synthetic/replaced-source" });
  hintPush(["docs/live.txt"]);
  hintPush(["src/live.txt"]); await drain();
  expect(read).not.toHaveBeenCalled();
  expect([...subscriptions.values()]).toEqual([expect.objectContaining({ scope: { kind: "board" }, kinds: ["state"] })]);
});

it("refreshes expanded visible directories and the selected workspace body on reconnect", async () => {
  await mount();
  text = "fresh selected bytes";
  events.refetchEverything(deviceId);
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain(text));
  await drain();
  expect(read.mock.calls.map(([method, params]) => [method, params.path]).sort()).toEqual([
    ["fs.read", "src/live.txt"], ["fs.tree", ""], ["fs.tree", "src"],
  ]);
});

it.each([false, true])("keeps live workspace state cold and refreshes explicitly (source capability %s)", async (capable) => {
  projectSources = capable;
  await mount(capable ? { ...directory, source_path: undefined } : directory);
  expect([...subscriptions.values()]).toEqual([expect.objectContaining({ scope: { kind: "board" }, kinds: ["state"] })]);
  statePush(); await drain(); expect(read).not.toHaveBeenCalled();
  text = "explicit refresh bytes";
  events.refetchEverything(deviceId);
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain(text));
  expect(read.mock.calls.some(([method]) => method === "fs.tree")).toBe(true);
});

it("keeps the legacy workspace files subscription and matching-file refresh", async () => {
  modern = false; await mount();
  expect([...subscriptions.values()]).toEqual([expect.objectContaining({ scope: { kind: "entity", id: workspaceId }, kinds: ["files"] })]);
  events.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "unrelated-workspace", files: { paths: ["src/live.txt"] } }] }, deviceId);
  await drain(); expect(read).not.toHaveBeenCalled();
  text = "legacy current bytes";
  events.dispatchChangeEvent({ type: "changes", items: [{ entity_id: workspaceId, files: { paths: ["src/live.txt"] } }] }, deviceId);
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain(text));
  expect(read.mock.calls.some(([method]) => method === "fs.read")).toBe(true);
});

it.each(["hidden", "disposed", "regreeted"])("rejects reconnect reads after their binding is %s", async (retired) => {
  await mount();
  const body = deferred(), tree = deferred();
  read.mockImplementation(async (method) => method === "fs.read" ? body.promise : tree.promise);
  events.refetchEverything(deviceId);
  await vi.waitFor(() => expect(read.mock.calls.map(([method]) => method).sort()).toEqual(["fs.read", "fs.tree", "fs.tree"]));
  if (retired === "hidden") host.hidden = true;
  else if (retired === "disposed") { view.dispose(); view = null; }
  else {
    read.mockImplementation(async () => new Promise(() => {}));
    await greet();
  }
  body.resolve(file("src/live.txt", "retired response bytes"));
  tree.resolve({ ...listing("src"), entries: [entry("retired.txt")] });
  await drain();
  expect((await cache.readCached(address("task-review-live-file", "src/live.txt"))).value.file.content_b64).toBe(btoa("held selected bytes"));
  expect((await cache.readCached(address("task-review-live-tree", "src"))).value.entries.map(({ name }) => name)).toEqual(["live.txt"]);
});


const leaseRoot = (fresh = false) => ({ path: "",
  entries: Array.from({ length: 100 }, (_, i) => entry(fresh && i === 99 ? "fresh.txt" : `cached-${String(i).padStart(3, "0")}.txt`)),
  has_more: !fresh, next_cursor: fresh ? null : "opaque-directory-lease" });
const mountCursorReview = async (readDirectory) => {
  read.mockImplementation(async (method, params) => method === "fs.tree"
    ? readDirectory(params) : file(params.path, "held lease preview"));
  await greet();
  view = mountTaskReviewFiles(host, { deviceId, projectId, taskId: "synthetic-task", workspaceId,
    snapshot, directory, callRpc: wire });
  await vi.waitFor(() => expect(host.querySelector('[data-path="cached-000.txt"]')).not.toBeNull());
  await view.open("cached-000.txt");
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain("held lease preview"));
  await events.subscriptionsSettled(); await drain(); read.mockClear();
};

it("keeps cached live-review rows and preview quiet during a refused directory continuation", async () => {
  const replacement = deferred();
  const message = "synthetic expired directory cursor";
  let recovering = false, replacementReads = 0;
  await mountCursorReview(async (params) => {
    if (params.cursor) {
      recovering = true;
      throw Object.assign(new Error(message), { code: "invalid_params" });
    }
    if (recovering) { replacementReads++; return replacement.promise; }
    return leaseRoot();
  });
  const header = host.querySelector(".trf-file-head").textContent;
  const notices = [];
  const observer = new MutationObserver(() => {
    if (host.querySelector(".trf-status").textContent.includes(message)) notices.push(message);
  });
  observer.observe(host, { subtree: true, childList: true, characterData: true });
  try {
    host.querySelector("[data-tree-more]").click();
    await vi.waitFor(() => expect(replacementReads).toBe(1));
    const failure = await cache.readCached(address("task-review-tree-error", ""));
    await drain();
    expect(host.querySelector('[data-path="cached-000.txt"]')).not.toBeNull();
    expect(host.querySelector(".trf-file-head").textContent).toBe(header);
    expect(host.querySelector(".trf-preview").textContent).toContain("held lease preview");
    expect(host.querySelector(".trf-status").textContent).toBe("");
    expect(failure?.value?.message || "").toBe("");
    expect(host.querySelector('.throbber[aria-label="loading"]')).toBeNull();
    expect(notices).toEqual([]);
    replacement.resolve(leaseRoot(true));
    await vi.waitFor(() => expect(host.querySelector('[data-path="fresh.txt"]')).not.toBeNull());
    expect(host.querySelector(".trf-file-head").textContent).toBe(header);
    expect(host.querySelector("[data-tree-more]")).toBeNull();
    expect(notices).toEqual([]);
    expect(read.mock.calls.filter(([method]) => method === "fs.tree").map(([, params]) => params.cursor || null))
      .toEqual(["opaque-directory-lease", null]);
  } finally { replacement.resolve(leaseRoot(true)); observer.disconnect(); }
});

it.each([
  { demand: "first-page", code: "invalid_params" },
  { demand: "continuation", code: "unavailable" },
])("keeps ordinary $demand directory errors visible in live review", async ({ demand, code }) => {
  const message = `synthetic ordinary ${demand} directory failure`;
  let fail = false;
  await mountCursorReview(async (params) => {
    if (fail && (demand === "first-page" || params.cursor)) throw Object.assign(new Error(message), { code });
    return leaseRoot();
  });
  fail = true;
  if (demand === "first-page") view.refresh();
  else host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(host.querySelector(".trf-status").textContent).toContain(message));
  expect(host.querySelector('[data-path="cached-000.txt"]')).not.toBeNull();
  expect(host.querySelector(".trf-preview").textContent).toContain("held lease preview");
});
