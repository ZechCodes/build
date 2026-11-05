// @vitest-environment jsdom
// Matched e1/final selected-task mount probe. Real task page, registry, scoped
// runtime/engine and IndexedDB task writers; mocked service/transport/greeting.
// Device bootstrap is completed before the measured interval. Push subscription
// wire traffic, real SQL/disk, read marks, review metadata and user actions are
// excluded. Every callRpc issued during mount, including real task scope RPCs,
// is counted. This is a selected-task opener, not whole-app startup cost.
import { appendFileSync } from "node:fs";
import { hrtime } from "node:process";
import { setImmediate as immediate } from "node:timers";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, task } from "./trackerWireFixture.js";

const DEVICE = "synthetic-task-open", PROJECT = "synthetic-project", TASK = "synthetic-task";
const EPOCH = "synthetic-opening", INCARNATION = "synthetic-incarnation";
let context, cache, tracker, runtime, page, host, stopCache, domObserver;
let idbTasks, idbCallbacks, measured, calls, publications, faults, started, firstPaint, firstPublication;
let releaseReplies, repliesAllowed;
const capabilities = () => ({ sync: { device: true, projectTasks: true, task: true },
  tasks: { timelinePaged: true }, changes: { subscriptions: true, kinds: ["tasks"] } });
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, homeContext: () => context, liveContexts: () => context ? [context] : [],
  canAnswer: () => true, creationDevice: () => DEVICE,
  captureDeviceGreeting: () => context?.greeting, deviceContextIdentity: () => context,
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => context?.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeApiVersion: () => "3.35.0", bridgeCapabilities: () => capabilities(),
  onBridgeGreeted: () => () => {}, watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => faults.push(args) }));

const elapsed = () => Number(hrtime.bigint() - started) / 1e6;
const bytes = (value) => Buffer.byteLength(JSON.stringify(value));
async function drainCache() {
  let quiet = 0;
  for (let turn = 0; turn < 100000; turn += 1) {
    await new Promise(immediate);
    quiet = idbTasks ? 0 : quiet + 1;
    if (quiet >= 3) return;
  }
  throw new Error("task-open IndexedDB did not quiesce");
}

beforeEach(async () => {
  vi.resetModules(); measured = false; idbTasks = 0; idbCallbacks = 0;
  calls = []; publications = 0; faults = []; firstPaint = null; firstPublication = null;
  page = null; stopCache = null; domObserver = null;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: vi.fn() } });
  vi.stubGlobal("setImmediate", (callback) => {
    idbTasks += 1; if (measured) idbCallbacks += 1;
    return immediate(() => { try { callback(); } finally { idbTasks -= 1; } });
  });
  host = document.createElement("div"); host.scrollTo = vi.fn(); document.body.append(host);
  context = { deviceId: DEVICE, session: {}, active: () => true, cacheScope: { deviceId: DEVICE },
    greeted: Promise.resolve(), greeting: { epoch: EPOCH, current: () => true, capabilities: capabilities() } };
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  runtime = await import("../src/core/scopedSyncRuntime.js");
  await tracker.writeTasksRecord(DEVICE, PROJECT, { tasks: [], columns: columns() });
});
afterEach(async () => {
  releaseReplies?.(); page?.dispose(); runtime?.stopScopedSyncRuntime();
  stopCache?.(); domObserver?.disconnect(); await drainCache();
  context = null; host.remove(); delete navigator.locks;
  vi.unstubAllGlobals(); vi.restoreAllMocks();
});

