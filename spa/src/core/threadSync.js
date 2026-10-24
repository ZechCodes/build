// The one read-and-merge path for a conversation window.
//
// The ordered cache pass and a surface opening a cold conversation both come
// through here. That keeps the latest-window request, gap handling and cache
// write identical; the surface only gives a cold read foreground priority.
//
// # What a forward read cannot see (#120)
//
// A forward read walks the conversation's own order: the items made after the
// cursor. An item under the cursor that changed in place — a delivery status,
// a settled message, a tool call's answer — is carried by a push while a
// subscription is held, and by nothing while none is. So when the forward
// page says the conversation's counter moved on something the page does not
// carry, the recent items the record holds are read again and the changed
// ones taken.
//
// The forward read moves the cursor past the evidence, so the debt is written
// down in the same write: `repairThrough`, the counter a repair must read at or
// past. It stays on the record until a repair has — through a failed repair, a
// pass stood down under it, a reload.

import { LATEST_THREAD_ITEMS, LEAN_COLD_THREAD_MESSAGES, REPAIRED_THREAD_ITEMS } from "./cacheThresholds.js";
import { mergeCachedAtomically, readCached } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";
import { bridgeCapabilities } from "./changeEvents.js";
import {
  THREAD_RECORD_KIND,
  isProvisionalItem,
  latestItemSequence,
  mergeThreadItems,
  threadItemKey,
  windowFromThreadPayload,
} from "./thread.js";
import { digestCovering, mergeActivityDigests } from "./activityDigest.js";
import { threadHistoryStart, trimThreadHistory } from "./threadRetention.js";

const ALWAYS_ACTIVE = () => true;
const threadSub = (request) => request.conversationId || request.agentId || "";
const validThreadRequest = (request) =>
  !!request?.deviceId && !!request.entityId && !!threadSub(request) && typeof request.call === "function";
const threadTarget = (request) => request.address || {
  deviceId: request.deviceId,
  entityId: request.entityId,
  kind: THREAD_RECORD_KIND,
  sub: threadSub(request),
};
const requestPriority = (request) => request.priority === undefined ? "foreground" : request.priority;
const requestIsActive = (request) => (request.active === undefined ? ALWAYS_ACTIVE : request.active)();

async function requestThreadPage(request, params, cold = false) {
  try {
    const activity = threadActivityParam(request.deviceId);
    const limit = cold && activity.activity === "newest" ? LEAN_COLD_THREAD_MESSAGES : params.limit;
    const page = await request.call("thread.page", { ...params, ...activity, limit }, requestPriorityFields(requestPriority(request)));
    return page ? { page, lean: activity.activity === "newest" } : null;
  } catch {
    return null;
  }
}

/** Lean projection is a per-device wire feature, checked at dispatch. */
export const threadActivityParam = (deviceId) =>
  bridgeCapabilities(deviceId)?.threads?.leanActivity === true ? { activity: "newest" } : {};

/** One conversation, read forward from the sequence the cache holds — or the
 *  latest page where it holds none — and its recent items read again where
 *  that read says one of them changed. */
export async function syncThreadWindow(request) {
  if (!validThreadRequest(request)) return false;
  const target = threadTarget(request);
  const held = (await readCached(target))?.value;
  const after = Number(held?.deliveredSequence || 0);
  const response = await requestThreadPage(request, { ...threadPageParams(request.entityId, request.agentId || "", after), ...threadGenerationParam(threadIdFor(request, held)) }, !after);
  if (!response) return false;
  const { page, lean } = response;
  if (!requestIsActive(request)) return false;
  const owed = changedUnderCursor(page, after, lean) ? Number(page.thread_last_sequence) : 0;
  await mergeCachedAtomically(target, (current) =>
    mergedThreadPage(request, current, page, after, owed, lean));
  await repairRecentItems(request, target);
  return true;
}

/** The window the forward merge leaves, with the repair it owes written into
 *  it — onto the window as it stood where the page added nothing to it. */
function owingRepair(current, next, owed) {
  const window = next || current;
  if (!owed || !holdsAWindow(window)) return next;
  return { ...window, repairThrough: Math.max(owed, Number(window.repairThrough || 0)) };
}

/**
 * Whether something under the cursor changed in place: the conversation's
 * counter moved past it on a value no item on the forward page wears.
 *
 * Every item made and every change in place takes the next value of one
 * counter (bridge thread/conversation.rs `next`), and an unbroken forward page
 * carries every item made past the cursor. So a value past the cursor that no
 * item on it wears was taken by a change to an item under the cursor. An item
 * changed twice leaves its first value unworn too, which costs a read and
 * never misses one. A page that skipped a gap replaced the window whole, and
 * a window that was never held has nothing to repair.
 */
