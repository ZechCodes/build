// A project's issue list, pulled a page at a time from a bridge that pages it
// (#85, announced as `issues.listPaged`).
//
// The bridge answers in number-descending order, and each page's cursor goes
// on below where that page stopped reading. Numbers never move, so each page
// is the whole truth about one stretch of numbers: from where the page before
// it stopped down to its own last row (the first page reaches up without
// bound, the final one down). A pull lays each page over the held list for
// exactly that stretch, so the Issues tab fills in as the pages land rather
// than after the last of them, and a row outside the stretch is never touched.
//
// Render from cache: a page is written under its own address — one per
// filter, cursor, limit and read — the list is folded from what is read back from
// there, and the views repaint from the list record's announcement. Nothing
// paints from the wire.
//
// A page can be older than what the cache already holds: a push re-read, the
// Issues tab's own read or another tab's may lay a row, or take one away,
// while a page is out. So a page yields every row a read asked after it has
// had the say on, present or absent (core/issueReadOrder.js) — the newer word
// wins the way `pushFence` has it win for whole records (#142). A writer that
// is not a read of pages — a whole list from an older bridge, an issue filed
// here — is not in that order, so for what it wrote the timestamps stand in: a
// held row written after the page's copy of it keeps its place, and so does a
// row the page does not name that was written after the page was read.

import { cachedSubKeys, deleteCached, mergeCachedAtomically, mergeCachedTogether, readCached } from "./localCache.js";
import { lastSayIn, nextIssueRead, readsAddress, withStretch } from "./issueReadOrder.js";
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

/** What a page lays: the rows it names that no newer read has had the say on
 *  and that are not older than the held copy. */
function rowsToLay(issues, heldById, overtaken) {
  return issues.filter((row) => {
    const heldRow = heldById.get(row.id);
    return !overtaken(row) && !(heldRow && writtenAfter(heldRow, row));
  });
}

/** The span of a stretch a number falls in, or undefined outside it. A
 *  stretch without spans is one span: all of it read by the page itself. */
function spanAt(stretch, number) {
  const { above = Infinity, through = -Infinity, read = 0, readAt = null } = stretch;
  const spans = stretch.spans || [{ above, through, read, readAt }];
  return spans.find((span) => number < span.above && number >= span.through);
}

/**
 * The held list with one page laid over the stretch of numbers it answers for:
 * `through` (inclusive) up to `above` (exclusive). Pure. Each span of it was
 * read as read `read` at `readAt`, and `lastSay(row)` names the newest read
 * that had the say on a row: where that is newer than the span's, the held
 * list stands, the row present or absent.
 */
export function withIssuePage(held, stretch, lastSay = () => 0) {
  const { issues = [], read = 0 } = stretch;
  const heldRows = held || [];
  const spanOf = (row) => spanAt(stretch, Number(row.number));
  const overtaken = (row) => lastSay(row) > (spanOf(row)?.read ?? read);
  const laid = rowsToLay(issues, new Map(heldRows.map((row) => [row.id, row])), overtaken);
  const laidIds = new Set(laid.map((row) => row.id));
  const namedIds = new Set(issues.map((row) => row.id));
  const keeps = (row) => {
    const span = spanOf(row);
    return !span || namedIds.has(row.id) || overtaken(row) || writtenAfterRead(row, span.readAt);
  };
  const kept = heldRows.filter((row) => !laidIds.has(row.id) && keeps(row));
  return sortIssues([...kept, ...laid]);
}

/** Lay one page over the list record at `address`, and note beside it that
 *  the page had the say on its stretch there — the two in one transaction, so
 *  a write landing between the read and the write is not lost, and another
 *  tab never reads the list without the note or the note without the list. */
export const foldIssuesPage = (address, stretch, columnsOf) =>
  mergeCachedTogether([address, readsAddress(address)], ([held, reads]) => [
    issuesRecord(withIssuePage(held?.issues, stretch, lastSayIn(reads)), columnsOf(held)),
    withStretch(reads, stretch),
  ]);

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

const numbersOf = (issues) => issues.map((row) => Number(row.number));

/** The numbers a page answers for, and which read had the say on each.
 *
 *  A page answers down to its last row — the last page, everything below —
 *  and up to where the page before it left off. But the cursor is opaque, and
 *  a page need not end where its rows do: under a label or an assignee the
 *  bridge reads a bounded stretch past the last row it keeps, and a page can
 *  keep none. So the numbers between the last row the walk laid and this
 *  page's first were read by some page since that row's, and no later than
 *  by the oldest of those (`since`): that is the read that has the say on
 *  them. A read asked in between may have seen a row there that this page
 *  never read. From its own first row down, the page read every number
 *  itself. A page with a next and no rows answers for nothing yet: the page
 *  after it answers for the numbers it read past. */
