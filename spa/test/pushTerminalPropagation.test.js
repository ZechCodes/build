// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { passesSettled } from "./waits.js";
import { directoryCacheId } from "../src/core/directoryScope.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, projectSources: true,
    kinds: ["state", "thread", "git", "files", "terminals", "tasks"] }, term: { projectSources: true } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));
let context;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => [context],
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));
const scope = { project_id: "p1", source_id: "source", source_path: "/repo" };
const projects = ["p1", "p2"].map((id) => ({ project_id: id, sources: [{ id: "source", path: "/repo" }] }));
const board = Array.from({ length: 4 }, (_, index) => ({ kind: "branch", state: "building", project_id: index < 2 ? "p1" : "p2",
  run_id: `run-${index}`, worktree_id: `wt-${index}`, agents: [{ id: `agent-${index}`, conversation_id: `c${index}`, thread_id: `thread:c${index}` }] }));
const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });
const rosterAddress = address(directoryCacheId(scope), "terminals");
const sourceFact = (termId, source = scope) => ({ project_source: source, terminals: { tabs: [{ term_id: termId }] } });
const threadItems = (sequence) => board.map((row, index) => ({ entity_id: row.run_id, thread: [{
  agent_id: `agent-${index}`, conversation_id: `c${index}`, thread_id: `thread:c${index}`,
  since_sequence: sequence - 1, last_sequence: sequence,
  items: [{ type: "message", data: { id: `m-${index}-${sequence}`, sequence, role: "assistant", body: "agent activity" } }],
}] }));
let cache, sync, active;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = []; active = true;
  const answers = {
    "board.list": () => ({ items: board }), "project.list": () => ({ projects }), "workspace.list": () => ({ workspaces: [] }),
    "thread.page": (params) => ({ thread_id: params.thread_id, items: [], has_more: false }),
    "term.list": () => ({ terminals: [] }), "tasks.list": () => ({ tasks: [] }), "tasks.columns": () => ({ columns: [] }),
    "fs.tree": () => ({ entries: [] }), "git.status": () => ({ files: [], status_key: "initial" }),
    "git.log": () => ({ commits: [] }), "git.unpushed": () => ({ commits: [] }),
  };
  context = { deviceId: "dev-1", session: {}, greeted: Promise.resolve(), active: () => active,
    cacheScope: { deviceId: "dev-1", active: () => active },
    rpc: vi.fn(async (method, params = {}) => (answers[method] || (() => ({})))(params)) };
  cache = await import("../src/core/localCache.js"); sync = await import("../src/core/cacheSync.js");
  sync.startCacheSync(); await passesSettled(sync, "dev-1");
});
afterEach(async () => { sync.stopCacheSync(); vi.restoreAllMocks(); await passesSettled(sync, "dev-1"); });

it("propagates source terminal rosters past queued projects writes during multi-project activity", async () => {
  let db, pending = 0, peak = 0;
  const original = IDBDatabase.prototype.transaction;
  const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    db = this;
    const opened = original.apply(this, args);
    peak = Math.max(peak, ++pending);
    opened.addEventListener("complete", () => { pending -= 1; });
    opened.addEventListener("abort", () => { pending -= 1; });
    return opened;
  });
  await cache.readCanonicalCached(address("", "projects"));
  let held = true;
  const blocker = db.transaction("records", "readwrite"), store = blocker.objectStore("records");
  const keepAlive = () => { if (held) store.get("keep-alive").onsuccess = keepAlive; };
  keepAlive();
  const projectsWriter = cache.mergeCachedAtomically(address("", "projects"), (rows) => rows);
  await new Promise((resolve) => setTimeout(resolve, 0));
  const subscriber = watchers.find((entry) => entry.kinds.includes("thread"));
  let timer, rounds = 100;
  try {
    for (let sequence = 1; sequence <= rounds; sequence += 1) subscriber.onChanges(threadItems(sequence));
    const propagated = sync.applyProjectSourceChanges([sourceFact("live-terminal")], "dev-1");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(peak).toBeLessThanOrEqual(16);
    timer = setInterval(() => { rounds += 1; subscriber.onChanges(threadItems(rounds)); }, 5);
    await new Promise((resolve) => setTimeout(resolve, 15));
    const releasedAt = performance.now();
    held = false;
    await projectsWriter; await propagated;
    await vi.waitFor(async () => expect((await cache.readCached(rosterAddress))?.value.tabs).toEqual([{ term_id: "live-terminal" }]), { timeout: 2000 });
    expect(performance.now() - releasedAt).toBeLessThan(2000);
    expect(rounds).toBeGreaterThan(100);
    expect(peak).toBeLessThanOrEqual(16);
    await sync.applyProjectSourceChanges([sourceFact("stale-location", { ...scope, source_path: "/moved" })], "dev-1");
    expect((await cache.readCached(rosterAddress))?.value.tabs).toEqual([{ term_id: "live-terminal" }]);
  } finally {
    held = false; clearInterval(timer); await projectsWriter; transaction.mockRestore();
  }
});

it("fences source terminal rosters when the captured session ends", async () => {
  await sync.applyProjectSourceChanges([sourceFact("held-terminal")], "dev-1");
  const original = cache.readCached;
  const reading = vi.spyOn(cache, "readCached").mockImplementationOnce(async (key) => {
    const result = await original(key); active = false; return result;
  });
  try { await sync.applyProjectSourceChanges([sourceFact("retired-terminal")], "dev-1"); }
  finally { reading.mockRestore(); }
  expect((await cache.readCached(rosterAddress))?.value.tabs).toEqual([{ term_id: "held-terminal" }]);
});
