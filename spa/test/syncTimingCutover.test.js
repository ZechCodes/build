// @vitest-environment jsdom
// #894 extends #869's logical RPC clock with the real sync planner, scoped
// engine, fake IndexedDB transactions and taskFeed row publication. RPC time
// is a model; wall time is this process's fake-IDB work, never production disk.
import { appendFileSync } from "node:fs";
import { hrtime } from "node:process";
import { setImmediate as immediate } from "node:timers";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBCursor, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceRecord, deviceRecords } from "./fixtures/deviceSync.js";

const App = { route: { name: "inbox" }, devices: [{ id: "cutover" }] };
let context, peerVersion, listeners, greetingListeners, watchers;
const capabilities = () => ({ agents: { projectWorkers: true }, tasks: { listPaged: true },
  threads: { leanActivity: true }, sync: { device: true, projectTasks: true, task: true },
  changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } });
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, homeContext: () => context, liveContexts: () => context ? [context] : [],
  canAnswer: () => true, creationDevice: () => "cutover",
  captureDeviceGreeting: () => context?.greeting,
  deviceContextIdentity: () => context, deviceSession: () => context?.session,
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeApiVersion: () => peerVersion, bridgeCapabilities: () => capabilities(),
  onBridgeGreeted: (listener) => { greetingListeners.add(listener); return () => greetingListeners.delete(listener); },
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));

