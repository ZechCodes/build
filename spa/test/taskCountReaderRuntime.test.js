import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import countsFixture from "../../fixtures/api/v1/tasks.counts.json";

const live = vi.hoisted(() => ({}));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => live.current ? live.context : null,
  deviceContextIdentity: () => live.identity,
  whenGreeted: async (context, dispatch) => {
    const session = live.session, greeting = context.greeted;
    await live.greetingWait;
    const stands = () => live.current && !live.away && live.session === session && context.greeted === greeting;
    return stands() ? { sent: dispatch(stands), stands } : null;
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => live.session,
  deviceWatch: () => ({ away: () => live.away, moved: (listener) => {
    live.moved.add(listener); return () => live.moved.delete(listener);
  } }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  watchChanges: (spec) => { live.watches.add(spec); return { dispose: () => live.watches.delete(spec) }; },
  onBridgeGreeted: (listener) => { live.greeted.add(listener); return () => live.greeted.delete(listener); },
}));
vi.mock("../src/core/trackerPush.js", () => ({ tasksPushKinds: () => live.kinds }));

let start, readCounts, fence;
const running = new Set();
const scope = { deviceId: "count-runtime", projectId: "proj-1" };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const counts = (total = 5) => ({ ...countsFixture.result, agents: [], watched_unread_total: total });
const mount = () => {
  const runtime = start({ ...scope, context: live.context }); running.add(runtime); return runtime;
};
const push = () => [...live.watches].forEach(({ refresh }) => refresh());
const moved = () => [...live.moved].forEach((listener) => listener());
const greeted = () => [...live.greeted].forEach((listener) => listener(scope.deviceId));
const cachedTotal = async (total) => vi.waitFor(async () => expect((await readCounts(scope))?.watched_unread_total).toBe(total));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  Object.assign(live, { current: true, identity: "context-1", away: false, session: {}, greetingWait: undefined,
    kinds: ["tasks"], watches: new Set(), moved: new Set(), greeted: new Set() });
  live.context = { deviceId: scope.deviceId, greeted: {}, active: () => live.current,
    adapter: { capabilities: { tasks: { counts: true } } }, rpc: vi.fn().mockResolvedValue(counts()) };
  ({ followProjectTaskCounts: start } = await import("../src/core/taskCountReaderRuntime.js"));
  ({ readTaskCounts: readCounts } = await import("../src/core/taskAggregateReads.js"));
  fence = await import("../src/core/taskMutationFence.js");
});

afterEach(() => { for (const runtime of running) runtime.dispose(); running.clear(); });

it("admits exactly one bounded project counts request on mount, without catalogue or timeline fanout", async () => {
  mount(); await cachedTotal(5);
  expect(live.context.rpc.mock.calls).toEqual([["tasks.counts", { project_id: "proj-1", agent_ids: [] }]]);
  expect([...live.watches][0]).toMatchObject({ deviceId: scope.deviceId, entity: scope.projectId, kinds: ["tasks"] });
});

it.each(["unsupported", "offline", "no-session"])("paints cached content without dispatch while %s", async (mode) => {
  if (mode === "unsupported") live.context.adapter.capabilities.tasks = { listSummaries: true, lookup: true };
  if (mode === "offline") live.away = true;
  if (mode === "no-session") live.session = null;
  const runtime = mount(); await runtime.refresh();
  expect(live.context.rpc).not.toHaveBeenCalled();
  expect(await readCounts(scope)).toBeNull();
});

it("waits for the current greeting and rebinds only the mounted project's task push on upgrade", async () => {
  const hello = deferred(); live.greetingWait = hello.promise; live.kinds = [];
  const runtime = mount(); await Promise.resolve();
  expect(live.context.rpc).not.toHaveBeenCalled();
  // Real greetingInFlight/armGreeting replaces this object before its event.
  // Retire the opening read's authority before releasing its pending wait.
  live.context.greeted = {}; live.kinds = ["tasks"]; hello.resolve(); greeted(); await cachedTotal(5);
  expect([...live.watches]).toHaveLength(1);
  expect([...live.watches][0].kinds).toEqual(["tasks"]);
  expect(live.context.rpc).toHaveBeenCalledTimes(1);
  runtime.dispose();
});

