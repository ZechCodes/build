// @vitest-environment jsdom
// #879: report-shaped logical RPC time and real cache/planner/feed operations.
// No bridge, transport, server contention, browser idle delay or disk cost is modeled.
import { appendFileSync } from "node:fs";
import { hrtime } from "node:process";
import { setImmediate as immediate } from "node:timers";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBCursor, IDBFactory, IDBIndex, IDBKeyRange } from "fake-indexeddb";

const App = { route: { name: "inbox" }, devices: [{ id: "scale" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let context, watchers, mark;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, homeContext: () => context, liveContexts: () => [context],
  canAnswer: () => true, creationDevice: () => "scale",
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ agents: { projectWorkers: true }, tasks: { listPaged: true }, threads: { leanActivity: true },
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));
vi.mock("../src/core/syncLists.js", async (original) => {
  const actual = await original();
  return { ...actual, readSyncLists: async (...args) => { mark("lists"); return actual.readSyncLists(...args); } };
});
vi.mock("../src/core/syncPhases.js", async (original) => {
  const actual = await original();
  return { ...actual, createSyncPhases(options) {
    const readers = Object.fromEntries(Object.entries(options.readers).map(([kind, read]) => [kind,
      ["scope", "terminalScope"].includes(kind) ? read : (...args) => {
        const priority = args[kind === "tree" ? 4 : 3];
        mark(priority === "foreground" ? "foreground" : kind === "thread" ? "inbox threads" : "checkout");
        return read(...args);
      }]));
    const tasks = { ...options.tasks, rest: (...args) => { mark("tasks"); return options.tasks.rest(...args); } };
    const projectRosterReads = options.projectRosterReads.map((job) => ({ ...job,
      read: () => { mark("project rosters"); return job.read(); } }));
    const phases = actual.createSyncPhases({ ...options, readers, tasks, projectRosterReads });
    return { ...phases, run: async () => { const result = await phases.run(); mark("cleanup"); return result; } };
  } };
});

const BASE_LATENCY = { "board.list": 100, "project.list": 100, "workspace.list": 100,
  "thread.page": 500, "thread.activity": 500, "tasks.list": 350, "tasks.columns": 100,
  "git.status": 600, "fs.tree": 200, "term.list": 100, "git.log": 200,
  "entity.seen": 100, "git.unpushed": 400, "git.show": 500, "run.diff": 300, "worktree.diff": 300, "agent.list": 100 };
const EPOCH = Date.parse("2026-10-25T12:00:00Z");
let sync, cache, diagnostics, idbTasks, feed;
let metrics;
beforeEach(async () => {
  vi.resetModules();
vi.doMock("../src/core/projectAgentsCache.js", async (original) => {
  const actual = await original();
  return { ...actual, projectsWithAgentRosters: (...args) => {
    metrics.accountRosterSelections += 1;
    return actual.projectsWithAgentRosters(...args);
  } };
});
vi.doMock("../src/core/cacheLifetime.js", async (original) => {
  const actual = await original();
  return { ...actual, expireWorkspacesData: (...args) => {
    metrics.recentExpiries += args[1].length;
    metrics.expiryBatches += Number(args[1].length > 0);
    const started = hrtime.bigint(), callbacks = metrics.idbCallbacks, cursors = metrics.cursorContinuations;
    return actual.expireWorkspacesData(...args).finally(() => {
      metrics.expiryWallMs += Number(hrtime.bigint() - started) / 1e6;
      metrics.expiryIdbCallbacks += metrics.idbCallbacks - callbacks;
      metrics.expiryCursorContinuations += metrics.cursorContinuations - cursors;
    });
  } };
});
vi.doMock("../src/core/readStateRepair.js", async (original) => {
  const actual = await original();
  const wrap = (name) => (...args) => {
    metrics.repairRequests += 1;
    const started = hrtime.bigint(), callbacks = metrics.idbCallbacks;
    const repairContext = { ...args[0], call: (method, params, envelope = {}) =>
      args[0].call(method, params, { ...envelope, profileRepair: true }) };
    return actual[name](repairContext, ...args.slice(1)).finally(() => {
      metrics.repairWindowWallMs += Number(hrtime.bigint() - started) / 1e6;
      metrics.repairWindowIdbCallbacks += metrics.idbCallbacks - callbacks;
    });
  };
  return { ...actual, refreshReadState: wrap("refreshReadState"),
    repairReadFeed: wrap("repairReadFeed"), repairReadOwner: wrap("repairReadOwner") };
});
  metrics = emptyMetrics();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  // Date and Diagnostics share logical time. IDB uses real event-loop turns;
  // RPCs use the logical clock below, with no timer-based network simulation.
  vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(EPOCH);
  vi.spyOn(performance, "now").mockImplementation(() => Date.now() - EPOCH);
  idbTasks = 0;
  vi.stubGlobal("setImmediate", (callback) => {
    idbTasks += 1; metrics.idbCallbacks += 1;
    return immediate(() => { try { callback(); } finally { idbTasks -= 1; } });
  });
  vi.stubGlobal("requestIdleCallback", (callback) => { queueMicrotask(callback); return 0; });
  const continueCursor = IDBCursor.prototype.continue;
  vi.spyOn(IDBCursor.prototype, "continue").mockImplementation(function (...args) {
    metrics.cursorContinuations += 1;
    return continueCursor.apply(this, args);
  });
  const openKeyCursor = IDBIndex.prototype.openKeyCursor;
  vi.spyOn(IDBIndex.prototype, "openKeyCursor").mockImplementation(function (...args) {
    metrics.indexKeyCursors += 1;
    return openKeyCursor.apply(this, args);
  });
  watchers = [];
  App.route = { name: "inbox" };
  cache = await import("../src/core/localCache.js");
  // The real greeting records this capability before starting sync.
  await cache.writeCached({ deviceId: "scale", entityId: "", kind: "project-agents-support" }, { projectWorkers: true });
  sync = await import("../src/core/cacheSync.js");
  ({ syncDiagnostics: diagnostics } = await import("../src/core/syncDiagnostics.js"));
  const stageStart = diagnostics.stageStart;
  vi.spyOn(diagnostics, "stageStart").mockImplementation((pass, stage) => {
    const finish = stageStart(pass, stage), started = hrtime.bigint();
    return () => {
      const wallMs = Number(hrtime.bigint() - started) / 1e6;
      metrics.stageWallMs[stage] = (metrics.stageWallMs[stage] || 0) + wallMs;
      finish();
    };
  });
});
afterEach(() => { feed?.stopFeed(); feed = null; sync?.stopCacheSync(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

function emptyMetrics() {
  return { accountRosterSelections: 0, recentExpiries: 0, idbCallbacks: 0, cacheDrainWallMs: 0,
    expiryWallMs: 0, expiryBatches: 0, expiryIdbCallbacks: 0, expiryCursorContinuations: 0,
    cursorContinuations: 0, indexKeyCursors: 0, stageWallMs: {},
    repairRequests: 0, repairWindowWallMs: 0, repairWindowIdbCallbacks: 0 };
}

// Seed unrelated stale metadata in one transaction. Use the real replica shape,
// with no production helper overhead in the measured period.
async function seedUnrelatedStaleKeys(count) {
  await new Promise((resolve, reject) => {
    const request = indexedDB.open("build-cache");
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result, transaction = db.transaction("records", "readwrite");
      transaction.onerror = () => reject(transaction.error);
      transaction.oncomplete = () => { db.close(); resolve(); };
      const store = transaction.objectStore("records");
      for (let index = 0; index < count; index += 1) {
        store.put({ at: EPOCH - 4 * 24 * 60 * 60 * 1000, value: {} }, `unrelated|old-${String(index).padStart(5, "0")}|status|`);
      }
    };
  });
}

// fake-indexeddb schedules through global setImmediate. Drain its tracked
// macrotasks plus promise continuations; never estimate a disk delay with sleep.
async function cacheTurns() {
  const started = hrtime.bigint();
  let quiet = 0;
  for (let turn = 0; turn < 100000; turn += 1) {
    await new Promise(immediate);
    quiet = idbTasks ? 0 : quiet + 1;
    if (quiet === 2) { metrics.cacheDrainWallMs += Number(hrtime.bigint() - started) / 1e6; return; }
  }
  throw new Error("IDB did not quiesce");
}

function fixture({ workspaces = 40, agents = 5, tasks = 800, patches = 2, projects = 1, ownerAgents = 0, records = 23, recent = false, latency = {} } = {}) {
  const projectRows = Array.from({ length: projects }, (_, index) => ({ project_id: `project-${index}`, sources: [],
    ...(ownerAgents ? { entity_id: `owner-${index}` } : {}) }));
  const rows = Array.from({ length: workspaces }, (_, index) => ({
    kind: "branch", state: "building", dismissed: recent && index === workspaces - 1, project_id: `project-${index % projects}`, workspace_id: `ws-${index}`,
    run_id: `run-${index}`, worktree_id: `wt-${index}`, anchor: new Date(EPOCH).toISOString(),
    last_activity: new Date(EPOCH).toISOString(),
    agents: Array.from({ length: agents }, (_, agent) => ({ id: `agent-${index}-${agent}`,
      conversation_id: `c-${index}-${agent}`, thread_id: `thread-${index}-${agent}` })),
  }));
  let newWorker = false;
  let now = 0, periodStart = 0, stageStart = 0, stage = "startup", ringActive = false, ringStop = null;
  const pending = [], calls = [], rosterLandings = new Map(), rowPublications = new Map(), stages = [], passEnds = [], ringTransitions = [], observedPasses = new Set();
  mark = (next) => {
    if (next === stage) return;
    stages.push({ stage, start: stageStart, end: now });
    stage = next; stageStart = now;
  };
  const answer = (method, params) => {
    const answers = {
      "entity.seen": () => {
        const row = rows.find((row) => row.run_id === params.entity_id);
        if (row) row.agents = row.agents.map((agent) => ({ ...agent, unread_count: 0, read_through_sequence: params.read_through_sequence }));
        return {};
      },
      "board.list": () => ({ items: rows }),
      "project.list": () => ({ projects: projectRows }),
      "workspace.list": () => ({ workspaces: rows.map((row) => ({ id: row.workspace_id,
        project_id: row.project_id, entity_id: row.run_id, status: "ready", managed: true })) }),
      "agent.list": () => ({ entity_id: params.entity_id, pinned_agent_id: `${params.entity_id}-agent-0`,
        agents: Array.from({ length: ownerAgents + (newWorker && params.entity_id === "owner-0" ? 1 : 0) }, (_, index) => ({ id: `${params.entity_id}-agent-${index}`,
          conversation_id: `${params.entity_id}-c-${index}`, thread_id: `${params.entity_id}-t-${index}` })) }),
      "thread.page": () => ({ thread_id: params.thread_id, thread_last_sequence: records,
        items: params.after_sequence ? [] : Array.from({ length: records }, (_, index) => ({ type: "message",
          data: { id: `message-${index}`, sequence: index + 1, role: "user", body: "x".repeat(1024) } })), has_more: false }),
      "tasks.columns": () => ({ columns: [{ id: "backlog", name: "Backlog" }] }),
      "tasks.list": () => {
        const offset = Number(params.cursor || 0), count = Math.min(params.limit || tasks, tasks - offset);
        return { tasks: Array.from({ length: count }, (_, index) => ({ id: `${params.project_id}-task-${offset + index}`,
          number: tasks - offset - index, title: "Synthetic", status: "backlog", project_id: params.project_id })),
        next_cursor: offset + count < tasks ? String(offset + count) : null };
      },
      "git.status": () => ({ status_key: "initial", files: [] }),
      "fs.tree": () => ({ path: params.path, entries: [] }), "term.list": () => ({ terminals: [] }),
      "git.log": () => ({ commits: Array.from({ length: patches }, (_, index) => ({ hash: `commit-${index}`, ahead_of_base: true })) }),
      "git.unpushed": () => ({ commits: [] }), "git.show": () => ({ hash: params.hash, patch: "synthetic" }),
      "run.diff": () => ({ diff_key: "diff", files: [], stat: {} }),
    };
    if (!answers[method]) throw new Error(`Unmodeled verb: ${method}`);
    return answers[method]();
  };
  context = { deviceId: "scale", session: {}, greeted: Promise.resolve(), active: () => true };
  context.cacheScope = { deviceId: "scale", active: context.active };
  context.rpc = (method, params = {}, envelope = {}) => new Promise((resolve) => {
    const repair = envelope.profileRepair === true || method === "entity.seen"
      || (method === "board.list" && calls.some((call) => call.method === "board.list"));
    if (!repair) expect(ringActive, `${method} must run inside the connection ring`).toBe(true);
    const duration = repair && method === "board.list" && latency.repairBoard !== undefined ? latency.repairBoard : typeof latency[method] === "function" ? latency[method](params) : latency[method] ?? BASE_LATENCY[method];
    const call = { method, repair, ringAtDispatch: ringActive, stage, start: now, end: now + duration,
      priority: envelope.priority || "foreground" };
    calls.push(call);
    pending.push({ ...call, resolve: () => {
      const value = answer(method, params); call.bytes = new TextEncoder().encode(JSON.stringify(value)).length;
      if (method === "agent.list" && !rosterLandings.has(params.entity_id))
        rosterLandings.set(params.entity_id, { at: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() });
      resolve(value);
    } });
  });
  sync.subscribeConnectionSync((active) => {
    if (ringActive !== active) ringTransitions.push(active);
    if (ringActive && !active) ringStop = now;
    ringActive = active;
  });
  return {
    calls, rows, rosterLandings, rowPublications,
    addWorker() { newWorker = true; },
    async observeRows() {
      feed = await import("../src/core/taskFeed.js");
      const { projectRosterRows } = await import("../src/core/projectRosterFeed.js");
      feed.subscribeFeed((snapshot) => {
        // Same cache projection consumed by the inbox row renderer. No DOM paint.
        for (const row of projectRosterRows(snapshot)) {
          if (row.agents?.some((agent) => agent.id.endsWith("-agent-1")) && !rowPublications.has(row.entity_id))
            rowPublications.set(row.entity_id, { at: now, callbacks: metrics.idbCallbacks, wall: hrtime.bigint() });
        }
      });
      await feed.startFeed();
    },
    nextPeriod() {
      metrics = emptyMetrics();
      rosterLandings.clear(); rowPublications.clear();
      periodStart = now; stageStart = now; stage = "startup"; ringStop = null;
      calls.length = 0; stages.length = 0; passEnds.length = 0; ringTransitions.length = 0;
    },
    push: (items) => watchers.find((entry) => entry.kinds.includes("thread")).onChanges(items),
    async drive(until) {
      for (let tick = 0; tick < 10000; tick += 1) {
        const pass = sync.passInFlight("scale");
        if (pass && !observedPasses.has(pass)) {
          observedPasses.add(pass);
          void pass.then(() => { passEnds.push(now); if (!sync.passInFlight("scale")) mark("snapshot drain"); });
        }
        await cacheTurns();
        const before = pending.length;
        await cacheTurns();
        expect(pending.length, "IDB must be quiescent before advancing network time").toBe(before);
        if (until()) return;
        expect(pending.length, "simulation stalled with no pending RPC").toBeGreaterThan(0);
        now = Math.min(...pending.map((call) => call.end)); vi.setSystemTime(EPOCH + now);
        const ready = pending.filter((call) => call.end === now);
        for (const call of ready) { pending.splice(pending.indexOf(call), 1); call.resolve(); }
      }
      throw new Error("simulation exceeded RPC budget");
    },
    async finish(label, completion = Promise.resolve()) {
      let complete = false;
      void completion.then(() => { complete = true; });
      await this.drive(() => ringStop !== null);
      const ringCallbacks = metrics.idbCallbacks;
      const ringWallMs = { ...metrics.stageWallMs };
      await this.drive(() => complete && pending.length === 0);
      mark("finished");
      expect(ringTransitions, "the ring spans all conversations and chained passes").toEqual([true, false]);
      const period = diagnostics.report().periods.at(-1);
      expect(period.stop).toBe(ringStop - periodStart);
      for (const call of calls.filter(({ method }) => method === "thread.page")) {
        const wanted = call.priority === "foreground" ? "foreground" : "inbox";
        const containing = period.passes.flatMap((pass) => pass.stages).filter(({ stage, start, end }) =>
          stage === wanted && start <= call.start - periodStart && end >= call.end - periodStart);
        expect(containing, `${call.priority} thread RPC belongs to its Diagnostics stage`).toHaveLength(1);
      }
      const report = { label, clock: "logical RPC ms; real fake-IDB event-loop wall ms (not production disk)",
        repair: { requests: metrics.repairRequests, rpcCount: calls.filter((call) => call.repair).length,
          rpcAfterRing: calls.filter((call) => call.repair && !call.ringAtDispatch).length,
          completionAfterRingMs: now - ringStop, idbCallbacksAfterRing: metrics.idbCallbacks - ringCallbacks,
          windowIdbCallbacks: metrics.repairWindowIdbCallbacks,
          windowWallMs: metrics.repairWindowWallMs,
          note: "repair window counters include concurrent pass work; after-ring callbacks are disjoint" },
        ringCache: { idbCallbacks: ringCallbacks, stageWallMs: ringWallMs },
        rpc: { criticalPathMs: ringStop - periodStart, summedRequestMs: calls.reduce((sum, call) => sum + call.end - call.start, 0) },
        cache: { ...metrics, selectionWallMs: metrics.stageWallMs.selection || 0,
          selectionOtherWallMs: (metrics.stageWallMs.selection || 0) - metrics.expiryWallMs },
        rosterRowPublication: [...rosterLandings].filter(([owner]) => owner === "owner-0").map(([owner, landed]) => {
          const row = rowPublications.get(owner);
          return { owner, logicalMs: row ? row.at - landed.at : null,
            idbCallbacks: row ? row.callbacks - landed.callbacks : null,
            wallMs: row ? Number(row.wall - landed.wall) / 1e6 : null };
        }), ringMs: ringStop - periodStart,
        passEndsMs: passEnds.map((time) => time - periodStart), snapshotDrainMs: ringStop - passEnds.at(-1),
        threadResponseBytes: calls.filter((call) => call.method === "thread.page").reduce((sum, call) => sum + call.bytes, 0),
        stages: stages.map(({ stage, start, end }) => ({ stage, ms: end - start })),
        verbs: Object.fromEntries([...new Set(calls.map((call) => call.method))].map((method) => [method,
          calls.filter((call) => call.method === method).length])) };
      console.log(JSON.stringify(report));
      if (process.env.SYNC_PROFILE_OUTPUT) appendFileSync(process.env.SYNC_PROFILE_OUTPUT, `${JSON.stringify(report)}\n`);
      return report;
    },
    get now() { return now; },
    get stopped() { return ringStop; },
  };
}


// Report-shaped fixture, not a replay: report lacks per-owner/thread identities,
// roster RPC duration and cache operation durations. Task RPC range is observed;
// other RPC costs use the scale harness constants and agent.list=100ms assumption.
const profiles = [
  ...[0, 1000, 10000].map((staleKeys) => ({ label: `report-shape-stale-${staleKeys}`, staleKeys,
    projects: 8, workspaces: 7, agents: 1, ownerAgents: 1, recent: true,
    latency: { "tasks.list": (params) => 502 + Number(params.project_id.split("-").at(-1)) * (1936 - 502) / 7 } })),
  { label: "small-warm", projects: 2, workspaces: 2, agents: 1, ownerAgents: 1 },
  { label: "single-project", projects: 1, workspaces: 1, agents: 1, ownerAgents: 1 },
  { label: "single-project-markSeen", reporter: true, projects: 1, workspaces: 1, agents: 1, ownerAgents: 1 },
  { label: "single-project-delayed-repair", delayed: true, projects: 1, workspaces: 1, agents: 1, ownerAgents: 1,
    latency: { repairBoard: 6000 } },
];
it.each(profiles)("#881 profile $label separates logical RPC and actual cache work", async ({ label, staleKeys = 0, reporter = false, delayed = false, ...size }) => {
  const model = fixture({ ...size, tasks: 8, patches: 0 });
  if (size.recent) await cache.writeCached({ deviceId: "scale", entityId: "run-6", kind: "thread", sub: "c-6-0" },
    { thread_id: "thread-6-0", items: [] });
  // Start from the report's 8 owner +6 active +1 Recent conversations,
  // then introduce one new worker at owner-0 in the measured pass.
  sync.startCacheSync(); await model.finish(`${label}: seed`);
  sync.stopCacheSync(); context.session = {};
  await seedUnrelatedStaleKeys(staleKeys);
  model.addWorker();
  await model.observeRows();
  // The compatibility refresh requires a pre-existing Detail row. The scale
  // fixture's List digests alone do not retain this row after ownership cleanup.
  if (reporter) await cache.writeCached({ deviceId: "scale", entityId: "run-0", kind: "row", sub: "" }, model.rows[0]);
  const markSeen = reporter ? (await import("../src/core/inboxSeen.js")).markSeen : null;
  await cacheTurns(); model.nextPeriod();
  sync.startCacheSync();
  await model.drive(() => model.calls.length >= 3);
  // Explicit read-state requests repair badges outside ordered sync. Actual
  // markSeen callers already schedule their feed repair without another pass.
  const successor = reporter ? (async () => {
    expect(await markSeen("run-0", "agent-0-0", null, 23, "thread-0-0", "scale")).toBe(true);
    const row = await cache.readCached({ deviceId: "scale", entityId: "run-0", kind: "row", sub: "" });
    expect(row.value.agents[0].read_through_sequence).toBe(23);
  })() : sync.syncDevice("scale", "read-state");
  const report = await model.finish(`${label}: measured warm with ${reporter ? "markSeen" : "read-state"} after list dispatch`, successor);
  await successor;
  expect(report.passEndsMs, "read marks never start a successor pass").toHaveLength(1);
  expect(report.ringMs).toBe(size.projects === 8 ? 24152 : size.projects === 2 ? 4500 : 3050);
  expect(Object.values(report.verbs).reduce((sum, count) => sum + count, 0)).toBe(reporter ? 16 : size.projects === 8 ? 79 : size.projects === 2 ? 25 : 15);
  expect(report.verbs["agent.list"], "roster reads dispatched after the read-state ack already cover its owner marks").toBe(size.projects);
  expect(report.verbs["tasks.list"]).toBe(size.projects);
  expect(report.verbs["thread.page"]).toBe(size.workspaces * size.agents + size.projects * size.ownerAgents + 1);
  expect(report.rosterRowPublication).toHaveLength(1);
  expect(report.rosterRowPublication.every((row) => row.logicalMs === 0)).toBe(true);
  // Baseline scheduler counts through taskFeed row projection. Keep new-worker
  // publication prompt even when the pass's selection strategy changes.
  expect(report.rosterRowPublication[0].idbCallbacks).toBeLessThanOrEqual(reporter ? 25 : size.projects === 1 ? 15 : 33);
  // Account selection happens once; targeted repairs never select the account.
  expect(report.cache.accountRosterSelections).toBe(1);
  expect(report.verbs["entity.seen"] || 0).toBe(reporter ? 1 : 0);
  expect(report.verbs["board.list"]).toBe(2);
  expect(report.verbs["project.list"]).toBe(1);
  expect(report.verbs["workspace.list"]).toBe(1);
  expect(report.cache.recentExpiries).toBe(size.recent ? 1 : 0);
  expect(report.cache.expiryBatches).toBe(size.recent ? 1 : 0);
  expect(report.cache.idbCallbacks).toBeGreaterThan(0);
  expect(report.cache.cacheDrainWallMs).toBeGreaterThan(0);
  expect(report.cache.selectionWallMs).toBeGreaterThan(0);
  expect(report.cache.selectionOtherWallMs).toBeGreaterThanOrEqual(0);
  if (size.recent) {
    expect(report.cache.expiryWallMs).toBeGreaterThan(0);
    expect(report.cache.expiryCursorContinuations).toBe(0);
    expect(report.cache.indexKeyCursors).toBe(0);
  }
  if (delayed) {
    expect(report.repair.completionAfterRingMs).toBeGreaterThan(0);
    expect(report.repair.idbCallbacksAfterRing).toBeGreaterThan(0);
  }
}, 120000);