const EPOCH = Date.parse("2026-11-01T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;
const COST_MS = { "sync.snapshot": 100, "sync.changes": 100, "sync.head": 100,
  "board.list": 100, "project.list": 100, "workspace.list": 100, "agent.list": 100,
  "thread.page": 500, "thread.activity": 500, "tasks.list": 350, "tasks.columns": 100,
  "git.status": 600, "fs.tree": 200, "term.list": 100, "git.log": 200,
  "git.unpushed": 400, "git.show": 500, "run.diff": 300, "worktree.diff": 300 };
let sync, cache, feed, idbTasks, metrics, unobserve;
const emptyMetrics = () => ({ idbCallbacks: 0, cursorContinuations: 0, cacheNotifications: 0,
  rowPublications: 0, idbDrainWallMs: 0 });

beforeEach(async () => {
  vi.resetModules(); context = null; peerVersion = "3.34.0";
  listeners = new Set(); greetingListeners = new Set(); watchers = [];
  metrics = emptyMetrics(); idbTasks = 0;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  Object.defineProperty(navigator, "locks", { configurable: true, value: {
    request: (_name, acquire) => Promise.resolve().then(acquire),
  } });
  vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(EPOCH);
  vi.spyOn(performance, "now").mockImplementation(() => Date.now() - EPOCH);
  vi.stubGlobal("requestIdleCallback", (callback) => { queueMicrotask(callback); return 0; });
  vi.stubGlobal("setImmediate", (callback) => {
    idbTasks += 1; metrics.idbCallbacks += 1;
    return immediate(() => { try { callback(); } finally { idbTasks -= 1; } });
  });
  const advance = IDBCursor.prototype.continue;
  vi.spyOn(IDBCursor.prototype, "continue").mockImplementation(function (...args) {
    metrics.cursorContinuations += 1; return advance.apply(this, args);
  });
  App.route = { name: "inbox" };
  cache = await import("../src/core/localCache.js");
  await cache.writeCached({ deviceId: "cutover", entityId: "", kind: "project-agents-support" }, { projectWorkers: true });
  sync = await import("../src/core/cacheSync.js");
  feed = await import("../src/core/taskFeed.js");
  unobserve = cache.subscribeCache({ deviceId: "cutover" }, () => { metrics.cacheNotifications += 1; });
});
afterEach(() => {
  unobserve?.(); feed?.stopFeed(); sync?.stopCacheSync(); context = null;
  delete navigator.locks;
  vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks();
});

async function cacheTurns() {
  const started = hrtime.bigint(); let quiet = 0;
  for (let turn = 0; turn < 100000; turn += 1) {
    await new Promise(immediate);
    quiet = idbTasks ? 0 : quiet + 1;
    if (quiet === 2) { metrics.idbDrainWallMs += Number(hrtime.bigint() - started) / 1e6; return; }
  }
  throw new Error("IndexedDB did not quiesce");
}

function fixtureRows({ projects, workspaces, agents }) {
  const templates = new Map(deviceRecords().map(({ kind, row }) => [kind, row]));
  const rows = [], records = [];
  for (let index = 0; index < projects; index += 1) {
    const id = `project-${index}`;
    records.push(deviceRecord("project", id, { ...templates.get("project"), id, project_id: id,
      name: `Synthetic project ${index}`, path: `/synthetic/${id}` }));
    records.push(deviceRecord("project_source", `${id}/source`, { ...templates.get("project_source"),
      id: "source", project_id: id, path: `/synthetic/${id}` }));
  }
  for (let index = 0; index < workspaces; index += 1) {
    const projectId = `project-${index % projects}`, workspaceId = `workspace-${index}`, owner = `run-${index}`;
    records.push(deviceRecord("workspace", workspaceId, { ...templates.get("workspace"), id: workspaceId, project_id: projectId }));
    records.push(deviceRecord("workspace_directory", `${workspaceId}/directory`, { ...templates.get("workspace_directory"),
      id: "directory", workspace_id: workspaceId, project_id: projectId, path: `/synthetic/${workspaceId}` }));
    records.push(deviceRecord("owner_session", owner, { ...templates.get("owner_session"),
      owner_id: owner, workspace_id: workspaceId, project_id: projectId, session_started_ms: EPOCH, last_activity_ms: EPOCH }));
    const roster = [];
    for (let agentIndex = 0; agentIndex < agents; agentIndex += 1) {
      const id = `agent-${index}-${agentIndex}`;
      const agent = { ...templates.get("agent"), id, owner_id: owner, project_id: projectId, workspace_id: workspaceId,
        ordinal: agentIndex + 1, conversation_id: id, thread_id: `thread-${id}`, unread_count: 1 };
      roster.push(agent);
      records.push(deviceRecord("agent", id, agent));
      records.push(deviceRecord("inbox_conversation", id, { ...templates.get("inbox_conversation"),
        owner_id: owner, agent_id: id, thread_id: agent.thread_id, project_id: projectId, workspace_id: workspaceId,
        last_message_summary: "Initial summary", session_started_ms: EPOCH, last_activity_ms: EPOCH }));
    }
    rows.push({ kind: "branch", state: "building", run_id: owner, entity_id: owner, project_id: projectId,
      workspace_id: workspaceId, worktree_id: `worktree-${index}`, branch: "synthetic", agents: roster,
      anchor: new Date(EPOCH).toISOString(), last_activity: new Date(EPOCH).toISOString() });
  }
  records.push(deviceRecord("user_session", "current", templates.get("user_session")));
  return { rows, records };
}

function model(size) {
  const { rows, records } = fixtureRows(size), calls = [], pending = [];
  let now = 0, periodStart = 0, ringActive = false, ringStop = null, revision = 1, expired = false;
  let targetSummary = "Initial summary", landed = null, published = null;
  const opening = () => {
    const session = context.session;
    context.greeting = { epoch: "synthetic-epoch", capabilities: capabilities(), current: () => context?.session === session };
  };
  const response = (extra) => ({ status: "ok", incarnation: "synthetic-incarnation", bridge_epoch: "synthetic-epoch", ...extra });
  const currentRecords = () => records.map((record) => record.kind === "inbox_conversation" && record.key === "agent-0-0"
    ? { ...record, revision: String(revision), row: { ...record.row, last_message_summary: targetSummary,
      last_activity_ms: EPOCH + revision } } : record);
  function answer(method, params) {
    if (method === "sync.snapshot") {
      const start = Number(params.page || 0), all = currentRecords(), page = all.slice(start, start + 100);
      if (page.some((record) => record.key === "agent-0-0" && record.kind === "inbox_conversation")) landed ||= {
        logical: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() };
      return response({ cursor: `cursor-${revision}`, watermark: String(revision), records: page,
        next_page: start + page.length < all.length ? String(start + page.length) : null });
    }
    if (method === "sync.changes") {
      if (expired && params.cursor !== `cursor-${revision}`) {
        expired = false; return response({ status: "resync_required", reason: "retention_expired" });
      }
      const changed = params.cursor !== `cursor-${revision}`;
      if (changed) landed ||= { logical: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() };
      return response({ cursor: `cursor-${revision}`, head: String(revision), changes: changed
        ? currentRecords().filter((record) => record.kind === "inbox_conversation" && record.key === "agent-0-0")
          .map((record) => ({ ...record, seq: String(revision), operation: "upsert", member: true })) : [] });
    }
    if (method === "sync.head") return response({ head: String(revision), retention_floor: "0" });
    const answers = {
      "board.list": () => {
        if (peerVersion === "3.33.0") landed ||= { logical: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() };
        const observed = rows.map((row) => row.run_id === "run-0" ? { ...row, last_message_summary: targetSummary } : row);
        return { items: observed, runs: observed };
      },
      "project.list": () => ({ projects: Array.from({ length: size.projects }, (_, index) => ({ project_id: `project-${index}`, sources: [] })) }),
      "workspace.list": () => ({ workspaces: rows.map((row) => ({ id: row.workspace_id, project_id: row.project_id, entity_id: row.run_id, status: "ready" })) }),
      "agent.list": () => ({ agents: [] }),
      "thread.page": () => ({ thread_id: params.thread_id, thread_last_sequence: 23, items: params.after_sequence ? []
        : Array.from({ length: 23 }, (_, index) => ({ type: "message", data: { id: `message-${index}`, sequence: index + 1,
          role: "user", body: "synthetic ".repeat(100) } })), has_more: false }),
      "thread.activity": () => ({ thread_id: params.thread_id, items: [], has_more: false }),
      "tasks.columns": () => ({ columns: [{ id: "backlog", name: "Backlog" }] }),
      "tasks.list": () => ({ tasks: Array.from({ length: 8 }, (_, index) => ({ id: `${params.project_id}-task-${index}`,
        number: 8 - index, title: "Synthetic task", status: "backlog", project_id: params.project_id })), next_cursor: null }),
      "git.status": () => ({ status_key: "synthetic", files: [] }),
      "fs.tree": () => ({ path: params.path, entries: [] }), "term.list": () => ({ terminals: [] }),
      "git.log": () => ({ commits: [{ hash: "synthetic-commit", ahead_of_base: true }] }),
      "git.unpushed": () => ({ commits: [] }), "git.show": () => ({ hash: params.hash, patch: "synthetic patch" }),
      "run.diff": () => ({ diff_key: "synthetic", files: [], stat: {} }),
      "worktree.diff": () => ({ diff_key: "synthetic", files: [], stat: {} }),
    };
    if (!answers[method]) throw new Error(`Unmodeled verb: ${method}`);
    return answers[method]();
  }
  context = { deviceId: "cutover", session: {}, greeted: Promise.resolve(), active: () => true };
  context.cacheScope = { deviceId: "cutover", active: context.active }; opening();
  context.rpc = (method, params = {}, envelope = {}) => new Promise((resolve) => {
    const duration = COST_MS[method];
    if (duration === undefined) throw new Error(`Unmodeled RPC cost: ${method}`);
    const call = { method, start: now, end: now + duration, priority: envelope.priority || "foreground",
      requestBytes: new TextEncoder().encode(JSON.stringify({ method, params })).length };
    calls.push(call); pending.push({ ...call, resolve: () => {
      const value = answer(method, params); call.responseBytes = new TextEncoder().encode(JSON.stringify(value)).length; resolve(value);
    } });
  });
  sync.subscribeConnectionSync((active) => { if (ringActive && !active) ringStop = now; ringActive = active; });
  return {
    calls,
    async observe() {
      feed.subscribeFeed((view) => {
        metrics.rowPublications += 1;
        if (!published && view.items?.some((row) => row.run_id === "run-0" && row.last_message_summary === targetSummary))
          published = { logical: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() };
      });
      await feed.startFeed();
    },
    reset() { metrics = emptyMetrics(); calls.length = 0; periodStart = now; ringStop = null; landed = null; published = null; },
    reconnect(absent) {
      revision += 1; targetSummary = `Summary revision ${revision}`; expired = absent;
      if (absent) now += 8 * DAY;
      periodStart = now;
      vi.setSystemTime(EPOCH + now); context.session = {}; opening();
      for (const listener of [...listeners]) listener();
      for (const listener of [...greetingListeners]) listener("cutover");
    },
    async finish(label) {
      const started = hrtime.bigint();
      for (let tick = 0; tick < 10000; tick += 1) {
        await cacheTurns(); await cacheTurns();
        if (ringStop !== null && !pending.length && !sync.passInFlight("cutover")) break;
        expect(pending.length, "simulation stalled without an RPC").toBeGreaterThan(0);
        now = Math.min(...pending.map((call) => call.end)); vi.setSystemTime(EPOCH + now);
        for (const call of pending.filter((call) => call.end === now)) {
          pending.splice(pending.indexOf(call), 1); call.resolve();
        }
        if (tick === 9999) throw new Error("simulation exceeded RPC budget");
      }
      await cacheTurns();
      const report = { label, peerVersion, clock: "logical RPC ms; real fake-IDB wall ms, not production disk",
        rpcCount: calls.length, responseBytes: calls.reduce((sum, call) => sum + call.responseBytes, 0),
        requestBytes: calls.reduce((sum, call) => sum + call.requestBytes, 0),
        logicalMs: now - periodStart, ringMs: ringStop - periodStart,
        summedRequestMs: calls.reduce((sum, call) => sum + call.end - call.start, 0),
        cache: { ...metrics, workWallMs: Number(hrtime.bigint() - started) / 1e6 },
        publication: landed && published ? { logicalMs: published.logical - landed.logical,
          idbCallbacks: published.callbacks - landed.callbacks, wallMs: Number(published.wall - landed.wall) / 1e6 } : null,
        verbs: Object.fromEntries([...new Set(calls.map((call) => call.method))].map((method) => [method,
          calls.filter((call) => call.method === method).length])) };
      console.log(JSON.stringify(report));
      if (process.env.CUTOVER_PROFILE_OUTPUT) appendFileSync(process.env.CUTOVER_PROFILE_OUTPUT, `${JSON.stringify(report)}\n`);
      expect(report.rpcCount).toBeGreaterThan(0);
      expect(report.cache.idbCallbacks).toBeGreaterThan(0);
      expect(report.responseBytes).toBeGreaterThan(0);
      expect(report.publication).not.toBeNull();
      if (process.env.CUTOVER_EXPECT_LAZY === "1" && peerVersion === "3.34.0") {
        expect(calls.filter((call) => /^(thread\.|git\.|fs\.|tasks\.|run\.diff|worktree\.diff)/.test(call.method))).toEqual([]);
      }
      return report;
    },
  };
}

const profiles = [
  { label: "small", projects: 2, workspaces: 2, agents: 1 },
  { label: "single-project", projects: 1, workspaces: 24, agents: 3 },
  { label: "large", projects: 8, workspaces: 80, agents: 3 },
];
it.each(["3.33.0", "3.34.0"].flatMap((version) => profiles.map((profile) => ({ ...profile, version }))))(
  "#894 $version $label cold, warm reconnect and expired-retention starts", async ({ label, version, ...size }) => {
    peerVersion = version;
    const fixture = model(size); await fixture.observe(); await cacheTurns(); fixture.reset();
    sync.startCacheSync(); await fixture.finish(`${label}:cold`);
    await cacheTurns(); fixture.reset(); fixture.reconnect(false); await fixture.finish(`${label}:warm`);
    // The absence interval is outside the measured recovery clock.
    await cacheTurns(); fixture.reset(); fixture.reconnect(true); await fixture.finish(`${label}:long-absent`);
  }, 120000,
);
