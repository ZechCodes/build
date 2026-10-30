// Which read last had the say on each task of a list record (#85).
//
// A page of `tasks.list` can be out while a newer read of the same list
// lands — a push re-read, the Tasks tab's own read, the same read in another
// tab — and then lands after it, the older word. Its copy of a row can be
// older with the same `updated_at` (the bridge fills in who a task's agent
// is when it lists it, and touches no timestamp), and a row it still names
// can already have left the list. So what decides is the order the reads were
// ASKED in: every read takes a number before it asks, and a page lays a row,
// or takes one away, only where no read asked after it has had the say on
// that row.
//
// A page has the say on a stretch of task numbers, not only on the rows it
// names: every number in it that it does not name is not on the list. Numbers
// never move, so the stretch is kept, and a row a newer page left out stays
// out — the tombstone a timestamp cannot be, and one that holds for a row the
// cache never had. A card moved here has the say by its id.
//
// The same rule as `pushFence` (#142), per stretch rather than per record,
// since a page is the truth about a stretch of a list and not the whole of it.
//
// Every tab on this browser profile writes the same list records, so the
// order is theirs to share, and it lives in the cache beside the lists: the
// number is a count every tab raises in one transaction, and what each read
// had the say on is a note of its own beside each list, written in the same
// transaction as the page it notes (`foldTasksPage`). A page that lands
// after another tab's newer read reads that tab's word there.

import { mergeCachedAtomically, mergeCachedRecordsTogether, recordWriteOf, takeCachedCount } from "./localCache.js";
import { randomToken } from "./idbRecords.js";

/** The kind of the note of what each read had the say on under a list
 *  record: one note per list, under the list's own device and project. */
export const TASK_READS_KIND = "tracker-task-reads";

/** The count every tab takes a read's number from. Under no device and no
 *  entity, like the device list: every list's reads share the one count. */
export const TASK_READ_COUNT_ADDRESS = Object.freeze({ deviceId: "", entityId: "", kind: "tracker-task-read-count" });
export const TASK_CONTENT_GENERATION_ADDRESS = Object.freeze({ deviceId: "", entityId: "", kind: "task-content-generation" });
export const taskWatchControlAddress = ({ deviceId, projectId }) => ({ deviceId, entityId: "", kind: "task-watch-control", sub: projectId });
export const taskContentScopeAddress = ({ deviceId, projectId }) => ({ deviceId, entityId: projectId, kind: "task-content-scope" });

/** A content observation never certifies a module-local fallback number.
 * Allocate its shared count and capture its store/scope lifetime and optional
 * exact canonical baseline in the same transaction, including absence. */
export async function nextTaskReadTicket(scope, { address } = {}) {
  const addresses = [TASK_READ_COUNT_ADDRESS, TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress(scope), taskWatchControlAddress(scope), ...(address ? [address] : [])];
  let ticket = null;
  let invalid = false;
  const committed = await mergeCachedRecordsTogether(addresses, ([count, store, domain, watch, baseline]) => {
    const allocation = sharedContentAllocation(count, store, domain, watch);
    if (!allocation) { invalid = true; return null; }
    ticket = { ...allocation.ticket, ...(address ? { written: recordWriteOf(baseline) } : {}) };
    return [...allocation.values, ...(address ? [null] : [])];
  });
  if (invalid) throw new Error("Invalid shared task content counter allocation");
  return committed ? ticket : null;
}

function sharedContentAllocation(count, store, domain, watch) {
  const held = count ? count.value : 0;
  if (!Number.isSafeInteger(held) || held < 0) return null;
  const readOrder = Math.max(held + 1, Date.now());
  if (!Number.isSafeInteger(readOrder) || readOrder <= 0) return null;
  return contentAllocationValues(readOrder, store, domain, watch);
}

export const initialTaskWatchControl = () => ({ generation: randomToken(), pendingGeneration: randomToken(),
  watchRetirementEpoch: randomToken(), blindRetirementEpoch: randomToken(), pendingCount: 0, uncertain: true });

