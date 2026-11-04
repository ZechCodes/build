// The task list's attention groups, from records already in the cache.
// Feed rows say which agents hold tasks, whether each is mid-turn, and which
// watched tasks are in the user's inbox; a cached task detail supplies comments and the read mark.
// This module neither reads the bridge nor decides when a cache record loads.
//
// Two rules for what needs the user, chosen by `askedOnly` (#144). A bridge
// that keeps `notify_user` on comments (`tasks.commentUserNotifies`) gets the
// narrow one: assigned to the user, an unread agent comment that mentioned
// or asked them, or a mentioned creation. The In review column alone no longer counts, because agents
// review each other's work there. Against an older bridge, which cannot say
// which comments asked, the earlier rule stands: In review, or any unread
// agent comment on a watched task.

import { taskIsBlocked } from "./trackerTaskReadiness.js";
import { entityIdOf } from "./entityId.js";
import { projectCoordinatorAgent, projectRosterRows } from "./projectRosterFeed.js";
import { isFinished } from "./trackerAgentTasks.js";
import { changedSince } from "./trackerModel.js";
import { latestTaskMark } from "./trackerUnread.js";
import { partialTimelineCount, timelineNeedsCounters } from "./taskTimelineWindow.js";

/** Tasks held by an agent the feed lists for this project, mid-turn or not
 *  and whatever its job, until they reach Done. The id predates the rule. */
export const WORKING_GROUP = "working";
export const NEEDS_YOU_GROUP = "needsYou";
export const REST_GROUP = "rest";

export const ATTENTION_REASONS = Object.freeze({
  inReview: "in_review",
  inbox: "inbox_comment",
  assigned: "assigned_to_user",
});

const REASON_LABELS = Object.freeze({
  [ATTENTION_REASONS.inReview]: "In review",
  [ATTENTION_REASONS.inbox]: "Mentioned you",
  [ATTENTION_REASONS.assigned]: "Assigned to you",
});

export const attentionReasonLabel = (reason) => REASON_LABELS[reason] || "";

const itemsOf = (feed) => feed?.items || [];
const projectsOf = (feed) => feed?.projects || [];

/** A task's event ids share a time-ordered suffix, while `tc-` and `te-`
 *  themselves do not sort together. This is the bridge inbox's comparison. */
const orderedId = (id) => String(id || "").split("-", 2).at(-1);
const afterMark = (id, mark) => !mark || orderedId(id) > orderedId(mark);

const isAgentComment = (entry) => entry?.type === "comment" && entry.author?.kind === "agent";
const isAgentCreatedEvent = (entry) => entry?.type === "event" && entry.kind === "created" && entry.actor?.kind === "agent";
const SERVICE_NOTICES = new Set(["revision_ready", "verdict_undelivered", "revision_undelivered"]);
const isServiceAsk = (entry) => entry?.type === "comment" && entry.author?.kind === "build" &&
  entry.mentions_user === true && SERVICE_NOTICES.has(entry.review_notice);
const asksTheUser = (entry) => entry.mentions_user === true || entry.notifies_user === true;

/** The agent comments the user has not read, from the cached timeline. The
 *  read mark is the newer of the detail's and the list's: a read on another
 *  tab reaches the pushed list before the detail is read again. None until a
 *  timeline is cached. */
export function unreadAgentComments(task, detail) {
  if (!Array.isArray(detail?.timeline)) return [];
  const mark = latestTaskMark(detail.task?.read_through, task?.read_through) || "";
  return detail.timeline.filter((entry) => isAgentComment(entry) && afterMark(entry.id, mark));
}

/** Unread requests for the user: the narrow rule includes a mentioned created
 *  event, and the older rule still counts all unread agent comments. */
export const unreadAsks = (task, detail, askedOnly = false, reviewSupport = {}) => {
  if (!askedOnly) return unreadAgentComments(task, detail);
  if (!Array.isArray(detail?.timeline)) return [];
  const mark = latestTaskMark(detail.task?.read_through, task?.read_through) || "";
  return detail.timeline.filter((entry) =>
    ((isAgentComment(entry) || isAgentCreatedEvent(entry)) && asksTheUser(entry) ||
      reviewSupport.verdictDelivery && isServiceAsk(entry)) && afterMark(entry.id, mark));
};

