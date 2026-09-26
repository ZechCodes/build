// Where the tracker's records live in the local cache.
//
// An issue belongs to a project, and a project is named by the machine it is on
// — both machines mint a `proj-1` — so every record here is addressed by
// (device, project). That is also what makes eviction one act: when a project
// stops being listed on a device, the cache's single range delete over
// (device, project) takes its issue list and every issue page with it.
//
// The records:
//
//   `tracker-issues`            the project's whole list, plus its columns
//   `tracker-issues-query`      one narrowed list, by its params
//   `tracker-issues-page`       one page of a paged list, by its params (#85)
//   `tracker-issue` / <id>      one issue and its timeline, as `issues.get`
//                               answered them
//
// The list is written by the sync layer on every pass (core/trackerSync.js) and
// is what the Issues tab paints before the bridge has answered. The per-issue
// record is written through by the issue page itself on first open, which is
// how every other detail surface warms its own cache.

import { mergeCachedAtomically, readCached, writeCached } from "./localCache.js";
import { issueUnreadKey, latestIssueMark } from "./trackerUnread.js";

export const TRACKER_ISSUES_KIND = "tracker-issues";
export const TRACKER_ISSUES_QUERY_KIND = "tracker-issues-query";
export const TRACKER_ISSUE_KIND = "tracker-issue";
export const TRACKER_ISSUES_PAGE_KIND = "tracker-issues-page";

/** The project's list. */
export const issuesAddress = (deviceId, projectId) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_ISSUES_KIND,
});

/** One narrowed list answer. The whole-list record above is the project's
 *  durable catalog; a filter response must not replace it or the menus would
 *  forget every value the current filter hid. Query params are assembled in
 *  a stable order by trackerFilters, so their JSON spelling is a stable cache
 *  key too. */
export const issuesQueryAddress = (deviceId, projectId, params) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_ISSUES_QUERY_KIND,
  sub: JSON.stringify(params || {}),
});

/** One page of a paged `issues.list` answer (#85), under the params that asked
 *  for it — its filter, cursor and limit — so every combination is a record of
 *  its own. A pull folds the list from what it reads back from here
 *  (core/trackerPages.js). */
export const issuesPageAddress = (deviceId, projectId, params) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_ISSUES_PAGE_KIND,
  sub: JSON.stringify(params || {}),
});

/** One issue and its timeline, under the project it belongs to. */
export const issueAddress = (deviceId, projectId, issueId) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_ISSUE_KIND,
  sub: String(issueId || ""),
});

/** The list record. `columns` rides beside the issues because the two are read
 *  together on every paint — a board cannot be drawn from one without the
 *  other — and because the columns change far more rarely than the issues, so
 *  holding them costs a field and saves a round trip on every cold open.
 *
 *  `readOrder` is the number the read that answered it took before it asked
 *  (core/issueReadOrder.js): when the bridge was asked, not when the answer
 *  reached the browser (#129). A list written here rather than read — a card
 *  moved, an issue filed — takes its number from the same count as it is
 *  written, so every list is ordered against every other on one scale. */
export const issuesRecord = (issues, columns, readOrder) => ({
  issues: issues || [],
  columns: columns || [],
  ...(Number.isFinite(readOrder) ? { read_order: readOrder } : {}),
});

/** When a cached list (`{ at, value }`) was asked for or written: its number.
 *  Only a list written before #129 has none, and stands in with the cache's
 *  own stamp on it — the clock the count is floored at, the nearest the two
 *  scales come to meeting. */
export const listAskedAt = (cached) => Number(cached?.value?.read_order) || cached?.at || 0;

export const issueRecord = (issue, timeline) => ({ issue: issue || null, timeline: timeline || [] });

/** What the cache holds for a project, or null when it has never been read
 *  there. Never throws: a cache miss and a broken cache are the same answer. */
export async function readIssuesRecord(deviceId, projectId) {
  const record = await readCached(issuesAddress(deviceId, projectId));
  return record?.value || null;
}

export async function readIssueRecord(deviceId, projectId, issueId) {
  const record = await readCached(issueAddress(deviceId, projectId, issueId));
  return record?.value || null;
}

export async function readIssuesQueryRecord(deviceId, projectId, params) {
  const record = await readCached(issuesQueryAddress(deviceId, projectId, params));
  return record?.value || null;
}

/** A list record and when the cache took it, `{ at, value }`, or null when it
 *  holds none: one read, so a paint never takes the list from one record and
 *  its stamp from another, or reads a stamp of 0 for a record that went
 *  between the two (#129). */
async function readListCached(address) {
  const record = await readCached(address);
  return record?.value ? { at: record.at || 0, value: record.value } : null;
}

export const readIssuesCached = (deviceId, projectId) => readListCached(issuesAddress(deviceId, projectId));

export const readIssuesQueryCached = (deviceId, projectId, params) =>
  readListCached(issuesQueryAddress(deviceId, projectId, params));

export const writeIssuesRecord = (deviceId, projectId, record) =>
  writeCached(issuesAddress(deviceId, projectId), record);

/** Pulls may carry an older server read mark than one this browser has already
 * accepted. Keep the accepted mark in the cached issue while replacing its
 * other fields and timeline with the newest pull. The transaction makes two
 * tabs' writes obey the same floor. */
export const writeIssueRecord = (deviceId, projectId, issueId, record) =>
  mergeCachedAtomically(issueAddress(deviceId, projectId, issueId), (held) => {
    const floor = latestIssueMark(held?.issue?.read_through, record?.issue?.read_through);
    if (!record?.issue || !floor || floor === record.issue.read_through) return record;
    return { ...record, issue: { ...record.issue, read_through: floor } };
  });

/** Only an accepted read report may raise the cached floor. Do not touch the
 * timeline, and never accept a synthetic key as a read mark. */
export const advanceIssueReadThrough = (deviceId, projectId, issueId, mark) => {
  if (issueUnreadKey(mark) === null) return Promise.resolve(false);
  return mergeCachedAtomically(issueAddress(deviceId, projectId, issueId), (held) => {
    if (!held?.issue) return null;
    const floor = latestIssueMark(held.issue.read_through, mark);
    if (floor === held.issue.read_through) return null;
    return { ...held, issue: { ...held.issue, read_through: floor } };
  });
};

export const writeIssuesQueryRecord = (deviceId, projectId, params, record) =>
  writeCached(issuesQueryAddress(deviceId, projectId, params), record);

/** When the cache last took an answer for a record, or 0 for one it has never
 *  held. A surface showing a cached copy says when that copy was read
 *  (core/transientRead.js), and this is when — the moment the answer landed,
 *  not the moment the surface got round to painting it. */
export const issuesRecordAt = async (deviceId, projectId) =>
  (await readCached(issuesAddress(deviceId, projectId)))?.at || 0;

export const issueRecordAt = async (deviceId, projectId, issueId) =>
  (await readCached(issueAddress(deviceId, projectId, issueId)))?.at || 0;
