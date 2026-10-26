// @vitest-environment jsdom
// Investigation #869: simulated network time, real cacheSync/planner/queues/IDB.
// No bridge, transport, server contention, browser idle delay or disk cost is modeled.
import { setImmediate as immediate } from "node:timers";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const App = { route: { name: "inbox" }, devices: [{ id: "scale" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let context, watchers, mark;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => [context],
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
  "git.unpushed": 400, "git.show": 500, "run.diff": 300, "worktree.diff": 300, "agent.list": 100 };
const EPOCH = Date.parse("2026-10-25T12:00:00Z");
let sync, cache, diagnostics, idbTasks;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  // Date and Diagnostics share logical time. IDB uses real event-loop turns;
  // RPCs use the logical clock below, with no timer-based network simulation.
  vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(EPOCH);
  vi.spyOn(performance, "now").mockImplementation(() => Date.now() - EPOCH);
  idbTasks = 0;
  vi.stubGlobal("setImmediate", (callback) => {
    idbTasks += 1;
    return immediate(() => { try { callback(); } finally { idbTasks -= 1; } });
  });
  vi.stubGlobal("requestIdleCallback", (callback) => { queueMicrotask(callback); return 0; });
  watchers = [];
  App.route = { name: "inbox" };
  cache = await import("../src/core/localCache.js");
  // The real greeting records this capability before starting sync.
  await cache.writeCached({ deviceId: "scale", entityId: "", kind: "project-agents-support" }, { projectWorkers: true });
  sync = await import("../src/core/cacheSync.js");
  ({ syncDiagnostics: diagnostics } = await import("../src/core/syncDiagnostics.js"));
});
afterEach(() => { sync.stopCacheSync(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

// fake-indexeddb schedules through global setImmediate. Drain its tracked
// macrotasks plus promise continuations; never estimate a disk delay with sleep.
async function cacheTurns() {
  let quiet = 0;
  for (let turn = 0; turn < 100000; turn += 1) {
    await new Promise(immediate);
    quiet = idbTasks ? 0 : quiet + 1;
    if (quiet === 2) return;
  }
  throw new Error("IDB did not quiesce");
}

function fixture({ workspaces = 40, agents = 5, tasks = 800, patches = 2, projects = 1, ownerAgents = 0, records = 23, latency = {} } = {}) {
  const projectRows = Array.from({ length: projects }, (_, index) => ({ project_id: `project-${index}`, sources: [],
    ...(ownerAgents ? { entity_id: `owner-${index}` } : {}) }));
  const rows = Array.from({ length: workspaces }, (_, index) => ({
    kind: "branch", state: "building", project_id: `project-${index % projects}`, workspace_id: `ws-${index}`,
    run_id: `run-${index}`, worktree_id: `wt-${index}`, anchor: new Date(EPOCH).toISOString(),
    last_activity: new Date(EPOCH).toISOString(),
    agents: Array.from({ length: agents }, (_, agent) => ({ id: `agent-${index}-${agent}`,
      conversation_id: `c-${index}-${agent}`, thread_id: `thread-${index}-${agent}` })),
  }));
  let now = 0, periodStart = 0, stageStart = 0, stage = "startup", ringActive = false, ringStop = null;
  const pending = [], calls = [], stages = [], passEnds = [], ringTransitions = [], observedPasses = new Set();
  mark = (next) => {
    if (next === stage) return;
    stages.push({ stage, start: stageStart, end: now });
    stage = next; stageStart = now;
  };
  const answer = (method, params) => {
    const answers = {
      "board.list": () => ({ items: rows }),
      "project.list": () => ({ projects: projectRows }),
      "workspace.list": () => ({ workspaces: rows.map((row) => ({ id: row.workspace_id,
        project_id: row.project_id, entity_id: row.run_id, status: "ready", managed: true })) }),
      "agent.list": () => ({ entity_id: params.entity_id, pinned_agent_id: `${params.entity_id}-agent-0`,
        agents: Array.from({ length: ownerAgents }, (_, index) => ({ id: `${params.entity_id}-agent-${index}`,
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
    expect(ringActive, `${method} must run inside the connection ring`).toBe(true);
    const duration = typeof latency[method] === "function" ? latency[method](params) : latency[method] ?? BASE_LATENCY[method];
    const call = { method, stage, start: now, end: now + duration,
      priority: envelope.priority || "foreground" };
    calls.push(call);
    pending.push({ ...call, resolve: () => {
      const value = answer(method, params); call.bytes = new TextEncoder().encode(JSON.stringify(value)).length;
      resolve(value);
    } });
  });
  sync.subscribeConnectionSync((active) => {
    if (ringActive !== active) ringTransitions.push(active);
    if (ringActive && !active) ringStop = now;
    ringActive = active;
  });
  return {
    calls, rows,
    nextPeriod() {
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
    async finish(label) {
      await this.drive(() => ringStop !== null);
      mark("finished");
      expect(ringTransitions, "the ring spans all conversations and chained passes").toEqual([true, false]);
      const period = diagnostics.report().periods.at(-1);
      expect(period.stop).toBe(now - periodStart);
      for (const call of calls.filter(({ method }) => method === "thread.page")) {
        const wanted = call.priority === "foreground" ? "foreground" : "inbox";
        const containing = period.passes.flatMap((pass) => pass.stages).filter(({ stage, start, end }) =>
          stage === wanted && start <= call.start - periodStart && end >= call.end - periodStart);
        expect(containing, `${call.priority} thread RPC belongs to its Diagnostics stage`).toHaveLength(1);
      }
      const report = { label, ringMs: ringStop - periodStart,
        passEndsMs: passEnds.map((time) => time - periodStart), snapshotDrainMs: ringStop - passEnds.at(-1),
        threadResponseBytes: calls.filter((call) => call.method === "thread.page").reduce((sum, call) => sum + call.bytes, 0),
        stages: stages.map(({ stage, start, end }) => ({ stage, ms: end - start })),
        verbs: Object.fromEntries([...new Set(calls.map((call) => call.method))].map((method) => [method,
          calls.filter((call) => call.method === method).length])) };
      console.log(JSON.stringify(report));
      return report;
    },
    get now() { return now; },
    get stopped() { return ringStop; },
  };
}

it("reports empty-cache, fresh-session cached load, and same-session restored pass stages", async () => {
  const model = fixture();
  sync.startCacheSync();
  const cold = await model.finish("cold: 40 workspaces, 200 conversations, 800 tasks, 2 patches each");
  expect(cold.verbs["thread.page"]).toBe(200);
  expect(cold.verbs["tasks.list"]).toBe(8);
  expect(cold.verbs["git.show"]).toBe(80);
  expect(cold.ringMs).toBe(108900);
  sync.stopCacheSync(); context.session = {}; model.nextPeriod(); sync.startCacheSync();
  const cached = await model.finish("hard refresh equivalent: new in-memory sync lifetime/session, retained IDB");
  expect(cached.ringMs).toBe(82900);
  expect(cached.verbs["thread.page"]).toBe(200);
  expect(cached.verbs["git.show"]).toBeUndefined();
  expect(cached.threadResponseBytes).toBeLessThan(cold.threadResponseBytes / 10);
  model.nextPeriod();
  const restored = sync.syncRestoredDevice("scale", context.session);
  const restoredReport = await model.finish("same-session syncRestoredDevice with completed previous pass");
  await restored;
  expect(restoredReport.ringMs).toBe(82900);
});

it("grounds a 400-conversation first walk in many selected project and workspace rosters", async () => {
  const model = fixture({ projects: 20, ownerAgents: 10, tasks: 40 });
  sync.startCacheSync();
  const result = await model.finish("20 project owners x10 +40 active workspace rows x5 =400 conversations; 800 total tasks");
  expect(result.verbs["agent.list"]).toBe(20);
  expect(result.verbs["thread.page"]).toBe(400);
  expect(result.verbs["tasks.list"]).toBe(20);
  expect(result.ringMs).toBe(165100);
});

it("keeps selected conversation and checkout data ahead of the long inbox walk", async () => {
  const model = fixture();
  App.route = { name: "workspace", deviceId: "scale", projectId: "project-0", workspaceId: "ws-0", agent: "agent-0-0" };
  sync.startCacheSync();
  const report = await model.finish("foreground route before 199 other conversations");
  const firstThread = model.calls.find((call) => call.method === "thread.page");
  const inbox = report.stages.find((stage) => stage.stage === "inbox threads");
  const foreground = report.stages.find((stage) => stage.stage === "foreground");
  expect(firstThread.priority).toBe("foreground");
  expect(firstThread.end).toBe(600);
  expect(model.calls.find((call) => call.method === "git.status").priority).toBe("foreground");
  expect(foreground.ms).toBe(2650);
  expect(inbox.ms).toBe(50000);
  console.log(JSON.stringify({ selectedThreadAnswerMs: firstThread.end,
    foregroundDataMs: foreground.ms, otherThreadWalkMs: inbox.ms,
    note: "cache availability boundaries; DOM paint time is not modeled" }));
});

it("reports a chained pass without restarting the ring", async () => {
  const model = fixture();
  sync.startCacheSync();
  await model.drive(() => model.calls.length >= 3);
  const chained = sync.syncDevice("scale");
  const result = await model.finish("two chained passes; second reuses cached patches/diff/columns");
  await chained;
  expect(result.verbs["board.list"]).toBe(2);
  expect(result.verbs["thread.page"]).toBe(400);
  expect(result.verbs["git.show"]).toBe(80);
  expect(result.ringMs).toBe(191800);
});

it("separates a short pass from one captured slow push RPC", async () => {
  const model = fixture({ workspaces: 1, agents: 1, tasks: 1, patches: 0,
    latency: { "tasks.list": (params) => params.project_id === "pushed-project" ? 120000 : 350 } });
  sync.startCacheSync();
  await model.drive(() => model.calls.some((call) => call.method === "thread.page"));
  // Separate project prevents pass task serialization from waiting on this push.
  const pushed = model.push([{ entity_id: "pushed-project", tasks: { task_ids: ["updated"] } }]);
  await model.drive(() => !sync.passInFlight("scale"));
  const passEnd = model.now;
  expect(model.stopped).toBe(null);
  const result = await model.finish("one 120s push task read held in final snapshot");
  await pushed;
  console.log(JSON.stringify({ passEndMs: passEnd, snapshotDrainMs: result.ringMs - passEnd }));
  expect(passEnd).toBeLessThan(5000);
  expect(result.ringMs - passEnd).toBeGreaterThan(115000);
});


// Serial baselines were captured with the same fixtures and RPC costs.
it.each([
  { label: "98 conversations / thread windows, 2254 message records (synthetic)", workspaces: 14, agents: 7, records: 23, serialMs: 59950, parallelMs: 35450 },
  { label: "1 conversation, 23 message records", workspaces: 1, agents: 1, records: 23, serialMs: 2450, parallelMs: 2450 },
  { label: "6 conversations, 138 message records", workspaces: 2, agents: 3, records: 23, serialMs: 4950, parallelMs: 3450 },
  { label: "2 conversations, 98 message records (synthetic, not prod equivalent)", workspaces: 2, agents: 1, records: 49, serialMs: 2950, parallelMs: 2450 },
])("reports a small warm profile: $label", async ({ label, serialMs, parallelMs, ...size }) => {
  const model = fixture({ ...size, tasks: 8, patches: 0 });
  sync.startCacheSync();
  await model.finish(`small cold: ${label}`);
  sync.stopCacheSync(); context.session = {}; model.nextPeriod(); sync.startCacheSync();
  const warm = await model.finish(`small warm: ${label}`);
  expect(warm.verbs["thread.page"]).toBe(size.workspaces * size.agents);
  expect(warm.threadResponseBytes).toBeLessThan(size.workspaces * size.agents * 100);
  expect(warm.ringMs).toBeLessThanOrEqual(serialMs);
  expect(warm.ringMs).toBe(parallelMs);
});


it("keeps the ring and Inbox Diagnostics open until the slower concurrent walker completes", async () => {
  const model = fixture({ workspaces: 1, agents: 2, tasks: 1, patches: 0,
    latency: { "thread.page": (params) => params.thread_id === "thread-0-0" ? 200 : 900 } });
  sync.startCacheSync();
  await model.drive(() => model.now === 300);
  expect(model.calls.filter(({ method }) => method === "thread.page")).toHaveLength(2);
  expect(model.stopped).toBeNull();
  const stages = diagnostics.report().periods[0].passes[0].stages;
  expect(stages).toContainEqual(expect.objectContaining({ stage: "inbox", end: null }));
  expect(stages.some(({ stage }) => stage === "background")).toBe(false);
  expect(model.calls.some(({ method }) => method === "git.status")).toBe(false);
  await model.finish("uneven workers: 200ms and 900ms thread replies");
});
