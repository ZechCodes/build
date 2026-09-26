// An issue's unread, counted the way an agent's is (#104).
//
// Zech: "Issues should be treated as agents when computing unread counts." And:
// "When not watched no unread count should ever be shown." So only a watched
// issue has one. What it counts is #99's unread line — every timeline entry
// after the user's read mark that is not their own (core/trackerUnread.js) —
// read off the cached records: the issue's cached timeline while it is at least
// as new as the list's row, which is where a read this tab just made shows
// first, and otherwise the `unread_count` the bridge puts on each watched row
// of `issues.list` (wire 1.29.0), by the same rule.
//
// Where the count is worn:
//   • the issue's own bubble, on the board, the list and the dashboard;
//   • the Issues tab: the project's carries every watched issue's, a
//     workspace's only those its agents hold;
//   • the rail: an issue an agent holds counts on that agent's workspace row,
//     and every other one — nobody's, the user's, the project agent's, or an
//     agent's whose workspace has no row to wear it — on the project's own.
//
// No DOM, no app imports.

import { changedSince } from "./trackerModel.js";
import { timelineRows } from "./trackerTimeline.js";
import { issueUnreadReading, latestIssueMark } from "./trackerUnread.js";

const listedCount = (issue) => (Number.isSafeInteger(issue?.unread_count) ? issue.unread_count : null);

/** #99's count over a cached timeline, from the newer of the two read marks. */
function timelineUnread(issue, detail) {
  if (!Array.isArray(detail?.timeline)) return 0;
  const mark = latestIssueMark(detail.issue?.read_through, issue?.read_through);
  return issueUnreadReading(timelineRows(detail.timeline), mark).unreadCount;
}

/** How much of one issue is unread: `issue` as the cached list holds it,
 *  `detail` its cached `issues.get` record when there is one. */
export function issueUnreadCount(issue, detail = null) {
  if (issue?.watched !== true) return 0;
  const listed = listedCount(issue);
  const timelineIsCurrent = Array.isArray(detail?.timeline) && !changedSince(issue, detail);
  return timelineIsCurrent || listed === null ? timelineUnread(issue, detail) : listed;
}

/** The bubble an issue wears where issues are listed, and nothing at all for
 *  none. The inbox's own badge, so an unread count reads the same anywhere. */
export const unreadBubbleHtml = (count) =>
  (count > 0 ? `<span class="badge inbox-unread issue-unread" title="${count} unread">${count}</span>` : "");

/** The unread over a list of issues, as an Issues tab wears it: `detailOf`
 *  finds an issue's cached timeline, `only` keeps the ones the tab is about. */
export function watchedIssuesUnread(issues = [], { detailOf = () => null, only = () => true } = {}) {
  return issues.reduce((total, issue) => (only(issue) ? total + issueUnreadCount(issue, detailOf(issue)) : total), 0);
}

/** The agent holding an issue, when an agent does. */
const holderOf = (issue) => (issue?.assignee?.kind === "agent" ? issue.assignee.agent_id || null : null);

const sumOf = (held) => held.reduce((total, one) => total + one.count, 0);

/**
 * The rail's reading of every followed project's watched issues:
 * `sources` is `[{ project, issues, details }]`, as core/watchedIssueFollower.js
 * holds them.
 *
 * `heldBy(projectKey, agentIds)` is what one workspace row wears: the issues
 * its agents hold. `unheldBy(rows)` answers, per project key, what the
 * project's own badge wears: every issue none of those workspace rows' agents
 * hold, `rows` being `[{ projectKey, agentIds }]`.
 */
export function issueUnreadTally(sources = []) {
  const byProject = new Map();
  for (const { project, issues = [], details = new Map() } of sources) {
    const held = issues
      .map((issue) => ({ holder: holderOf(issue), count: issueUnreadCount(issue, details.get(issue.id) || null) }))
      .filter((one) => one.count > 0);
    byProject.set(project.projectKey, held);
  }
  const heldIn = (projectKey) => byProject.get(projectKey) || [];
  return {
    heldBy(projectKey, agentIds = []) {
      const ids = new Set(agentIds);
      return sumOf(heldIn(projectKey).filter((one) => ids.has(one.holder)));
    },
    unheldBy(rows = []) {
      return (projectKey) => {
        const ids = new Set(rows.filter((row) => row.projectKey === projectKey).flatMap((row) => row.agentIds || []));
        return sumOf(heldIn(projectKey).filter((one) => !ids.has(one.holder)));
      };
    },
  };
}

/** The reading of a rail that follows no issues. */
export const NO_ISSUE_UNREAD = issueUnreadTally();
