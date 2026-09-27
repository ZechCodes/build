// The Dashboard is a projection of the issue list, feed, and detail records
// already held by the client. Missing detail means an unknown timeline, not
// proof that an issue was completed or that a comment was read.

import { attentionGroups, attentionReasonLabel } from "./trackerAttentionModel.js";
import { agentLabels, projectName, workspaceAgents } from "./trackerAssignee.js";
import { actorName } from "./trackerLineWords.js";
import { firstLine } from "./activityDigest.js";
import { PRIORITIES, columnName } from "./trackerModel.js";
import { isFinished } from "./trackerAgentIssues.js";

const MINUTE_MS = 60 * 1000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
/** How long an absence "since you left" reaches back over. Past it the user
 *  starts from a blank slate: 96 hours holds a three-day weekend, and a week
 *  away is too much to catch up on as a list. */
export const DONE_SINCE_CAP_MS = 96 * HOUR_MS;
/** The bridge states its own gap; this is only for an answer that does not. */
const USER_SESSION_GAP_MS = 6 * HOUR_MS;
const isDoneMove = (entry) =>
  entry?.type === "event" && entry.kind === "moved" && entry.payload?.to === "done";
const activityLineOf = (item) => {
  const data = item?.data;
  const words = item?.type === "event" ? data?.summary : data?.role === "agent" ? data?.body : "";
  return firstLine(words).trim();
};

const digestActivityLine = (window) => {
  const digests = Array.isArray(window?.activityDigests) ? window.activityDigests : [];
  const calls = digests.map((digest) => digest?.last_tool_call).filter((call) => call?.summary);
  const latest = calls.reduce((best, call) => !best || call.sequence > best.sequence ? call : best, null);
  return firstLine(latest?.summary).trim();
};

/** A line the latest cached conversation window can actually substantiate.
 * User messages are requests, not agent activity. A page may have cut older
 * activity into digests, so their last tool call is the fallback. */
export function latestCachedAgentActivity(window) {
  const items = Array.isArray(window?.items) ? window.items : [];
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const line = activityLineOf(items[index]);
    if (line) return line;
  }
  return digestActivityLine(window);
}

/** The most recent cached move into Done while the issue still stands there.
 * `issues.get` delivers the timeline in event order, so walking it backwards
 * finds the latest move without sorting tied timestamps differently. */
export function doneMoveToday(issue, detail, nowMs) {
  if (issue?.status !== "done" || !Number.isFinite(nowMs)) return null;
  const timeline = detail?.timeline;
  if (!Array.isArray(timeline)) return null;
  const event = timeline.findLast(isDoneMove);
  if (!event) return null;
  const movedMs = Date.parse(event.at || "");
  return movedMs >= nowMs - DAY_MS && movedMs <= nowMs ? event.at : null;
}

/**
 * Where "Done since you left" starts, in epoch milliseconds.
 *
 * `session` is the bridge's user session as the cache holds it
 * (core/userSessionCache.js). The cutoff is the last thing the user did before
 * their latest silence of `gap_ms` or more. An arrival is recorded on the
 * bridge (`user.present`), so normally the bridge has already started this
 * session and says where the last one ended.
 *
 * A silence the bridge had already seen when it answered (a list read that
 * landed before the arrival did) counts from its last activity. That is read
 * off the answer alone: `now_ms`, the bridge's clock when it answered, against
 * `last_activity_ms`. Nothing is inferred from time passing since the answer:
 * the user may have been busy on another client all along, and that keeps the
 * session going without moving anything this tab holds. Whatever does move it
 * (a new session) is pushed.
 *
 * With no earlier session, or one that ended more than 96 hours before this
 * one started, the cutoff is this session's start: a blank slate that fills
 * as work finishes while the user is here. Until the bridge has recorded that
 * start the slate shows nothing. The cap measures the absence, not the time
 * since it, so a long weekend does not vanish halfway through the day the
 * user comes back.
 */
export function doneSinceCutoff(session) {
  const { started, ended } = sessionAt(session);
  if (started === null) return ended ?? Infinity;
  return ended === null || started - ended > DONE_SINCE_CAP_MS ? started : ended;
}

