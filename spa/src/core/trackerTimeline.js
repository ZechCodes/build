// A task's timeline: comments and events, interleaved.
//
// `tasks.get` answers them as one ascending list already ordered by
// `(timestamp, id)` — ids are time-ordered, so two entries stamped in the same
// second still have one order every reader agrees on. Nothing here re-sorts:
// re-sorting on the timestamp alone would scramble exactly those pairs.
//
// The two records stamp themselves differently on purpose — a comment was
// WRITTEN (`created_at`), an event HAPPENED (`at`) — and they name their actor
// differently for the same reason (`author`, `actor`). Both are normalized to
// one row shape here, so a renderer reads one thing.
//
// No DOM, no app imports.

import { columnName } from "./trackerModel.js";
import { actorName } from "./trackerLineWords.js";
import { humanBytes } from "./workspaceLifecycle.js";

const COMMENT = "comment";
const EVENT = "event";

/** `key` is the record's own id — stable across every repaint, which is what a
 *  keyed list needs — falling back to the position for a record written
 *  without one. */
const keyOf = (entry, index) => String(entry.id || `${entry.type}-${index}`);

/** A comment was WRITTEN and an event HAPPENED, so the two stamp themselves
 *  under different keys. One reading, here. */
const stampOf = (entry) => entry.created_at || entry.at || "";

const commentRow = (entry, base) => ({
  ...base,
  type: COMMENT,
  actor: entry.author || null,
  body: String(entry.body || ""),
  mentionsUser: entry.mentions_user === true,
  refs: entry.refs || [],
  attachments: entry.attachments || [],
});

const eventRow = (entry, base) => ({
  ...base,
  type: EVENT,
  actor: entry.actor || null,
  kind: String(entry.kind || ""),
  mentionsUser: entry.mentions_user === true,
  payload: entry.payload || {},
});

/** One entry, as a row. */
function timelineRow(entry, index) {
  const type = entry && entry.type;
  if (type !== COMMENT && type !== EVENT) return null;
  const base = { key: keyOf(entry, index), at: stampOf(entry) };
  return type === COMMENT ? commentRow(entry, base) : eventRow(entry, base);
}

/** The whole timeline as rows, in the order it arrived. An entry of a type this
 *  client has never heard of is dropped rather than drawn blank: a later minor
 *  adding a kind must not put an empty row in a reader's history. */
export const timelineRows = (timeline) =>
  (timeline || []).map((entry, index) => timelineRow(entry, index)).filter(Boolean);

/** The comments alone, for a count beside the title. */
export const commentRows = (rows) => (rows || []).filter((row) => row.type === COMMENT);

const labelChange = (payload) => {
  const added = (payload.added || []).join(", ");
  const removed = (payload.removed || []).join(", ");
  if (added && removed) return `added ${added} and removed ${removed}`;
  if (added) return `added ${added}`;
  return removed ? `removed ${removed}` : "changed the labels";
};

/** What a `linked` event linked. The payload carries the link that was added,
 *  which is one of the five keys a task link names. */
const linkChange = (payload) => {
  const named = ["workspace_id", "branch", "commit", "conversation_id", "parent_task_id"]
    .map((key) => (payload[key] ? `${key.replace(/_/g, " ")} ${payload[key]}` : ""))
    .find(Boolean);
  return named ? `linked ${named}` : "linked this";
};

/** Done deleted the branch the task's work was on (#87), or a reclaim did
 *  (#167, `reclaimed`). */
/** The branch, and the repository it was in when the event names one: a
 *  reclaim over several sources says which (#167). */
const repositoryNamed = (payload) => {
  const name = String(payload.repository || "").replace(/\/+$/, "").split("/").pop();
  return name ? ` in ${name}` : "";
};
const branchNamed = (payload) =>
  `${payload.branch ? `branch ${payload.branch}` : "the branch"}${repositoryNamed(payload)}`;