function stretchOf(page, place, own, next) {
  const issues = Array.isArray(page.issues) ? page.issues : [];
  const named = numbersOf(issues);
  const through = next ? (named.length ? Math.min(...named) : place.above) : -Infinity;
  const top = named.length ? Math.min(Math.max(...named) + 1, place.above) : through;
  const spans = [
    { above: place.above, through: top, ...(place.since || own) },
    { above: top, through, ...own },
  ].filter((span) => span.through < span.above);
  return { issues, above: place.above, through, spans, ...own };
}

/** Whether a stretch answers for any number, and so has anything to lay. */
const coversNumbers = (stretch) => stretch.through < stretch.above;

/** Keep the body and the read that produced it together. Even simultaneous
 * reads of one cursor have their own addresses, so one can never borrow the
 * other's body or cursor. */
async function landPage(deviceId, projectId, asked, answer, read) {
  const address = issuesPageAddress(deviceId, projectId, { ...asked, read_order: read });
  await mergeCachedAtomically(address, (held) =>
    Number(held?.read_order) > read ? null : { ...answer, read_order: read });
  return { sub: address.sub, page: (await readCached(address))?.value };
}

/** A page's filter and originating read, from its sub-key. An old page with
 * no read token is eligible for cleanup when a current walk completes. */
function pageIdentity(sub) {
  try {
    const filter = { ...JSON.parse(sub) };
    const read = Number.isSafeInteger(filter.read_order) ? filter.read_order : 0;
    delete filter.cursor;
    delete filter.limit;
    delete filter.read_order;
    return { filter: JSON.stringify(filter), read };
  } catch {
    return null;
  }
}

/** Drop only pages from older reads of this filter that this walk did not
 * land. A newer walk may have written a page but still await its readback. */
async function forgetOtherPages(deviceId, projectId, params, landed, pullRead) {
  const filter = JSON.stringify(params);
  const subs = await cachedSubKeys(deviceId, projectId, TRACKER_ISSUES_PAGE_KIND);
  const stale = subs.filter((sub) => {
    const page = pageIdentity(sub);
    return !landed.has(sub) && page?.filter === filter && page.read < pullRead;
  });
  if (stale.length) await deleteCached(stale.map((sub) => ({ deviceId, entityId: projectId, kind: TRACKER_ISSUES_PAGE_KIND, sub })));
}

/** Ask for, land and fold the page at `place`, and answer where the next
 *  starts (no cursor once this was the last), or null when the pull stops.
 *  Each page is a read of its own, numbered as it is asked; `pullRead` is the
 *  pull's first, and `since` the read that has the say on the numbers below
 *  the last row laid, until a page lays a row under them. */
async function pullPage({ ask, deviceId, projectId, params, fold, active, limit }, place) {
  const asked = pageParams(params, place.cursor, limit);
  const read = await nextIssueRead();
  const answer = await ask(asked);
  if (!answer || !active()) return null;
  const { sub, page } = await landPage(deviceId, projectId, asked, answer, read);
  if (!Number.isSafeInteger(page?.read_order) || page.read_order !== read || !active()) return null;
  return foldLandedPage({ sub, page, place, read, fold });
}

/** This read's cached body owns both the fold and the cursor walk. */
async function foldLandedPage({ sub, page, place, read, fold }) {
  const next = nextCursorOf(page, place.cursor);
  const own = { read: page.read_order, readAt: userSessionOf(page)?.now_ms ?? null };
  const stretch = { ...stretchOf(page, place, own, next), pullRead: place.pullRead ?? read };
  // An exhausted or refused canonical write did not lay this stretch. Keep
  // its cursor out of the walk so a later page cannot conceal the gap.
  if (coversNumbers(stretch) && await fold(stretch, page) === false) return null;
  return {
    sub,
    cursor: next,
    above: stretch.through,
    pullRead: stretch.pullRead,
    since: sinceAfter(stretch, place, own),
  };
}

/** The read that has the say below a page: the page's own once it has laid
 *  a row, and otherwise still the oldest one that read past the last row. */
const sinceAfter = (stretch, place, own) => (stretch.issues.length ? own : place.since || own);

/**
 * Pull one list page by page. `ask(params)` answers a page or null; each page
 * is written under its own address, read back, and handed to
 * `fold(stretch, page)` before the next is asked. Answers whether the pull
 * reached the last page. A caller that stops being `active` stops it.
 */
export async function pullIssuePages(pull) {
  const walk = { active: () => true, limit: ISSUE_PAGE_LIMIT, ...pull };
  const landed = new Set();
  let place = { cursor: null, above: Infinity, pullRead: null, since: null };
  do {
    place = await pullPage(walk, place);
    if (!place) return false;
    landed.add(place.sub);
  } while (place.cursor && walk.active());
  if (!walk.active()) return false;
  await forgetOtherPages(walk.deviceId, walk.projectId, walk.params, landed, place.pullRead);
  return true;
}