function changedUnderCursor(page, after, lean) {
  if (!after || after < threadHistoryStart(page) || pageSkipsWindow(page, lean)) return false;
  const omitted = lean ? omittedNewActivity(page, after) : 0;
  return Number(page.thread_last_sequence || 0) - after > valuesWornPast(page.items, after).size + omitted;
}

/** A wholly new run's census accounts for creations intentionally omitted
 * by lean paging. A run crossing the cursor says nothing exact about how
 * many creations happened since it, so those reads keep conservative repair. */
const omittedNewActivity = (page, after) =>
  (page.activity_digests || []).filter((digest) => digest.from_sequence > after).reduce((total, digest) => {
    const shipped = (page.items || []).filter((item) =>
      item.data?.sequence >= digest.from_sequence && item.data?.sequence <= digest.through_sequence).length;
    return total + Math.max(0, Number(digest.rows || 0) - shipped);
  }, 0);

/** Every counter value past the cursor that an item wears. */
function valuesWornPast(items, after) {
  const worn = new Set();
  for (const item of items || []) {
    for (const value of [item?.data?.sequence, item?.data?.updated_sequence]) {
      if (Number(value) > after) worn.add(Number(value));
    }
  }
  return worn;
}

/** Where the record owes one: the newest items it holds, read again, and the
 *  ones that changed taken. Nothing it does not hold is added and the cursor
 *  does not move: the forward read owns both, and a repair that crossed a new
 *  item on the wire must not carry the cursor past it. */
async function repairRecentItems(request, target) {
  const held = (await readCached(target))?.value;
  if (!held?.repairThrough) return;
  const floor = repairFloor(held);
  if (floor === null || !requestIsActive(request)) return;
  const page = await recentRepairPage(request, held, floor);
  if (!page || !requestIsActive(request)) return;
  await mergeCachedAtomically(target, (current) => requestIsActive(request) ? repairedThreadWindow(current, page) : null);
}

async function recentRepairPage(request, held, floor) {
  const response = await requestThreadPage(request, {
    entity_id: request.entityId,
    ...(request.agentId ? { agent_id: request.agentId } : {}),
    ...threadGenerationParam(request.threadId || held.thread_id),
    after_sequence: floor,
    limit: REPAIRED_THREAD_ITEMS,
  });
  if (!response || !requestIsActive(request)) return null;
  return repairedHeldCopies(request, held, response.page, response.lean);
}

/** Repair only recent held representatives omitted by a cap or projection.
 * At most REPAIRED_THREAD_ITEMS exact reads can follow one repair page. A
 * sparse window therefore makes progress without walking all omitted history;
 * failed reads leave debt in place rather than blessing stale rows. */
async function repairedHeldCopies(request, held, page, lean) {
  const shipped = new Set((page.items || []).map(threadItemKey));
  const missing = recentHeldItems(held).filter((item) =>
    !shipped.has(threadItemKey(item)) && (page.has_more === true
      || (lean && !!digestCovering(page.activity_digests, item.data?.sequence))));
  const copies = [];
  for (const item of missing) {
    const projected = lean && digestCovering(page.activity_digests, item.data?.sequence);
    const copy = await (projected ? readActivityCopy(request, held, item) : readExactCopy(request, held, item));
    if (!copy) return null;
    if (copy.item) copies.push(copy.item);
  }
  return { ...page, items: mergeThreadItems(page.items, copies) };
}

/** An unprojected one-item forward read reaches exactly the known creation.
 * Its snapshot counter is intentionally ignored: this read repairs a held
 * copy and cannot certify newly created messages beyond the forward page. */
async function readExactCopy(request, held, item) {
  if (!requestIsActive(request)) return null;
  const sequence = Number(item.data.sequence);
  try {
    const page = await request.call("thread.page", {
      entity_id: request.entityId,
      ...(request.agentId ? { agent_id: request.agentId } : {}),
      ...threadGenerationParam(threadIdFor(request, held)),
      after_sequence: sequence - 1,
      limit: 1,
    }, requestPriorityFields(requestPriority(request)));
    const threadId = threadIdFor(request, held);
    if (threadId && page.thread_id !== threadId) return null;
    return (page.items || []).find((copy) => copy.data?.sequence === sequence) || null;
  } catch {
    return null;
  }
}

