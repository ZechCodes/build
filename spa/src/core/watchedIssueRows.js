// Watched issues as inbox rows (#125), as a pure model.
//
// A watched issue is a row only while there is something in it for the user:
// it is assigned to them, or an agent asked them in an unread comment or
// when filing the issue — the Issues tab's "Needs you" rules
// (core/trackerAttentionModel.js, which also keeps the earlier rule for a
// bridge that cannot say which comments asked). When the reason goes, so does the row, and
// nothing that is Done or closed is ever one.
//
// Read from the cached issue records, never the board feed's `tracker_issue`
// rows: an `issues` push re-reads the project's list and nothing re-reads the
// board for it, so the list is the record that moves when the issue does.
//
// No DOM, no app imports — the wiring (core/watchedIssueFollower.js) reads the
// cache and core/inboxView.js paints these beside the workspace rows.

import { TRACKER_ISSUE, entryKeyOf } from "./inbox.js";
import { ATTENTION_REASONS, unreadAsks, watchedIssueReasons } from "./trackerAttentionModel.js";

/** Why the row is there, in the inbox's words. */
const REASON_WORDS = Object.freeze({
  [ATTENTION_REASONS.inReview]: "In review",
  [ATTENTION_REASONS.assigned]: "Assigned to you",
});

const reasonWord = (reason, askedOnly) => reason === ATTENTION_REASONS.inbox
  ? (askedOnly ? "Mentioned you" : "New comment") : REASON_WORDS[reason];

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

const titleOf = (issue) => issue.title || "(untitled)";

/** The fields a row carries about a checkout, which an issue has none of. */
const NOT_A_CHECKOUT = Object.freeze({
  branch: null,
  working: false,
  pending: null,
  placeholder: false,
  canFinish: false,
  merged: false,
  muted: false,
  dismissed: false,
  warnings: [],
});

function toEntry(project, issue, detail, reasons, askedOnly) {
  const facts = reasons.map((reason) => reasonWord(reason, askedOnly)).join(" · ");
  const changedMs = ms(issue.updated_at);
  return {
    ...NOT_A_CHECKOUT,
    key: entryKeyOf({ kind: TRACKER_ISSUE, issue_id: issue.id }),
    kind: TRACKER_ISSUE,
    // An issue is not an entity the board's read and mute verbs know; its own
    // verbs are `issues.*`, named by the issue id.
    entityId: null,
    issueId: issue.id,
    number: issue.number ?? null,
    deviceId: project.deviceId,
    projectId: project.id,
    projectKey: project.projectKey,
    project: project.name || project.id,
    name: issue.number ? `#${issue.number} ${titleOf(issue)}` : titleOf(issue),
    title: titleOf(issue),
    // Every row here is asking for the user; that is why it is a row.
    state: "unread",
    reason: facts,
    facts,
    unreadCount: unreadAsks(issue, detail, askedOnly).length,
    route: { name: "trackerIssue", deviceId: project.deviceId, projectId: project.id, issueId: issue.id },
    anchorMs: changedMs,
    lastActivityMs: changedMs,
  };
}

/** Oldest change first, as every inbox list is; an undated issue goes last. */
const byChange = (left, right) => (left.anchorMs ?? Infinity) - (right.anchorMs ?? Infinity);

/**
 * The rows, from each followed project's cached records: `sources` is
 * `[{ project, issues, details, askedOnly }]`, where `project` is the feed's
 * project (`id`, `deviceId`, `projectKey`, `name`), `issues` the cached
 * `issues.list`, `details` the cached `issues.get` answers by issue id, and
 * `askedOnly` the machine's cached Needs you rule (core/needsYouRule.js).
 */
export function watchedIssueEntries(sources = []) {
  const entries = [];
  for (const { project, issues = [], details = new Map(), askedOnly = false } of sources) {
    for (const issue of issues) {
      const detail = details.get(issue.id) || null;
      const reasons = watchedIssueReasons(issue, detail, askedOnly);
      if (reasons.length) entries.push(toEntry(project, issue, detail, reasons, askedOnly));
    }
  }
  return entries.sort(byChange);
}
