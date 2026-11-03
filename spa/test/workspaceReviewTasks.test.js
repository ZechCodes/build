// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const live = vi.hoisted(() => ({}));
vi.mock("../src/core/deviceContexts.js", async (original) => ({ ...await original(),
  contextFor: () => live.context,
  deviceContextIdentity: () => live.identity,
  whenGreeted: async (context, dispatch) => {
    const session = live.session, greeting = context.greeted;
    const stands = () => live.context === context && live.session === session && context.greeted === greeting;
    return stands() ? { sent: dispatch(stands), stands } : null;
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({ deviceSession: () => live.session }));

const place = { deviceId: "review-choices", projectId: "proj-1", workspaceId: "workspace-1" };
const support = { taskChoices: true, linkedTaskChoices: false };
const row = (id, extra = {}) => ({ id, number: Number(id.slice(-1)), project_id: place.projectId,
  title: id, state: "open", watched: false, updated_at: "2026-11-01T00:00:00Z", ...extra });
const page = (tasks, extra = {}) => ({ summary: true, tasks, ...extra });
const deferred = () => { let resolve; const promise = new Promise((yes) => { resolve = yes; }); return { promise, resolve }; };
let cache, summaries, content, choices, fence;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  live.session = {}; live.identity = "current";
  live.context = { active: () => true, greeted: {}, adapter: { capabilities: {
    reviews: support, tasks: { listSummaries: true, listLinkedWorkspace: true },
  } } };
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  fence = await import("../src/core/taskMutationFence.js");
  choices = await import("../src/core/workspaceReviewTasks.js");
});
afterEach(() => vi.restoreAllMocks());

it.each(["session", "greeting", "identity"])("refuses a task-choice reply after %s replacement", async (replaced) => {
  const answer = deferred(), changed = vi.fn();
  const scope = { ...place, callRpc: vi.fn(() => answer.promise) };
  const reading = choices.refreshReviewTaskChoices(scope, support, () => true, changed);
  await vi.waitFor(() => expect(scope.callRpc).toHaveBeenCalledOnce(), { timeout: 5000 });
  if (replaced === "session") live.session = {};
  if (replaced === "greeting") live.context.greeted = {};
  if (replaced === "identity") live.identity = "replacement";
  answer.resolve(page([row("t1")]));
  const outcome = await reading;
  expect(await choices.readReviewTaskChoices(scope)).toEqual([]);
  expect(await cache.readCached(summaries.summaryAddress(scope, "t1"))).toBeUndefined();
  expect(changed).not.toHaveBeenCalled();
  expect(outcome).toMatchObject({ complete: false });
});

it("refuses a pre-mutation reply and admits the next covering task-choice read", async () => {
  const answer = deferred(), mutation = deferred();
  const scope = { ...place, callRpc: vi.fn().mockImplementationOnce(() => answer.promise)
    .mockResolvedValue(page([row("t2")])) };
  const reading = choices.refreshReviewTaskChoices(scope, support, () => true, vi.fn());
  await vi.waitFor(() => expect(scope.callRpc).toHaveBeenCalledOnce(), { timeout: 5000 });
  const pending = fence.wrapTaskMutationCall(scope.deviceId, "tasks.update", () => mutation.promise);
  mutation.resolve({}); await pending;
  answer.resolve(page([row("t1")]));
  const outcome = await reading;
  expect(await choices.readReviewTaskChoices(scope)).toEqual([]);
  expect(await cache.readCached(summaries.summaryAddress(scope, "t1"))).toBeUndefined();
  expect(outcome).toMatchObject({ complete: false });
  expect(await choices.refreshReviewTaskChoices(scope, support, () => true, vi.fn())).toMatchObject({ complete: true });
  expect((await choices.readReviewTaskChoices(scope)).map(({ id }) => id)).toEqual(["t2"]);
});

it("selects normalized rows before legacy timestamps and filters after newest-source selection", async () => {
  await cache.writeCached({ deviceId: place.deviceId, entityId: place.projectId, kind: "tracker-tasks" }, {
    tasks: [row("t1", { title: "Future legacy copy", updated_at: "2030-01-01T00:00:00Z" }), row("t3")],
  });
  const contentRead = await content.captureTaskContentRead(place, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(place, { state: "open" }, contentRead.readOrder, { contentRead });
  expect(await summaries.landSummaryPage(ticket, page([row("t1", { title: "Normalized choice" }), row("t2"),
    row("t3", { state: "closed" })]), { read: contentRead.readOrder })).toBe(true);
  const tasks = await choices.readReviewTaskChoices(place);
  expect(tasks.map(({ id }) => id)).toEqual(["t2", "t1"]);
  expect(tasks.find(({ id }) => id === "t1").title).toBe("Normalized choice");
});

it("continues the filtered linked query automatically and retains its preselection", async () => {
  const linkedSupport = { ...support, linkedTaskChoices: true };
  live.context.adapter.capabilities.reviews = linkedSupport;
  const member = { matched_memberships: { linked_workspace_id: place.workspaceId } };
  const scope = { ...place, callRpc: vi.fn(async (_method, params) => {
    if (!params.linked_workspace_id) return page([row("t2")]);
    return params.cursor ? page([row("t1", member)])
      : page([row("t3", { ...member, review_summary: { mode: "pull_request" } })], { next_cursor: "linked-next" });
  }) };
  const changed = vi.fn();
  expect(await choices.refreshReviewTaskChoices(scope, linkedSupport, () => true, changed)).toMatchObject({ complete: true });
  expect(scope.callRpc).toHaveBeenCalledTimes(3);
  expect(scope.callRpc.mock.calls[1][1]).toMatchObject({ linked_workspace_id: place.workspaceId, cursor: "linked-next", limit: 100 });
  expect(choices.preselectedReviewTask({ workspace: { id: place.workspaceId } }, await choices.readReviewTaskChoices(scope))).toBe("t1");
  expect(changed).toHaveBeenCalledTimes(3);
});

it("stops a stalled linked cursor and still reads ordinary choices", async () => {
  const linkedSupport = { ...support, linkedTaskChoices: true };
  live.context.adapter.capabilities.reviews = linkedSupport;
  const member = { matched_memberships: { linked_workspace_id: place.workspaceId } };
  const scope = { ...place, callRpc: vi.fn(async (_method, params) => params.linked_workspace_id
    ? page([row("t1", member)], { next_cursor: "same" }) : page([row("t2")])) };
  const changed = vi.fn();
  expect(await choices.refreshReviewTaskChoices(scope, linkedSupport, () => true, changed)).toMatchObject({ complete: true });
  expect(scope.callRpc).toHaveBeenCalledTimes(3);
  expect(changed).toHaveBeenCalledTimes(3);
  expect((await summaries.readSummaryQuery(scope, { state: "open", linked_workspace_id: place.workspaceId })).next_cursor).toBe("same");
});

it("does not continue a retired query even with a new content capture", async () => {
  const scope = { ...place, callRpc: vi.fn(async () => page([row("t1")], { next_cursor: "next", basis: "before" })) };
  await choices.refreshReviewTaskChoices(scope, support, () => true, vi.fn());
  await content.mergeTaskContentRecordsTogether(scope, [], () => ({ values: [], retire: true }));
  expect((await choices.readReviewTaskPage(scope)).stale).toBe(true);
  expect(await choices.refreshReviewTaskChoices(scope, support, () => true, vi.fn(), "next")).toMatchObject({ complete: false });
  expect(scope.callRpc).toHaveBeenCalledOnce();
});

it("reports incomplete after a lost cache reread while retaining the admitted payload", async () => {
  const scope = { ...place, callRpc: vi.fn(async () => page([row("t1")])) };
  vi.spyOn(summaries, "readSummaryQuery").mockResolvedValueOnce(null);
  expect(await choices.refreshReviewTaskChoices(scope, support, () => true, vi.fn())).toMatchObject({ complete: false });
  expect((await cache.readCached(summaries.summaryAddress(scope, "t1"))).value.task.id).toBe("t1");
});

it("retains dev eligibility selection when only legacy rows without issued read orders exist", async () => {
  await cache.writeCached({ deviceId: place.deviceId, entityId: place.projectId, kind: "tracker-task", sub: "t1" }, {
    task: row("t1", { state: "closed", updated_at: "2026-11-02T00:00:00Z" }),
  });
  await cache.writeCached({ deviceId: place.deviceId, entityId: place.projectId, kind: "tracker-tasks-query", sub: "old" }, {
    tasks: [row("t1")],
  });
  expect(await choices.readReviewTaskChoices(place)).toEqual([]);
});

it("reads cached options with only a device and project scope", async () => {
  expect(await choices.readReviewTaskChoices({ deviceId: place.deviceId, projectId: place.projectId })).toEqual([]);
});