const branchDeletedSentence = (payload) => {
  const when = payload.reclaimed ? "reclaimed" : "finished";
  const sentence = `deleted ${branchNamed(payload)} when the workspace was ${when}`;
  return payload.reason ? `${sentence}; ${payload.reason}` : sentence;
};

/** A reclaim left the branch where it was, and the bridge's sentence says
 *  why (#167). */
const branchKeptSentence = (payload) => {
  const sentence = `kept ${branchNamed(payload)} when the workspace was reclaimed`;
  return payload.reason ? `${sentence}. ${payload.reason}` : sentence;
};

const closedSentence = (payload) =>
  payload.reason === "workspace_finished" ? "closed this when the workspace was finished" : "closed this";

/** A dispatching assign moves the task itself; an agent reporting Complete
 *  moves it too, and says so. The `by` on the payload is what tells them
 *  apart — the reader should not have to guess why a card moved on its own. */
const movedSentence = (payload, columns) => {
  const journey = `moved this from ${columnName(columns, payload.from)} to ${columnName(columns, payload.to)}`;
  return payload.by === "report" ? `${journey} on reporting Complete` : journey;
};

/** What the workspace reclaim service (#135) recorded about a workspace this
 *  task links: it went quiet, its build output was dropped, it was reclaimed. */
const workspaceNamed = (payload) => `workspace ${payload.workspace_name || payload.workspace_id || ""}`.trim();
/** The idle threshold a `workspace_idle` was written under (#167), in the
 *  largest whole unit: "a day", "6 hours", "90 minutes". Events from before
 *  it was a setting carry none, and were a day. */
const QUIET_UNITS = [[86_400, "a day", "days"], [3600, "an hour", "hours"], [60, "a minute", "minutes"]];
const quietFor = (seconds) => {
  const secs = Number(seconds);
  if (!Number.isFinite(secs) || secs <= 0) return "a day";
  const [size, one, many] = QUIET_UNITS.find(([unit]) => secs % unit === 0) || [1, "a second", "seconds"];
  const count = secs / size;
  return count === 1 ? one : `${count} ${many}`;
};
const reclaimedSentence = (payload) => {
  const size = payload.size_bytes ? ` (${humanBytes(payload.size_bytes)})` : "";
  return `reclaimed ${workspaceNamed(payload)}${size}`;
};

const SENTENCES = Object.freeze({
  created: () => "filed this",
  assigned: (payload, _columns, reading) => `assigned this to ${actorName(payload.assignee, reading) || "nobody"}`,
  unassigned: () => "unassigned this",
  moved: movedSentence,
  labelled: (payload) => labelChange(payload),
  linked: (payload) => linkChange(payload),
  closed: (payload) => closedSentence(payload),
  reopened: () => "reopened this",
  dispatched: () => "started an agent on this",
  branch_deleted: (payload) => branchDeletedSentence(payload),
  branch_kept: branchKeptSentence,
  workspace_idle: (payload) =>
    `noted ${workspaceNamed(payload)} has had no activity for ${quietFor(payload.idle_after_secs)}`,
  workspace_pruned: (payload) =>
    `dropped ${humanBytes(payload.pruned_bytes)} of build output from ${workspaceNamed(payload)}`,
  workspace_reclaimed: reclaimedSentence,
});

/**
 * What one event says, as the predicate of a sentence whose subject is its
 * actor: "You moved this from Backlog to In review".
 *
 * An event kind this client has never heard of says its own kind rather than
 * nothing — a later minor adding one leaves a reader with a row they can at
 * least recognize.
 */
export function eventSentence(row, reading = {}) {
  const write = SENTENCES[row?.kind];
  if (!write) return String(row?.kind || "did something");
  // The whole reading goes through, not the labels alone: an actor named in a
  // sentence is named by the tracker's one naming function, which needs the
  // project as well as its agents (#63).
  return write(row.payload || {}, reading.columns || null, reading);
}
