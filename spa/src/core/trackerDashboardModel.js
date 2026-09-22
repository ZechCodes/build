// The Dashboard is a projection of the issue list, feed, and detail records
// already held by the client. Missing detail means an unknown timeline, not
// proof that an issue was completed or that a comment was read.

import { attentionGroups, attentionReasonLabel } from "./trackerAttentionModel.js";
import { agentLabels, projectName, workspaceAgents } from "./trackerAssignee.js";
import { actorName } from "./trackerLineWords.js";
import { firstLine } from "./activityDigest.js";

const DAY_MS = 24 * 60 * 60 * 1000;
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

const cachedActivity = (activityByAgent, agentId) => {
  const snippet = activityByAgent instanceof Map
    ? activityByAgent.get(agentId)
    : activityByAgent?.[agentId];
  return typeof snippet === "string" ? snippet.trim() : "";
};

/** Three sections of the same cached issue list, in its existing order.
 * `activityByAgent` contains optional text read from cached conversations,
 * keyed by agent id. No feed digest provides a latest activity snippet. */
export function dashboardSections(issues, {
  feed = null,
  projectKey = "",
  detailById = new Map(),
  nowMs = Date.now(),
  activityByAgent = new Map(),
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
  const doneToday = (issues || []).flatMap((issue) => {
    const movedAt = doneMoveToday(issue, detailById.get(issue.id), nowMs);
    return movedAt ? [{ issue, movedAt, sha: issue.links?.commits?.at(-1) || null }] : [];
  });
  return { inProgress, needsYou, doneToday };
}
