// Timeline windows retain only history connected to the current head. An
// explicitly sought comment has its own bounded section until paging joins it.
import { changedSince } from "./trackerModel.js";
import { latestTaskMark, taskUnreadKey } from "./trackerUnread.js";

const fields = ["timeline", "has_more", "next_cursor", "timeline_read_through", "timeline_incarnation", "timeline_identities", "timeline_anchor_pages", "timeline_legacy_complete"];
export const taskTimelineFields = (record) => Object.fromEntries(fields
  .filter((field) => Object.hasOwn(record || {}, field)).map((field) => [field, record[field]]));
export const timelineNeedsCounters = (detail) => detail?.stale === true || detail?.has_more === true
  || !Array.isArray(detail?.timeline);
const sameGeneration = (held, incoming) => !held?.timeline_incarnation || !incoming?.timeline_incarnation
  || held.timeline_incarnation === incoming.timeline_incarnation;
const rowKey = (row) => `${row?.type}:${row?.id}`;
const historyOffset = (held, row) => row?.id ? (held?.timeline || []).findIndex((entry) => rowKey(entry) === rowKey(row)) : -1;
const canPrependHeld = (held, incoming) => held.has_more !== true || (Object.hasOwn(held, "next_cursor")
  && held.timeline_incarnation === incoming.timeline_incarnation);

function prependHeldPrefix(next, held, offset) {
  // Older scoped heads have no continuation. Only a complete legacy history
  // or a paged window from this incarnation can extend the arriving page.
  if (!canPrependHeld(held, next)) return;
  next.timeline = [...held.timeline.slice(0, offset), ...next.timeline];
  next.has_more = held.has_more === true;
  if (held.timeline_legacy_complete && !Object.hasOwn(next, "next_cursor")) next.timeline_legacy_complete = true;
  if (Object.hasOwn(held, "next_cursor") || Object.hasOwn(next, "next_cursor")) next.next_cursor = held.next_cursor || null;
  next.timeline_identities = { ...held.task?.identities, ...held.timeline_identities, ...next.task?.identities, ...next.timeline_identities };
}

export function attachTaskTimelineAnchors(next, anchors) {
  delete next.timeline_anchor_pages;
  if (!anchors || next.has_more === false) return next;
  const pages = Object.values(anchors);
  // There are at most four sections. Joining one can expose another overlap.
  for (let pass = 0; pass < pages.length; pass += 1) {
    for (const page of pages) {
      const offset = historyOffset(page, next.timeline[0]);
      if (offset >= 0) prependHeldPrefix(next, page, offset);
    }
  }
  const pending = Object.entries(anchors).filter(([anchor]) => !next.timeline.some((row) => row.id === anchor));
  if (pending.length && next.has_more) next.timeline_anchor_pages = Object.fromEntries(pending);
  return next;
}

export function mergeTaskTimeline(held, incoming) {
  const next = { ...incoming, timeline: incoming.timeline || [] };
  if (incoming.has_more !== true || !sameGeneration(held, incoming)) return next;
  const offset = historyOffset(held, next.timeline[0]);
  if (offset >= 0) prependHeldPrefix(next, held, offset);
  return attachTaskTimelineAnchors(next, held?.timeline_anchor_pages);
}

/** Counters come from complete server facts, never the size of a head page. */
export function timelineCounterTask(task, detail) {
  const held = detail?.task;
  if (!held || detail.stale === true || changedSince(task, detail)) return task;
  const newest = latestTaskMark(task?.read_through, held.read_through);
  return newest && newest === task?.read_through && newest !== held.read_through ? task : held;
}

function timelineReadFloor(task, detail) {
  const read = latestTaskMark(task?.read_through, detail?.task?.read_through);
  const baseline = latestTaskMark(task?.watch_started_after, detail?.task?.watch_started_after);
  return taskUnreadKey(latestTaskMark(read, baseline));
}

export function timelineWasRead(task, detail) {
  if (detail?.stale === true || changedSince(task, detail)) return false;
  const head = taskUnreadKey(detail?.timeline_read_through);
  const floor = timelineReadFloor(task, detail);
  return head !== null && floor !== null && floor >= head;
}

export function partialTimelineCount(task, detail, field) {
  if (timelineWasRead(task, detail)) return 0;
  const value = timelineCounterTask(task, detail)?.[field];
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}
