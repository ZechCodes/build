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

it("subscribes to the exact source and reads only visible demand named by its hints", async () => {
  await mount();
  expect([...subscriptions.values()]).toEqual([expect.objectContaining({ scope: sourceScope, kinds: ["files.hints"] })]);
  hintPush(["src/live.txt"], { ...sourceScope, source_id: "unrelated-source" });
  hintPush(["src/live.txt"], { ...sourceScope, source_path: "/synthetic/replaced-source" });
  hintPush(["docs/live.txt"]); await drain();
  expect(read).not.toHaveBeenCalled();
  text = "fresh selected bytes";
  hintPush(["src/live.txt"]);
  await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain(text));
  await drain();
  expect(read.mock.calls.map(([method, params]) => [method, params.path]).sort()).toEqual([
    ["fs.read", "src/live.txt"], ["fs.tree", "src"],
  ]);
});

it.each([false, true])("keeps modern fallback state cold and refreshes explicitly (source capability %s)", async (capable) => {
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

it.each(["hidden", "disposed", "regreeted"])("rejects hint reads after their binding is %s", async (retired) => {
  await mount();
  const body = deferred(), tree = deferred();
  read.mockImplementation(async (method) => method === "fs.read" ? body.promise : tree.promise);
  hintPush(["src/live.txt"]);
  await vi.waitFor(() => expect(read.mock.calls.map(([method]) => method).sort()).toEqual(["fs.read", "fs.tree"]));
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