/** The earlier rule's unread comment. A cached inbox row is proof the user is
 *  watching the task. Its unread count alone may be an event such as a move,
 *  so prefer the cached timeline when there is one and require an unread agent
 *  comment there. */
export function hasUnreadInboxComment(task, detail, inboxRow) {
  if (!inboxRow || inboxRow.done_until_next === true || !(Number(inboxRow.unread) > 0)) return false;
  if (timelineNeedsCounters(detail)) {
    const count = partialTimelineCount(task, detail, "unread_agent_comments");
    if (count !== null) return count > 0;
  }
  return unreadAgentComments(task, detail).length > 0;
}

/** Every agent the project's feed rows list, keyed by agent id so a task's
 *  tagged assignee can find its own digest, working or not. */
export function knownAgentsOf(feed, projectKey) {
  const known = new Map();
  for (const row of projectRosterRows(feed)) {
    if (row.projectKey !== projectKey) continue;
    for (const agent of row.agents || []) {
      if (agent?.id) known.set(agent.id, { agent, row });
    }
  }
  return known;
}

/** A legacy assignment names the coordinator. A complete worker roster's
 *  persisted pin remains authoritative regardless of working state or order. */
function projectAgentOf(feed, projectKey) {
  const project = projectsOf(feed).find((candidate) => candidate.projectKey === projectKey);
  const ownerId = project?.entity_id || project?.run_id;
  if (!ownerId) return null;
  const row = projectRosterRows(feed).find((item) => item.projectKey === projectKey && entityIdOf(item) === ownerId);
  const agents = row?.agents || [];
  const agent = projectCoordinatorAgent(project, agents);
  return agent ? { agent, row } : null;
}

const knownHolderOf = (assignee, knownAgents, projectAgent) => {
  if (assignee?.kind === "project_agent") return projectAgent;
  return assignee?.kind === "agent" ? knownAgents.get(assignee.agent_id) || null : null;
};

/** The known agent holding an unfinished task, with whether it is mid-turn. */
const holdingAgentOf = (task, knownAgents, projectAgent) => {
  if (isFinished(task)) return null;
  const holder = knownHolderOf(task?.assignee, knownAgents, projectAgent);
  return holder ? { ...holder, working: holder.agent.working === true } : null;
};

/** Compact summary counts stand in for history until current details are cached. */
const serviceHistoryIsCurrent = (task, detail) => !timelineNeedsCounters(detail) && !changedSince(task, detail);
const taskReadMark = (task, detail) => latestTaskMark(detail?.task?.read_through, task?.read_through) || "";

export function hasUnreadReviewServiceAsk(task, detail, reviewSupport = {}) {
  if (!reviewSupport.verdictDelivery || task?.watched !== true) return false;
  if (timelineNeedsCounters(detail)) {
    const count = partialTimelineCount(task, detail, "unread_service_mentions");
    if (count !== null) return count > 0;
  }
  if (!serviceHistoryIsCurrent(task, detail)) return Number(task.unread_service_mentions) > 0;
  const mark = taskReadMark(task, detail);
  return detail.timeline.some((entry) => isServiceAsk(entry) && afterMark(entry.id, mark));
}

const reviewNeedsAuthor = (task, reviewSupport) => reviewSupport.verdictDelivery &&
  ["changes_requested", "approved"].includes(task?.review_summary?.status);
const assignmentNeedsUser = (task, reviewSupport) => task?.assignee?.kind === "user" &&
  !taskIsBlocked(task) && !reviewNeedsAuthor(task, reviewSupport);

const reasonsOf = (task, hasUnreadComment, askedOnly, serviceAsk, reviewSupport) => {
  if (isFinished(task)) return serviceAsk ? [ATTENTION_REASONS.inbox] : [];
  const reasons = [];
  if (!askedOnly && task?.status === "in_review") reasons.push(ATTENTION_REASONS.inReview);
  if (hasUnreadComment || serviceAsk) reasons.push(ATTENTION_REASONS.inbox);
  if (assignmentNeedsUser(task, reviewSupport)) reasons.push(ATTENTION_REASONS.assigned);
  return reasons;
};