it("suppresses pending mutations and refreshes once after the final acknowledgment", async () => {
  const write = deferred();
  fence.wrapTaskMutationCall(scope.deviceId, "tasks.read_through", () => write.promise);
  mount(); await Promise.resolve(); await Promise.resolve();
  expect(live.context.rpc).not.toHaveBeenCalled();
  push(); write.resolve({}); await cachedTotal(5);
  expect(live.context.rpc).toHaveBeenCalledTimes(1);
});

it.each([false, true])("rejects late pre-mutation totals and refreshes after settlement (failed=%s)", async (failed) => {
  const old = deferred(), write = deferred();
  live.context.rpc.mockImplementationOnce(() => old.promise).mockResolvedValue(counts(2));
  mount(); await vi.waitFor(() => expect(live.context.rpc).toHaveBeenCalledTimes(1));
  const mutation = fence.wrapTaskMutationCall(scope.deviceId, "tasks.watch", () => write.promise);
  void mutation.catch(() => {});
  old.resolve(counts(99)); await Promise.resolve(); await Promise.resolve();
  expect(await readCounts(scope)).toBeNull();
  if (failed) write.reject(new Error("uncertain write")); else write.resolve({});
  await cachedTotal(2); expect(live.context.rpc).toHaveBeenCalledTimes(2);
});

it("coalesces a push burst into one trailing bounded read while the first request is pending", async () => {
  const first = deferred();
  live.context.rpc.mockImplementationOnce(() => first.promise).mockResolvedValue(counts(3));
  mount(); await vi.waitFor(() => expect(live.context.rpc).toHaveBeenCalledTimes(1));
  for (let index = 0; index < 100; index += 1) push();
  await Promise.resolve(); expect(live.context.rpc).toHaveBeenCalledTimes(1);
  first.resolve(counts(7)); await cachedTotal(3);
  expect(live.context.rpc).toHaveBeenCalledTimes(2);
});

it("keeps authoritative totals on marker refusal with no error-driven retry", async () => {
  const refusal = Object.assign(new Error("unsupported marker"), countsFixture.refusals[0].reply);
  const runtime = mount(); await cachedTotal(5);
  live.context.rpc.mockRejectedValueOnce(refusal).mockResolvedValue(counts(1));
  await runtime.refresh(); await Promise.resolve(); await Promise.resolve();
  expect(live.context.rpc).toHaveBeenCalledTimes(2);
  expect((await readCounts(scope)).watched_unread_total).toBe(5);
  push(); await cachedTotal(1);
  expect(live.context.rpc).toHaveBeenCalledTimes(3);
});

it("starts a replacement session without waiting for the dead request and fences its late result", async () => {
  const old = deferred();
  live.context.rpc.mockImplementationOnce(() => old.promise).mockResolvedValue(counts(4));
  mount(); await vi.waitFor(() => expect(live.context.rpc).toHaveBeenCalledTimes(1));
  live.session = {}; live.context.greeted = {}; moved(); greeted();
  await cachedTotal(4); expect(live.context.rpc).toHaveBeenCalledTimes(2);
  old.resolve(counts(99)); await Promise.resolve(); await Promise.resolve();
  expect((await readCounts(scope)).watched_unread_total).toBe(4);
});

it("fences a same-session new greeting and a retired context identity", async () => {
  const old = deferred();
  live.context.rpc.mockImplementationOnce(() => old.promise).mockResolvedValue(counts(4));
  const runtime = mount(); await vi.waitFor(() => expect(live.context.rpc).toHaveBeenCalledTimes(1));
  live.context.greeted = {}; greeted(); await cachedTotal(4);
  old.resolve(counts(99)); await Promise.resolve();
  live.identity = "replacement-context"; push(); await runtime.refresh();
  expect(live.context.rpc).toHaveBeenCalledTimes(2);
  expect((await readCounts(scope)).watched_unread_total).toBe(4);
});

it("disposes every listener and suppresses queued dispatch and in-flight cache publication", async () => {
  const old = deferred(); live.context.rpc.mockImplementationOnce(() => old.promise);
  const runtime = mount(); await vi.waitFor(() => expect(live.context.rpc).toHaveBeenCalledTimes(1));
  push(); runtime.dispose(); old.resolve(counts(99)); await runtime.refresh();
  expect(live.context.rpc).toHaveBeenCalledTimes(1);
  expect(await readCounts(scope)).toBeNull();
  expect(live.watches.size + live.moved.size + live.greeted.size).toBe(0);
});