const finite = (value) => (Number.isFinite(value) ? value : null);
const gapOf = (session) => (session?.gap_ms > 0 ? session.gap_ms : USER_SESSION_GAP_MS);

/** Where the user stood when the bridge answered: `here` inside a session,
 *  `away` in a silence of a gap or more, `gone` past the 96-hour cap, and
 *  null with no activity to measure from. */
export function standingOf(session) {
  const last = finite(session?.last_activity_ms);
  const now = finite(session?.now_ms);
  if (last === null) return null;
  if (now === null || now - last < gapOf(session)) return "here";
  return now - last > DONE_SINCE_CAP_MS ? "gone" : "away";
}

/** This session's start and where the one before it ended. A silence the
 *  bridge had seen has no start: it ended at the last activity, unless it had
 *  run past the cap, which is a blank slate. */
function sessionAt(session) {
  const standing = standingOf(session);
  if (standing === null || standing === "gone") return { started: null, ended: null };
  const last = session.last_activity_ms;
  if (standing === "away") return { started: null, ended: last };
  return { started: finite(session.session_started_ms) ?? last, ended: finite(session.previous_session_ended_ms) };
}

/** When an issue moved into Done, if it is there and got there at or after
 *  `cutoffMs`. Read from the list record alone, so an issue whose timeline
 *  this client never fetched is not missed. */
export function doneSince(issue, cutoffMs) {
  if (issue?.status !== "done" || !Number.isFinite(cutoffMs)) return null;
  const movedMs = Date.parse(issue.done_at || "");
  return movedMs >= cutoffMs ? issue.done_at : null;
}

/** Where the current session started, for grouping Done. A user the bridge
 *  saw away, or long gone, has no session yet: everything in Done finished
 *  while they were away. */
export function doneSessionStart(session) {
  return sessionAt(session).started ?? Infinity;
}

const QUARTER_MS = 15 * MINUTE_MS;
const AWAY_GROUP = { id: "away", title: "While you were away", rank: Infinity };

/** The group one move into Done falls in: quarter hours for the first hour,
 *  whole hours after, and anything before this session apart. */
function doneGroupOf(movedMs, nowMs, sessionStartedMs) {
  if (sessionStartedMs !== null && movedMs < sessionStartedMs) return AWAY_GROUP;
  const age = Math.max(0, nowMs - movedMs);
  if (age < HOUR_MS) {
    const quarter = Math.floor(age / QUARTER_MS);
    const title = quarter === 0 ? "Last 15 minutes" : `${quarter * 15} minutes ago`;
    return { id: `minutes-${quarter * 15}`, title, rank: quarter };
  }
  const hours = Math.floor(age / HOUR_MS);
  return { id: `hours-${hours}`, title: hours === 1 ? "1 hour ago" : `${hours} hours ago`, rank: 3 + hours };
}

/** Done's entries split by how long ago they moved, newest group first and
 *  each group in the entries' own order. Only groups with entries are given.
 *  `sessionStartedMs` is null on a bridge without the user's session, which
 *  has no "While you were away". */
export function doneGroups(entries, { nowMs, sessionStartedMs = null }) {
  const groups = new Map();
  for (const entry of entries || []) {
    const group = doneGroupOf(Date.parse(entry.movedAt), nowMs, sessionStartedMs);
    if (!groups.has(group.id)) groups.set(group.id, { ...group, entries: [] });
    groups.get(group.id).entries.push(entry);
  }
  return [...groups.values()].sort((a, b) => a.rank - b.rank)
    .map(({ id, title, entries: grouped }) => ({ id, title, entries: grouped }));
}

const cachedActivity = (activityByAgent, agentId) => {
  const snippet = activityByAgent instanceof Map
    ? activityByAgent.get(agentId)
    : activityByAgent?.[agentId];
  return typeof snippet === "string" ? snippet.trim() : "";
};

/** Who holds a task, as its Assigned row says it: "you" for the user, and
 *  otherwise the name every other surface gives that actor, read off the
 *  cached issue's own identities. Null for nobody. */
