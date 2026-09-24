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

import { LATEST_THREAD_ITEMS, REPAIRED_THREAD_ITEMS } from "./cacheThresholds.js";
import { mergeCached, readCached } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";
import {
  THREAD_RECORD_KIND,
  isProvisionalItem,
  latestItemSequence,
  mergeThreadItems,
  threadItemKey,
  windowFromThreadPayload,
} from "./thread.js";
import { mergeActivityDigests } from "./activityDigest.js";

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

async function requestThreadPage(request, params) {
  try {
    return await request.call("thread.page", params, requestPriorityFields(requestPriority(request)));
  } catch {
    return null;
  }
}

/** One conversation, read forward from the sequence the cache holds — or the
 *  latest hundred where it holds none — and its recent items read again where
 *  that read says one of them changed. */
export async function syncThreadWindow(request) {
  if (!validThreadRequest(request)) return false;
  const target = threadTarget(request);
  const held = (await readCached(target))?.value;
  const after = Number(held?.deliveredSequence || 0);
  const page = await requestThreadPage(request, threadPageParams(request.entityId, request.agentId || "", after));
  if (!page) return false;
  if (!requestIsActive(request)) return false;
  await mergeCached(target, (current) =>
    requestIsActive(request) ? threadWindow(current, page, { newest: after > 0 }) : null);
  if (changedUnderCursor(page, after)) await repairRecentItems(request, target);
  return true;
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
function changedUnderCursor(page, after) {
  if (!after || page.has_more === true) return false;
  return Number(page.thread_last_sequence || 0) - after > valuesWornPast(page.items, after).size;
}

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

/** The newest items the record holds, read again, and the ones that changed
 *  taken. Nothing it does not hold is added and the cursor does not move: the
 *  forward read owns both, and a repair that crossed a new item on the wire
 *  must not carry the cursor past it. */
async function repairRecentItems(request, target) {
  const held = (await readCached(target))?.value;
  const floor = repairFloor(held);
  if (floor === null || !requestIsActive(request)) return;
  const page = await requestThreadPage(request, {
    entity_id: request.entityId,
    ...(request.agentId ? { agent_id: request.agentId } : {}),
    after_sequence: floor,
    limit: REPAIRED_THREAD_ITEMS,
  });
  if (!page || !requestIsActive(request)) return;
  await mergeCached(target, (current) => requestIsActive(request) ? repairedThreadWindow(current, page) : null);
}

/** The sequence just under the newest items the record holds from the wire,
 *  or null where it holds none. */
function repairFloor(held) {
  const recent = (held?.items || []).filter((item) => !isProvisionalItem(item)).slice(-REPAIRED_THREAD_ITEMS);
  if (!recent.length) return null;
  return Math.max(0, Number(recent[0].data?.sequence || 0) - 1);
}

/** The window with the items a repair page carries newer copies of, or null
 *  where it carries none. */
export function repairedThreadWindow(held, page) {
  if (!holdsAWindow(held)) return null;
  const heldByKey = new Map(held.items.map((item) => [threadItemKey(item), item]));
  const newer = (page?.items || []).filter((item) => {
    const copy = heldByKey.get(threadItemKey(item));
    return copy && latestItemSequence(item) > latestItemSequence(copy);
  });
  return newer.length ? { ...held, items: mergeThreadItems(held.items, newer) } : null;
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
 * With no window held, the page IS the window: it is the latest hundred items,
 * and `has_more` on it means the conversation reaches back further than the
 * window does. With a window held, an abutting page is appended. A newest-mode
 * page whose `has_more` says it skipped a gap replaces the stale window with
 * the returned tip, leaving that gap behind the ordinary load-older path.
 */
export function threadWindow(held, page, { newest = false } = {}) {
  const arrived = windowFromThreadPayload(page);
  if (!held) return arrived;
  if (!arrived) return null; // nothing new: the record stands
  if (newest && page.has_more === true) return newestThreadWindow(held, arrived, page);
  return appendedThreadWindow(held, arrived, page);
}

const appendedThreadWindow = (held, arrived, page) => ({
  // A record of stand-ins alone is not a window to append to: the page is
  // the window, and only the reader's own messages carry over into it.
  ...(holdsAWindow(held) ? held : arrived),
  // Merged rather than appended: the record may hold this tab's own message
  // waiting for the wire to carry it, and the arrival takes that stand-in away.
  items: mergeThreadItems(held.items, arrived.items),
  deliveredSequence: Math.max(Number(held.deliveredSequence || 0), arrived.deliveredSequence),
  knownTotalItems: arrived.knownTotalItems ?? held.knownTotalItems ?? null,
  activityDigests: mergeActivityDigests(held.activityDigests || [], page),
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
    items,
    deliveredSequence: Math.max(Number(held.deliveredSequence || 0), arrived.deliveredSequence),
    knownTotalItems: Math.max(Number(held.knownTotalItems || 0), Number(arrived.knownTotalItems || 0)) || null,
  };
};

const highestCreationSequence = (items) =>
  (items || []).reduce((highest, item) => Math.max(highest, Number(item?.data?.sequence || 0)), 0);
