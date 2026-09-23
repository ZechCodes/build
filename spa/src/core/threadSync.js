// The one read-and-merge path for a conversation window.
//
// The ordered cache pass and a surface opening a cold conversation both come
// through here. That keeps the latest-window request, gap handling and cache
// write identical; the surface only gives a cold read foreground priority.

import { LATEST_THREAD_ITEMS } from "./cacheThresholds.js";
import { mergeCached, readCached } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";
import {
  THREAD_RECORD_KIND,
  isProvisionalItem,
  mergeThreadItems,
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

async function requestThreadPage(request, after) {
  try {
    return await request.call(
      "thread.page",
      threadPageParams(request.entityId, request.agentId || "", after),
      requestPriorityFields(requestPriority(request)),
    );
  } catch {
    return null;
  }
}

/** One conversation, read forward from the sequence the cache holds — or the
 *  latest hundred where it holds none. */
export async function syncThreadWindow(request) {
  if (!validThreadRequest(request)) return false;
  const target = threadTarget(request);
  const held = (await readCached(target))?.value;
  const after = Number(held?.deliveredSequence || 0);
  const page = await requestThreadPage(request, after);
  if (!page) return false;
  if (!requestIsActive(request)) return false;
  await mergeCached(target, (current) => threadWindow(current, page, { newest: after > 0 }));
  return true;
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
  ...(Array.isArray(page.activity_spans) ? { activity_spans: page.activity_spans } : {}),
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
    ...(Array.isArray(page.activity_spans) ? { activity_spans: page.activity_spans } : {}),
  };
};

const highestCreationSequence = (items) =>
  (items || []).reduce((highest, item) => Math.max(highest, Number(item?.data?.sequence || 0)), 0);

const latestItemSequence = (item) =>
  Math.max(Number(item?.data?.sequence || 0), Number(item?.data?.updated_sequence || 0));
