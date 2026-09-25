// A project's issue list, pulled a page at a time from a bridge that pages it
// (#85, announced as `issues.listPaged`).
//
// The bridge answers in number-descending order, and a page's cursor is the
// last number it answered. Numbers never move, so each page is the whole truth
// about one stretch of numbers: from the page before it's last down to its own
// last (the first page reaches up without bound, the final one down). A pull
// lays each page over the held list for exactly that stretch, so the Issues tab
// fills in as the pages land rather than after the last of them, and a row
// outside the stretch is never touched.
//
// Render from cache: a page is written under its own address — one per
// filter, cursor and limit — the list is folded from what is read back from
// there, and the views repaint from the list record's announcement. Nothing
// paints from the wire.
//
// A page can be older than what the cache already holds: a push re-read, the
// Issues tab's own read or another tab may write a row while a page is out.
// So a held row written after the page's copy of it keeps its place, and so
// does a row the page does not name that was written after the page was read.
// The newer word wins the way `pushFence` has it win for whole records (#142).

import { cachedSubKeys, deleteCached, mergeCachedAtomically, readCached, writeCached } from "./localCache.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { sortIssues } from "./trackerFilters.js";
import { TRACKER_ISSUES_PAGE_KIND, issuesPageAddress, issuesRecord } from "./trackerCache.js";
import { userSessionOf } from "./userSessionCache.js";

/** Issues per page. A tenth of a large tracker, and one page of a small one. */
export const ISSUE_PAGE_LIMIT = 100;

/** Whether this device's bridge answers `issues.list` a page at a time.
 *  Defensive like every capability read: a shape this build does not expect
 *  costs the paging and nothing else, and the whole-list read stands in. */
export const pagesIssues = (deviceId) => bridgeCapabilities(deviceId)?.issues?.listPaged === true;

const instant = (text) => Date.parse(text || "");
const writtenAfter = (held, answered) => instant(held.updated_at) > instant(answered.updated_at);
const writtenAfterRead = (held, readAt) => Number.isFinite(readAt) && instant(held.updated_at) > readAt;

/**
 * The held list with one page laid over the stretch of numbers it answers for:
 * `through` (inclusive) up to `above` (exclusive). Pure.
 */
export function withIssuePage(held, { issues = [], above = Infinity, through = -Infinity, readAt = null }) {
  const heldById = new Map((held || []).map((row) => [row.id, row]));
  const answered = issues.map((row) => {
    const heldRow = heldById.get(row.id);
    return heldRow && writtenAfter(heldRow, row) ? heldRow : row;
  });
  const answeredIds = new Set(answered.map((row) => row.id));
  const covered = (row) => Number(row.number) < above && Number(row.number) >= through;
  const kept = (held || []).filter((row) =>
    !answeredIds.has(row.id) && (!covered(row) || writtenAfterRead(row, readAt)));
  return sortIssues([...kept, ...answered]);
}

/** Lay one page over the list record at `address`, in one transaction, so a
 *  write landing between the read and the write is not lost. */
export const foldIssuesPage = (address, stretch, columnsOf) =>
  mergeCachedAtomically(address, (held) => issuesRecord(withIssuePage(held?.issues, stretch), columnsOf(held)));

const pageParams = (params, cursor, limit) => (cursor ? { ...params, limit, cursor } : { ...params, limit });

/** The cursor to ask with next, or null when this page was the last. A page
 *  can be short, or empty, and still name the next: under a label or an
 *  assignee the bridge reads a bounded stretch per page and answers what it
 *  kept there. A cursor that does not move ends the pull rather than asking
 *  for the same page forever. */
function nextCursorOf(page, cursor) {
  const next = page?.next_cursor;
  if (typeof next !== "string" || !next || next === cursor) return null;
  return next;
}

/** The numbers a page answers for: down to its last row, or, the last page,
 *  everything below. A page with a next and no rows answers for none — the
 *  cursor is opaque, so the page after it starts where the last row kept
 *  left off, and answers for the numbers this one read past. */
function stretchOf(page, above, next) {
  const issues = Array.isArray(page.issues) ? page.issues : [];
  const last = issues.at(-1);
  return {
    issues,
    above,
    through: next ? (last ? Number(last.number) : above) : -Infinity,
    readAt: userSessionOf(page)?.now_ms ?? null,
  };
}

/** Whether a stretch answers for any number, and so has anything to lay. */
const coversNumbers = (stretch) => stretch.through < stretch.above;

/** Write one page where it lives and answer what the cache now holds there. */
async function landPage(deviceId, projectId, asked, answer) {
  const address = issuesPageAddress(deviceId, projectId, asked);
  await writeCached(address, answer);
  return { sub: address.sub, page: (await readCached(address))?.value };
}

/** A list's filter, from one of its pages' sub-keys: the page's params less
 *  where it started and how long it was. */
function filterOfPage(sub) {
  try {
    const filter = { ...JSON.parse(sub) };
    delete filter.cursor;
    delete filter.limit;
    return JSON.stringify(filter);
  } catch {
    return null;
  }
}

/** Drop the pages an earlier pull of this same list left under cursors this
 *  one did not reach. Cursors move as issues are filed, so without this each
 *  pull would leave its pages behind for good. Other filters' pages stay. */
async function forgetOtherPages(deviceId, projectId, params, landed) {
  const filter = JSON.stringify(params);
  const subs = await cachedSubKeys(deviceId, projectId, TRACKER_ISSUES_PAGE_KIND);
  const stale = subs.filter((sub) => !landed.has(sub) && filterOfPage(sub) === filter);
  if (stale.length) await deleteCached(stale.map((sub) => ({ deviceId, entityId: projectId, kind: TRACKER_ISSUES_PAGE_KIND, sub })));
}

/** Ask for, land and fold the page at `place`, and answer where the next
 *  starts (no cursor once this was the last), or null when the pull stops. */
async function pullPage({ ask, deviceId, projectId, params, fold, active, limit }, { cursor, above }) {
  const asked = pageParams(params, cursor, limit);
  const answer = await ask(asked);
  if (!answer || !active()) return null;
  const { sub, page } = await landPage(deviceId, projectId, asked, answer);
  if (!page || !active()) return null;
  const next = nextCursorOf(page, cursor);
  const stretch = stretchOf(page, above, next);
  if (coversNumbers(stretch)) await fold(stretch, page);
  return { sub, cursor: next, above: stretch.through };
}

/**
 * Pull one list page by page. `ask(params)` answers a page or null; each page
 * is written under its own address, read back, and handed to
 * `fold(stretch, page)` before the next is asked. Answers whether the pull
 * reached the last page. A caller that stops being `active` stops it.
 */
export async function pullIssuePages(pull) {
  const walk = { active: () => true, limit: ISSUE_PAGE_LIMIT, ...pull };
  const landed = new Set();
  let place = { cursor: null, above: Infinity };
  do {
    place = await pullPage(walk, place);
    if (!place) return false;
    landed.add(place.sub);
  } while (place.cursor && walk.active());
  if (!walk.active()) return false;
  await forgetOtherPages(walk.deviceId, walk.projectId, walk.params, landed);
  return true;
}
