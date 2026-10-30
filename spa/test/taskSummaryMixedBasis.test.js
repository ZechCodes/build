import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, summaries, content;
const scope = { deviceId: "mixed-basis-900", projectId: "proj-1" }, filters = { statuses: ["ready"] };
const row = (extra = {}) => ({ id: "task-1", project_id: scope.projectId, number: 1, title: "Body from B1",
  state: "open", status: "done", watched: false, read_through: null, dismissed_through: null,
  updated_at: "2027-10-29T12:00:00Z", ...extra });
const value = async () => (await cache.readCached(summaries.summaryAddress(scope, "task-1")))?.value;
async function reopen() {
  vi.resetModules(); cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  content = await import("../src/core/taskContentAuthority.js");
}
async function land(task, basis, query = filters) {
  const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const read = contentRead.readOrder, ticket = await summaries.beginSummaryQuery(scope, query, read, { contentRead });
  expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [task], ...(basis === undefined ? {} : { basis }) }, { read })).toBe(true);
  return read;
}
const current = (extra = {}) => row({ title: "Current B2 body", status: "ready", updated_at: "2026-10-29T12:00:00Z", ...extra });
async function expectMixed(basis) {
  expect(await summaries.readSummaryQuery(scope, filters)).toMatchObject({ complete: true, stale: true, basis,
    tasks: [{ id: "task-1", status: "done", title: "Body from B1" }] });
  expect(await value()).toMatchObject({ basis_uncertain: true, needs_unread_refresh: false });
  expect((await value()).basis).toBeUndefined();
}
beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange; await reopen();
});

it("keeps an unchanged-watch predicate mismatch uncertain through reopen and later partial snapshots until ordinary content wins", async () => {
  await land(row(), "B1", {});
  const read = await land(current(), "B2");
  expect((await value()).watch_read_order).toBe(read);
  await expectMixed("B2");
  await reopen(); await expectMixed("B2");
  await land(current(), "B3"); await expectMixed("B3");
  await land(current({ updated_at: "2028-10-29T12:00:00Z" }), "B4");
  expect(await summaries.readSummaryQuery(scope, filters)).toMatchObject({ complete: true, stale: false, basis: "B4",
    tasks: [{ status: "ready", title: "Current B2 body" }] });
  expect(await value()).toMatchObject({ basis: "B4", basis_uncertain: false, needs_unread_refresh: false });
});

it("keeps recovered watch counters useful without certifying retained ordinary content or reference absence", async () => {
  const held = row({ watched: true, watch_started_after: "opaque-zzzz", unread_count: 0,
    unread_mentions: 0, unread_agent_comments: 0 });
  await land(held, "B1", {});
  const incoming = current({ watched: true, watch_started_after: "te-01M64A9D058M2B7EMP3N30G6YM",
    unread_count: 2, unread_mentions: 1, unread_agent_comments: 2 });
  await land(incoming, "B2"); await expectMixed("B2");
  expect((await value()).task).toMatchObject({ watch_started_after: incoming.watch_started_after, unread_count: 2 });
  await reopen(); await expectMixed("B2");
  const { referenceTables, resolverFor } = await import("../src/core/referenceTargets.js");
  const { deviceKey } = await import("../src/core/deviceKey.js");
  const key = deviceKey(scope.deviceId, scope.projectId), query = await summaries.readSummaryQuery(scope, filters);
  const sources = { tasks: { [key]: query.tasks } };
  expect(resolverFor(referenceTables(sources), { place: scope }).task(42)).toBeUndefined();
  expect(resolverFor(referenceTables({ ...sources, taskLookups: { [key]: { 42: { status: "missing" } } } }),
    { place: scope }).task(42)).toBeNull();
});

it("does not treat two absent basis values as proof while ordinary content remains protected", async () => {
  await land(row(), undefined, {}); await land(current(), undefined);
  expect(await value()).toMatchObject({ basis_uncertain: true, needs_unread_refresh: false });
  expect(await summaries.readSummaryQuery(scope, filters)).toMatchObject({ complete: true, stale: true,
    tasks: [{ status: "done", title: "Body from B1" }] });
  await reopen(); expect(await value()).toMatchObject({ basis_uncertain: true });
});