const holderOf = (issue, reading) => {
  const assignee = issue.assignee;
  if (!assignee?.kind) return null;
  if (assignee.kind === "user") return "you";
  return actorName(assignee, { ...reading, identities: issue.identities || {} }) || null;
};

/** How pressing a priority is, highest first; an unknown one reads as none. */
const priorityRank = (priority) => -Math.max(0, PRIORITIES.findIndex((candidate) => candidate.id === priority));

/** One priority-ordered pass partitions every open task by holder and live
 *  agent state. An assigned agent absent from the feed is still assigned. */
function activeAndBacklog(issues, grouped, reading, activityByAgent, columns) {
  const working = [];
  const assigned = [];
  const backlog = [];
  const open = (issues || []).filter((issue) => issue && !isFinished(issue));
  open.sort((left, right) => priorityRank(left.priority) - priorityRank(right.priority));
  for (const issue of open) {
    const column = columnName(columns, issue.status);
    if (!issue.assignee?.kind) {
      backlog.push({ issue, columnName: column });
      continue;
    }
    const holding = grouped.attentionById.get(issue.id)?.holdingAgent;
    if (!holding?.working) {
      assigned.push({ issue, holder: holderOf(issue, reading), columnName: column });
      continue;
    }
    working.push({
      issue,
      agentName: actorName({ kind: "agent", agent_id: holding.agent.id }, reading),
      activity: cachedActivity(activityByAgent, holding.agent.id),
      working: true,
    });
  }
  const activeGroups = [
    { id: "working", title: "Working", entries: working },
    { id: "assigned", title: "Assigned", entries: assigned },
  ].filter((group) => group.entries.length);
  return { active: [...working, ...assigned], activeGroups, backlog };
}

/** Four sections of the same cached task list. Active and Backlog put the
 *  most pressing first within their groups.
 * `activityByAgent` contains optional text read from cached conversations,
 * keyed by agent id. No feed digest provides a latest activity snippet.
 *
 * `doneCutoffMs` is given for a bridge that carries `done_at` and the user's
 * session: Done is then everything finished since the user left. Without it
 * Done falls back to the cached timelines' last 24 hours. `sessionStartedMs`
 * comes with it, and sets apart what finished before this session.
 *
 * `askedOnly` is the machine's cached Needs you rule (core/needsYouRule.js).
 * `columns` are the cached `issues.columns`, which name rows' board columns. */
export function dashboardSections(issues, {
  feed = null,
  projectKey = "",
  detailById = new Map(),
  nowMs = Date.now(),
  activityByAgent = new Map(),
  doneCutoffMs = null,
  sessionStartedMs = null,
  askedOnly,
  columns,
} = {}) {
  const grouped = attentionGroups(issues, { feed, projectKey, detailById, askedOnly });
  const reading = {
    agentLabels: agentLabels(workspaceAgents(feed, projectKey)),
    projectName: projectName(feed, projectKey),
  };
  const placement = activeAndBacklog(issues, grouped, reading, activityByAgent, columns);
  const needsYou = (issues || []).flatMap((issue) => {
    const { reasons } = grouped.attentionById.get(issue.id);
    return reasons.length ? [{ issue, reasons, reasonLabels: reasons.map(attentionReasonLabel) }] : [];
  });
  const done = doneEntries(issues, { detailById, nowMs, doneCutoffMs });
  return {
    ...placement, needsYou,
    done, doneGroups: doneGroups(done, { nowMs, sessionStartedMs }),
  };
}

function doneEntries(issues, { detailById, nowMs, doneCutoffMs }) {
  const movedAtOf = doneCutoffMs === null
    ? (issue) => doneMoveToday(issue, detailById.get(issue.id), nowMs)
    : (issue) => doneSince(issue, doneCutoffMs);
  return (issues || []).flatMap((issue) => {
    const movedAt = movedAtOf(issue);
    return movedAt ? [{ issue, movedAt, sha: issue.links?.commits?.at(-1) || null }] : [];
  });
}