async function readActivityCopy(request, held, item) {
  if (!requestIsActive(request)) return null;
  const sequence = item.data.sequence;
  try {
    const page = await request.call("thread.activity", {
      entity_id: request.entityId,
      ...(request.agentId ? { agent_id: request.agentId } : {}),
      ...threadGenerationParam(threadIdFor(request, held)),
      from_sequence: sequence,
      through_sequence: sequence,
      limit: 1,
    }, requestPriorityFields(requestPriority(request)));
    await publishActivityFloor(request, held, page);
    const item = (page.items || []).find((copy) => copy.data?.sequence === sequence);
    return (item || sequence < threadHistoryStart(page)) ? { item } : null;
  } catch {
    return null;
  }
}

async function publishActivityFloor(request, held, page) {
  if (!threadHistoryStart(page) || !requestIsActive(request)) return;
  const scoped = { ...page, ...threadGenerationParam(threadIdFor(request, held)) };
  await mergeCachedAtomically(threadTarget(request), (current) =>
    requestIsActive(request) && !differentThreadGeneration(current, scoped)
      ? trimThreadHistory(current, scoped) : null);
}

/** The sequence just under the newest items the record holds from the wire,
 *  or null where it holds none. */
function repairFloor(held) {
  const recent = recentHeldItems(held);
  if (!recent.length) return null;
  return Math.max(0, Number(recent[0].data?.sequence || 0) - 1);
}

const recentHeldItems = (held) => (held?.items || [])
  .filter((item) => !isProvisionalItem(item)).slice(-REPAIRED_THREAD_ITEMS);

/** The window with the items a repair page carries newer copies of, and its
 *  debt paid where the page was read at or past it — or null where neither. */
export function repairedThreadWindow(held, page) {
  if (!holdsAWindow(held) || differentThreadGeneration(held, page)) return null;
  const trimmed = trimThreadHistory(held, page);
  const newer = newerCopies(trimmed.items, page?.items);
  const paid = repaidBy(held, page);
  if (!newer.length && !paid) return trimmed === held ? null : trimmed;
  const { repairThrough, ...window } = trimmed;
  return trimThreadHistory({
    ...window,
    ...(paid || !repairThrough ? {} : { repairThrough }),
    items: newer.length ? mergeThreadItems(trimmed.items, newer) : trimmed.items,
    ...repairedDigests(trimmed, page),
  }, page);
}

const repairedDigests = (held, page) => page.activity_digests?.length
  ? { activityDigests: mergeActivityDigests(held.activityDigests, page) } : {};

/** Whether this page was read at or past the counter the record owes a
 *  repair through. */
const repaidBy = (held, page) =>
  Boolean(held.repairThrough) && Number(page?.thread_last_sequence || 0) >= Number(held.repairThrough)
  && repairCoversHeld(held, page);

/** A capped repair can spend its page on omitted activity before reaching
 * a recent held representative. Its global counter alone cannot pay that
 * representative's debt. Lean exact-copy reads join page.items beforehand. */
function repairCoversHeld(held, page) {
  if (page.has_more !== true) return true;
  const shipped = new Set((page.items || []).map(threadItemKey));
  return recentHeldItems(held).every((item) => shipped.has(threadItemKey(item)));
}

/** The arrivals that are newer copies of items held. */
function newerCopies(heldItems, arrived) {
  const heldByKey = new Map(heldItems.map((item) => [threadItemKey(item), item]));
  return (arrived || []).filter((item) => {
    const copy = heldByKey.get(threadItemKey(item));
    return copy && latestItemSequence(item) > latestItemSequence(copy);
  });
}

export const threadPageParams = (entityId, agentId, after) => ({
  entity_id: entityId,
  ...(agentId ? { agent_id: agentId } : {}),
  ...(after ? { after_sequence: after, newest: true } : {}),
  limit: LATEST_THREAD_ITEMS,
});

/** Whether the record is a window over the conversation, or only this tab's
 *  own stand-ins waiting for one. */
const holdsAWindow = (held) => Number(held?.deliveredSequence || 0) > 0;

/**
 * The saved window after a page.
 *
 * With no window held, the page IS the window: it is the latest page of items,
 * and `has_more` on it means the conversation reaches back further than the
 * window does. With a window held, an abutting page is appended. A newest-mode
 * page whose `has_more` says it skipped a gap replaces the stale window with
 * the returned tip, leaving that gap behind the ordinary load-older path.
 */