it.each([20, 4096].flatMap((historyLength) => ["cold", "warm"].map((start) => ({ historyLength, start }))))(
  "measures every selected task mount RPC ($historyLength comments, $start cache)", async ({ historyLength, start }) => {
    const canonical = task({ id: TASK, project_id: PROJECT, number: 1,
      title: `Synthetic task with ${historyLength} comments`, body: "Synthetic selected task body.",
      watched: false, unread_count: 0, unread_agent_comments: 0, unread_mentions: 0 });
    const timeline = Array.from({ length: historyLength }, (_, index) => comment({
      id: `tc-synthetic-${String(index).padStart(6, "0")}`, task_id: TASK,
      body: `Synthetic historical comment ${index}: ${"x".repeat(48)}`,
      created_at: new Date(Date.UTC(2026, 9, 1) + index * 60000).toISOString(),
    }));
    const window = (params = {}) => {
      const limit = params.limit || 50, entries = timeline.slice(-limit);
      return { timeline: entries, has_more: historyLength > entries.length,
        next_cursor: historyLength > entries.length ? "synthetic-older-cursor" : null,
        timeline_incarnation: INCARNATION, timeline_read_through: null };
    };
    const { body: _body, attachments: _attachments, ...header } = canonical;
    const service = (method, params) => {
      if (method === "tasks.get") return { task: canonical,
        ...(params.timeline_paged === true ? window(params) : { timeline }) };
      const base = { status: "ok", incarnation: INCARNATION, head: "1", watermark: "1",
        cursor: `synthetic-${params.scope.kind}-cursor`, bridge_epoch: EPOCH };
      if (method === "sync.snapshot") return { ...base, next_page: null,
        records: params.scope.kind === "task" ? [{ kind: "task_head", key: TASK,
          revision: "1", row: { task: header, ...window() } }] : [] };
      if (method === "sync.changes") return { ...base, changes: [], continuation: null };
      if (method === "sync.head") return base;
      throw new Error(`Unexpected task-open RPC: ${method}`);
    };
    repliesAllowed = Promise.resolve();
    const callRpc = async (method, params = {}, envelope = {}) => {
      const entry = { method, requestBytes: bytes({ method, params, ...envelope }), responseBytes: 0 };
      if (measured) calls.push(entry);
      await repliesAllowed;
      const answer = service(method, params); entry.responseBytes = bytes(answer);
      return answer;
    };
    context.rpc = callRpc;
    runtime.startScopedSyncRuntime({ ownsLock: () => true,
      request: (_deviceId, method, params) => callRpc(method, params), onError: (error) => faults.push(error) });
    await runtime.captureScopedSync(DEVICE); await drainCache();
    if (start === "warm") await tracker.writeTaskRecord(DEVICE, PROJECT, TASK,
      { task: canonical, ...window() });
    await drainCache();
    repliesAllowed = new Promise((resolve) => { releaseReplies = resolve; });
    const address = tracker.taskAddress(DEVICE, PROJECT, TASK);
    stopCache = cache.subscribeCache(address, () => {
      if (!measured) return;
      publications += 1; firstPublication ??= elapsed();
    });
    domObserver = new MutationObserver(() => {
      if (host.querySelector(".task-page-title")?.textContent === canonical.title) firstPaint ??= elapsed();
    });
    domObserver.observe(host, { childList: true, subtree: true, characterData: true });
    const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
    started = hrtime.bigint(); measured = true;
    page = mountTaskPage(host, { deviceId: DEVICE, projectId: PROJECT, taskId: TASK,
      projectKey: `${DEVICE}|${PROJECT}`, callRpc,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn() });
    if (start === "warm") await vi.waitFor(() => expect(firstPaint).not.toBeNull());
    await vi.waitFor(() => expect(calls.some(({ method }) => method === "sync.snapshot")).toBe(true));
    const cachedPaintBeforeReplies = firstPaint !== null;
    expect(cachedPaintBeforeReplies).toBe(start === "warm");
    releaseReplies();
    await runtime.captureScopedSync(DEVICE);
    await vi.waitFor(async () => expect((await tracker.readTaskRecord(DEVICE, PROJECT, TASK))?.task.body).toBe(canonical.body));
    await vi.waitFor(() => expect(firstPaint).not.toBeNull());
    await drainCache();
    expect(faults).toEqual([]);
    const record = await tracker.readTaskRecord(DEVICE, PROJECT, TASK);
    const report = { schema: "task-open-v1", variant: process.env.TASK_OPEN_VARIANT || "unspecified",
      historyLength, start, rpcCount: calls.length,
      byMethod: Object.fromEntries([...new Set(calls.map(({ method }) => method))]
        .map((method) => [method, calls.filter((entry) => entry.method === method).length])),
      requestBytes: calls.reduce((sum, entry) => sum + entry.requestBytes, 0),
      responseBytes: calls.reduce((sum, entry) => sum + entry.responseBytes, 0),
      firstPaintMs: firstPaint, firstDataPublicationMs: firstPublication,
      dataPublications: publications, cachedPaintBeforeReplies, idbCallbacks,
      cachedTimelineRows: record.timeline.length, wallMs: elapsed(),
      measurement: "real selected-task mount + scope owner; precompleted device bootstrap; mocked transport/greeting/push subscriptions; read marks/reviews/actions excluded" };
    if (process.env.TASK_OPEN_COST_OUTPUT) appendFileSync(process.env.TASK_OPEN_COST_OUTPUT, `${JSON.stringify(report)}\n`);
    console.log(`TASK_OPEN_COST ${JSON.stringify(report)}`);
    measured = false;
  }, 120000,
);
