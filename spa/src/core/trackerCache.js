// Where the tracker's records live in the local cache.
//
// An issue belongs to a project, and a project is named by the machine it is on
// — both machines mint a `proj-1` — so every record here is addressed by
// (device, project). That is also what makes eviction one act: when a project
// stops being listed on a device, the cache's single range delete over
// (device, project) takes its issue list and every issue page with it.
//
// Two records, both whole:
//
//   `tracker-issues`            the project's whole list, plus its columns
//   `tracker-issue` / <id>      one issue and its timeline, as `issues.get`
//                               answered them
//
// The list is written by the sync layer on every pass (core/trackerSync.js) and
// is what the Issues tab paints before the bridge has answered. The per-issue
// record is written through by the issue page itself on first open, which is
// how every other detail surface warms its own cache.

import { readCached, writeCached } from "./localCache.js";

export const TRACKER_ISSUES_KIND = "tracker-issues";
export const TRACKER_ISSUE_KIND = "tracker-issue";

/** The project's list. */
export const issuesAddress = (deviceId, projectId) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_ISSUES_KIND,
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
 *  holding them costs a field and saves a round trip on every cold open. */
export const issuesRecord = (issues, columns) => ({
  issues: issues || [],
  columns: columns || [],
});

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

export const writeIssuesRecord = (deviceId, projectId, record) =>
  writeCached(issuesAddress(deviceId, projectId), record);

export const writeIssueRecord = (deviceId, projectId, issueId, record) =>
  writeCached(issueAddress(deviceId, projectId, issueId), record);