export function threadWindow(held, page, { newest = false, lean = false } = {}) {
  if (differentThreadGeneration(held, page)) return null;
  const arrived = windowFromThreadPayload(page);
  if (!held) return trimThreadHistory(arrived, page);
  if (!arrived) return trimThreadHistory(held, page);
  const next = newest && pageSkipsWindow(page, lean)
    ? newestThreadWindow(held, arrived, page) : appendedThreadWindow(held, arrived, page);
  return trimThreadHistory({ ...next, ...(held.historyStartSequence ? { historyStartSequence: held.historyStartSequence } : {}) }, page);
}

/** A newest message-budget cut starts at a message. A leading folded run
 * means has_more describes projected activity, including update-counter gaps. */
const pageSkipsWindow = (page, lean) =>
  page.has_more === true && (!lean || !digestCovering(page.activity_digests, page.items?.[0]?.data?.sequence));

const appendedThreadWindow = (held, arrived, page) => ({
  // A record of stand-ins alone is not a window to append to: the page is
  // the window, and only the reader's own messages carry over into it.
  ...(holdsAWindow(held) ? held : arrived),
  // Merged rather than appended: the record may hold this tab's own message
  // waiting for the wire to carry it, and the arrival takes that stand-in away.
  ...heldGenerationMetadata(held, arrived),
  items: mergeThreadItems(held.items, arrived.items),
  deliveredSequence: Math.max(Number(held.deliveredSequence || 0), arrived.deliveredSequence),
  knownTotalItems: arrived.knownTotalItems ?? held.knownTotalItems ?? null,
  activityDigests: mergeActivityDigests(held.activityDigests || [], page),
});

/** Reset authority belongs to the standing record even before its first
 * wire item arrives; a lean push may omit its generation revision. */
const heldGenerationMetadata = (held, arrived) => ({
  ...(held.thread_id ? { thread_id: held.thread_id,
    thread_generation_revision: Math.max(Number(held.thread_generation_revision || 0), Number(arrived.thread_generation_revision || 0)) } : {}),
  ...(held.retired_thread_ids ? { retired_thread_ids: held.retired_thread_ids } : {}),
});

/** Replace a stale window with a newest-forward page that skipped a gap.
 *
 * A send or push can write the record while the page is crossing the wire.
 * Preserve provisional stand-ins, plus wire items beyond the page's own
 * high-water, but none of the stale pre-gap window. */
const newestThreadWindow = (held, arrived, page) => {
  const pageLastSequence = Number(page.thread_last_sequence || highestCreationSequence(arrived.items));
  const provisional = (held.items || []).filter(isProvisionalItem);
  const newer = (held.items || []).filter((item) =>
    !isProvisionalItem(item) && latestItemSequence(item) > pageLastSequence);
  const items = mergeThreadItems(mergeThreadItems(provisional, arrived.items), newer);
  return {
    ...arrived,
    ...heldGenerationMetadata(held, arrived),
    items,
    deliveredSequence: Math.max(Number(held.deliveredSequence || 0), arrived.deliveredSequence),
    knownTotalItems: Math.max(Number(held.knownTotalItems || 0), Number(arrived.knownTotalItems || 0)) || null,
  };
};

const highestCreationSequence = (items) =>
  (items || []).reduce((highest, item) => Math.max(highest, Number(item?.data?.sequence || 0)), 0);

export const threadGenerationParam = (threadId) => threadId ? { thread_id: threadId } : {};
export const differentThreadGeneration = (held, page) => Boolean(held?.thread_id && page?.thread_id !== held.thread_id);

const threadIdFor = (request, held) => request.threadId || held?.thread_id;
function mergedThreadPage(request, current, page, after, owed, lean) {
  if (!requestIsActive(request) || differentThreadGeneration(current, page)) return null;
  const next = threadWindow(current, page, { newest: after > 0, lean });
  return owingRepair(current, forwardCoveredWindow(current, next, page), owed);
}

/** Only the forward RPC owns creation coverage. Its newest window reaches
 * the snapshot's tail (replacing an omitted message span where necessary),
 * so its counter also covers in-place changes with no newly created row.
 * A later repair response must never supply this cursor: it may already see
 * new messages that the forward request did not read. */
function forwardCoveredWindow(current, next, page) {
  const window = next || current;
  const through = Number(page.thread_last_sequence || 0);
  if (!holdsAWindow(window) || through <= Number(window.deliveredSequence || 0)) return next;
  return { ...window, deliveredSequence: through };
}
