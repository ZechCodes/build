// The Dashboard is a projection of the issue list, feed, and detail records
// already held by the client. Missing detail means an unknown timeline, not
// proof that an issue was completed or that a comment was read.

import { attentionGroups, attentionReasonLabel } from "./trackerAttentionModel.js";
import { agentLabels, projectName, workspaceAgents } from "./trackerAssignee.js";
import { actorName } from "./trackerLineWords.js";
import { firstLine } from "./activityDigest.js";

const HOUR_MS = 60 * 60 * 1000;
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
 * A silence the bridge has not seen end yet (this tab painted before the
 * arrival landed) counts from its last activity, measured on the BRIDGE's
 * clock: the `now_ms` it answered with plus the time since, never this
 * device's own clock, which may be hours out.
 *
 * With no earlier session, or one that ended more than 96 hours before this
 * one started, the cutoff is this session's start: a blank slate that fills
 * as work finishes while the user is here. Until the bridge has recorded that
 * start the slate shows nothing. The cap measures the absence, not the time
 * since it, so a long weekend does not vanish halfway through the day the
 * user comes back.
 */
export function doneSinceCutoff(session, localNowMs) {
  const { started, ended } = sessionAt(session, localNowMs);
  if (started === null) return ended ?? Infinity;
  return ended === null || started - ended > DONE_SINCE_CAP_MS ? started : ended;
}

const finite = (value) => (Number.isFinite(value) ? value : null);
const gapOf = (session) => (session?.gap_ms > 0 ? session.gap_ms : USER_SESSION_GAP_MS);

/** The bridge's clock now: what it said when it answered, plus the time this
 *  device has counted since. Null for a record that carries no clock. */
export function bridgeNow(session, localNowMs) {
  const said = finite(session?.now_ms);
  const heard = finite(session?.received_ms);
  if (said === null || heard === null || !Number.isFinite(localNowMs)) return null;
  return said + Math.max(0, localNowMs - heard);
}

/** This session's start and where the one before it ended. A silence the
 *  bridge has not seen end yet has no start: it ended at the last activity,
 *  unless it has run past the cap, which is a blank slate. */
function sessionAt(session, localNowMs) {
  const last = finite(session?.last_activity_ms);
  const now = bridgeNow(session, localNowMs);
  if (last === null) return { started: null, ended: null };
  if (now !== null && now - last >= gapOf(session)) {
    return { started: null, ended: now - last > DONE_SINCE_CAP_MS ? null : last };
  }
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

const cachedActivity = (activityByAgent, agentId) => {
  const snippet = activityByAgent instanceof Map
    ? activityByAgent.get(agentId)
    : activityByAgent?.[agentId];
  return typeof snippet === "string" ? snippet.trim() : "";
};

/** Three sections of the same cached issue list, in its existing order.
 * `activityByAgent` contains optional text read from cached conversations,
 * keyed by agent id. No feed digest provides a latest activity snippet.
 *
 * `doneCutoffMs` is given for a bridge that carries `done_at` and the user's
 * session: Done is then everything finished since the user left. Without it
 * Done falls back to the cached timelines' last 24 hours. */
export function dashboardSections(issues, {
  feed = null,
  projectKey = "",
  detailById = new Map(),
  nowMs = Date.now(),
  activityByAgent = new Map(),
  doneCutoffMs = null,
} = {}) {
  const grouped = attentionGroups(issues, { feed, projectKey, detailById });
  const reading = {
    agentLabels: agentLabels(workspaceAgents(feed, projectKey)),
    projectName: projectName(feed, projectKey),
  };
  const inProgress = grouped.working.map((issue) => {
    const { agent } = grouped.attentionById.get(issue.id).workingAgent;
    return {
      issue,
      agentName: actorName({ kind: "agent", agent_id: agent.id }, reading),
      activity: cachedActivity(activityByAgent, agent.id),
    };
  });
  const needsYou = (issues || []).flatMap((issue) => {
    const { reasons } = grouped.attentionById.get(issue.id);
    return reasons.length ? [{ issue, reasons, reasonLabels: reasons.map(attentionReasonLabel) }] : [];
  });
  const done = doneEntries(issues, { detailById, nowMs, doneCutoffMs });
  return { inProgress, needsYou, done };
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
