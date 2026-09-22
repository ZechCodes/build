// The issue list's attention groups, from records already in the cache.
// Feed rows say which agents are working and which watched issues are in the
// user's inbox; a cached issue detail supplies comments and the read mark.
// This module neither reads the bridge nor decides when a cache record loads.

import { entityIdOf } from "./entityId.js";

export const WORKING_GROUP = "working";
export const NEEDS_YOU_GROUP = "needsYou";
export const REST_GROUP = "rest";

export const ATTENTION_REASONS = Object.freeze({
  inReview: "in_review",
  question: "unanswered_question",
  inbox: "inbox_comment",
  assigned: "assigned_to_user",
});

const REASON_LABELS = Object.freeze({
  [ATTENTION_REASONS.inReview]: "In review",
  [ATTENTION_REASONS.question]: "Asked you a question",
  [ATTENTION_REASONS.inbox]: "Mentioned you",
  [ATTENTION_REASONS.assigned]: "Assigned to you",
});

export const attentionReasonLabel = (reason) => REASON_LABELS[reason] || "";

const itemsOf = (feed) => feed?.items || [];
const projectsOf = (feed) => feed?.projects || [];

/** An issue's event ids share a time-ordered suffix, while `ic-` and `ie-`
 *  themselves do not sort together. This is the bridge inbox's comparison. */
const orderedId = (id) => String(id || "").split("-", 2).at(-1);
const afterMark = (id, mark) => !mark || orderedId(id) > orderedId(mark);

const isAgentComment = (entry) => entry?.type === "comment" && entry.author?.kind === "agent";

/** A later user comment answers the earlier agent question. The timeline is
 *  already ordered by the bridge, so a second sort would lose tied events. */
export function hasUnansweredAgentQuestion(timeline) {
  let asking = false;
  for (const entry of timeline || []) {
    if (entry?.type !== "comment") continue;
    if (entry.author?.kind === "user") asking = false;
    else if (isAgentComment(entry) && String(entry.body || "").includes("?")) asking = true;
  }
  return asking;
}

/** A cached inbox row is proof the user is watching the issue. Its unread
 *  count alone may be an event such as a move, so prefer the cached timeline
 *  when there is one and require an unread agent comment there. The bridge
 *  does not persist `notify_user` on comments; it turns on issue.watched. */
export function hasUnreadInboxComment(issue, detail, inboxRow) {
  if (!inboxRow || inboxRow.done_until_next === true || !(Number(inboxRow.unread) > 0)) return false;
  if (!Array.isArray(detail?.timeline)) return false;
  const mark = detail.issue?.read_through || issue?.read_through || "";
  return detail.timeline.some((entry) => isAgentComment(entry) && afterMark(entry.id, mark));
}

/** An agent's digest on a project-scoped feed row. The map is keyed by agent
 *  id so an issue's tagged assignee can find its own working digest. */
export function workingAgentsOf(feed, projectKey) {
  const working = new Map();
  for (const row of itemsOf(feed)) {
    if (row.projectKey !== projectKey) continue;
    for (const agent of row.agents || []) {
      if (agent?.id && agent.working === true) working.set(agent.id, { agent, row });
    }
  }
  return working;
}

/** A project-agent assignment has no agent id on the issue. When the feed
 *  carries the project owner's row, its owner id from project.list identifies
 *  that row without mistaking a workspace agent for the project's own. Some
 *  bridge snapshots omit the owner row; then its working state is unknown. */
function workingProjectAgentOf(feed, projectKey) {
  const ownerId = projectOwnerId(feed, projectKey);
  if (!ownerId) return null;
  const row = itemsOf(feed).find((item) => item.projectKey === projectKey && entityIdOf(item) === ownerId);
  const agent = (row?.agents || []).find((candidate) => candidate.working === true);
  return agent ? { agent, row } : null;
}

const projectOwnerId = (feed, projectKey) => {
  const owner = projectsOf(feed).find((project) => project.projectKey === projectKey);
  return owner?.entity_id || owner?.run_id || null;
};

const workingAgentOf = (assignee, workingAgents, projectAgent) => {
  if (assignee?.kind === "project_agent") return projectAgent;
  return assignee?.kind === "agent" ? workingAgents.get(assignee.agent_id) || null : null;
};

const attentionReasonsOf = (issue, detail, inboxRow) => {
  const reasons = [];
  if (issue?.status === "in_review") reasons.push(ATTENTION_REASONS.inReview);
  if (hasUnansweredAgentQuestion(detail?.timeline)) reasons.push(ATTENTION_REASONS.question);
  if (hasUnreadInboxComment(issue, detail, inboxRow)) reasons.push(ATTENTION_REASONS.inbox);
  if (issue?.assignee?.kind === "user") reasons.push(ATTENTION_REASONS.assigned);
  return reasons;
};

/** One issue's attention, with every reason available to a Dashboard row.
 *  Working wins for list placement, but the reasons are retained so the
 *  Dashboard can still explain what needs the user's look. */
export function issueAttention(issue, { workingAgents = new Map(), projectAgent = null, detail = null, inboxRow = null } = {}) {
  const workingAgent = workingAgentOf(issue?.assignee, workingAgents, projectAgent);
  const reasons = attentionReasonsOf(issue, detail, inboxRow);
  return { workingAgent, needsYou: reasons.length > 0, reasons, reason: reasons[0] || null };
}

const inboxRowsOf = (feed, projectKey) => new Map(itemsOf(feed)
  .filter((row) => row.projectKey === projectKey && row.kind === "tracker_issue")
  .map((row) => [row.issue_id, row]));

const attentionGroupOf = (attention) =>
  attention.workingAgent ? WORKING_GROUP : attention.needsYou ? NEEDS_YOU_GROUP : REST_GROUP;

/** Stable partition of the list's existing order. `detailById` contains the
 *  cached `{issue,timeline}` records, keyed by issue id; it may be incomplete
 *  while an issue page has never been opened. */
export function attentionGroups(issues, { feed = null, projectKey = "", detailById = new Map() } = {}) {
  const groups = { [WORKING_GROUP]: [], [NEEDS_YOU_GROUP]: [], [REST_GROUP]: [], attentionById: new Map() };
  const workingAgents = workingAgentsOf(feed, projectKey);
  const projectAgent = workingProjectAgentOf(feed, projectKey);
  const inboxRows = inboxRowsOf(feed, projectKey);
  for (const issue of issues || []) {
    const detail = detailById.get(issue.id) || null;
    const attention = issueAttention(issue, {
      workingAgents,
      projectAgent,
      detail,
      inboxRow: inboxRows.get(issue.id) || null,
    });
    groups.attentionById.set(issue.id, attention);
    groups[attentionGroupOf(attention)].push(issue);
  }
  return groups;
}