function contentAllocationValues(readOrder, store, domain, control) {
  const generation = store?.value?.generation || randomToken();
  const scopeValue = contentScopeValue(domain?.value), watch = control?.value || initialTaskWatchControl();
  const ticket = contentScopeTicket(readOrder, generation, scopeValue, watch);
  return { ticket, values: [readOrder, store ? null : { generation }, domain ? null : scopeValue, control ? null : watch] };
}

function contentScopeValue(held = {}) {
  return { ...held, generation: held.generation || randomToken(), retirementEpoch: held.retirementEpoch || randomToken() };
}

function contentScopeTicket(readOrder, generation, scope, watch) {
  return { readOrder, storeGeneration: generation, scopeGeneration: scope.generation,
    retirementEpoch: scope.retirementEpoch, watchGeneration: watch.generation,
    watchRetirementEpoch: watch.watchRetirementEpoch,
    watchPending: !Number.isSafeInteger(watch.pendingCount) || watch.pendingCount !== 0,
    watchUncertain: watch.uncertain !== false || !watch.blindRetirementEpoch };
}

/** Where the note of what each read had the say on under `address` lives. */
export const readsAddress = ({ deviceId, entityId, kind, sub = "" }) => ({
  deviceId,
  entityId,
  kind: TASK_READS_KIND,
  sub: JSON.stringify([kind, sub]),
});

export const taskWatchIntentAddress = ({ deviceId, projectId }, taskId) => readsAddress({
  deviceId, entityId: projectId, kind: "tracker-task", sub: taskId,
});

let uncounted = 0;

/** The number a read takes before it asks: every later one is newer, in this
 *  tab and every other. Never under the clock, so a count wiped with the
 *  cache does not start again under a page still out. Without a cache there
 *  is nothing to fold into and nothing to share, and the count is this tab's. */
export async function nextTaskRead() {
  const taken = await takeCachedCount(TASK_READ_COUNT_ADDRESS, Date.now());
  if (Number.isFinite(taken)) return taken;
  uncounted = Math.max(uncounted + 1, Date.now());
  return uncounted;
}

const covers = (stretch, number) => number < stretch.above && number >= stretch.through;

/** The newest read that had the say on a row under a list, by the list's note
 *  `reads`, 0 for none: one whose stretch holds the row's number, or one that
 *  wrote it by id. */
export function lastSayIn(reads) {
  const stretches = reads?.stretches || [];
  const rows = reads?.rows || {};
  return (row) => stretches.reduce(
    (newest, stretch) => (covers(stretch, Number(row.number)) ? Math.max(newest, stretch.read) : newest),
    Number(rows[row.id]) || 0,
  );
}

/** The note with a page's say added: each of its spans, `through` (inclusive)
 *  up to `above` (exclusive), as read `read`. The last page of a pull that
 *  began as read `pullRead` completes it: every number has had a say at least
 *  that new, so every older say is spent and forgotten. */
export function withStretch(reads, stretch) {
  const { above, through, read, pullRead = read } = stretch;
  const held = through === -Infinity ? forgetOlder(reads, pullRead) : reads;
  const spans = (stretch.spans || [{ above, through, read }])
    .map((span) => ({ above: span.above, through: span.through, read: span.read }));
  return { stretches: [...(held?.stretches || []), ...spans], rows: { ...held?.rows } };
}

function forgetOlder(reads, read) {
  return {
    stretches: (reads?.stretches || []).filter((stretch) => stretch.read >= read),
    rows: Object.fromEntries(Object.entries(reads?.rows || {}).filter(([, said]) => said >= read)),
  };
}

/** The note with these tasks written, by id, as read `read`. */
export const withWritten = (reads, taskIds, read) => ({
  stretches: reads?.stretches || [],
  rows: { ...reads?.rows, ...Object.fromEntries(taskIds.map((taskId) => [taskId, read])) },
});

/** These tasks were written under these list records here — a card moved
 *  before the bridge said so — which is newer than any read already out, in
 *  this tab or another. Noted before the write it is about, and answers the
 *  number it was noted as, which the write stamps its lists with. */
export async function noteWritten(addresses, taskIds) {
  const read = await nextTaskRead();
  await Promise.all(addresses.map((address) =>
    mergeCachedAtomically(readsAddress(address), (reads) => withWritten(reads, taskIds, read))));
  return read;
}