/** Whether an unread comment puts the task in Needs you. The narrow rule reads
 *  the cached task and timeline alone, as the inbox does: the board's feed row
 *  is only re-read with the board, so it can still count nothing unread long
 *  after an `tasks` push cached the comment that asked, and still be there
 *  long after Stop watching cached `watched: false`. A bridge that announces
 *  the rule always says `watched`, so the task's own is the watch. */
const hasUnreadAsk = (task, detail, inboxRow, askedOnly, reviewSupport) => (askedOnly
  ? task?.watched === true && hasUnreadTimelineAsk(task, detail, true, reviewSupport)
  : hasUnreadInboxComment(task, detail, inboxRow));

function hasUnreadTimelineAsk(task, detail, askedOnly, reviewSupport) {
  const count = timelineNeedsCounters(detail)
    ? partialTimelineCount(task, detail, askedOnly ? "unread_mentions" : "unread_agent_comments") : null;
  return count !== null ? count > 0 : unreadAsks(task, detail, askedOnly, reviewSupport).length > 0;
}

const attentionReasonsOf = (task, detail, inboxRow, askedOnly, reviewSupport) =>
  reasonsOf(task, hasUnreadAsk(task, detail, inboxRow, askedOnly, reviewSupport), askedOnly,
    hasUnreadReviewServiceAsk(task, detail, reviewSupport), reviewSupport);

/** Why a watched task is in the inbox (#125): the same reasons as Needs you,
 *  read from the cached task records alone. The task's own `watched` is the
 *  watch, so no feed row is consulted. None for a task nobody watches. */
export const watchedTaskReasons = (task, detail, askedOnly = false, reviewSupport = {}) =>
  task?.watched === true ? reasonsOf(task, hasUnreadTimelineAsk(task, detail, askedOnly, reviewSupport), askedOnly,
    hasUnreadReviewServiceAsk(task, detail, reviewSupport), reviewSupport) : [];

/** One task's attention, with every reason available to a Dashboard row.
 *  Held by a known agent wins for list placement, but the reasons are retained so the
 *  Dashboard can still explain what needs the user's look. */
export function taskAttention(task, {
  knownAgents = new Map(), projectAgent = null, detail = null, inboxRow = null, askedOnly = false, reviewSupport = {},
} = {}) {
  const holdingAgent = holdingAgentOf(task, knownAgents, projectAgent);
  const reasons = attentionReasonsOf(task, detail, inboxRow, askedOnly, reviewSupport);
  return { holdingAgent, needsYou: reasons.length > 0, reasons, reason: reasons[0] || null };
}

const inboxRowsOf = (feed, projectKey) => new Map(itemsOf(feed)
  .filter((row) => row.projectKey === projectKey && row.kind === "tracker_task")
  .map((row) => [row.task_id, row]));

const attentionGroupOf = (attention) =>
  attention.holdingAgent ? WORKING_GROUP : attention.needsYou ? NEEDS_YOU_GROUP : REST_GROUP;

/** Stable partition of the list's existing order. `detailById` contains the
 *  cached `{task,timeline}` records, keyed by task id; it may be incomplete
 *  while a task page has never been opened. */
export function attentionGroups(tasks, { feed = null, projectKey = "", detailById = new Map(), askedOnly = false, reviewSupport } = {}) {
  const groups = { [WORKING_GROUP]: [], [NEEDS_YOU_GROUP]: [], [REST_GROUP]: [], attentionById: new Map() };
  const knownAgents = knownAgentsOf(feed, projectKey);
  const projectAgent = projectAgentOf(feed, projectKey);
  const inboxRows = inboxRowsOf(feed, projectKey);
  for (const task of tasks || []) {
    const detail = detailById.get(task.id) || null;
    const attention = taskAttention(task, {
      knownAgents,
      projectAgent,
      detail,
      inboxRow: inboxRows.get(task.id) || null,
      askedOnly, reviewSupport,
    });
    groups.attentionById.set(task.id, attention);
    groups[attentionGroupOf(attention)].push(task);
  }
  return groups;
}
