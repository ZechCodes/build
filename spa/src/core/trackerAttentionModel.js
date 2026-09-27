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

import { entityIdOf } from "./entityId.js";
import { isFinished } from "./trackerAgentTasks.js";
import { latestTaskMark } from "./trackerUnread.js";

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
export const unreadAsks = (task, detail, askedOnly = false) => {
  if (!askedOnly) return unreadAgentComments(task, detail);
  if (!Array.isArray(detail?.timeline)) return [];
  const mark = latestTaskMark(detail.task?.read_through, task?.read_through) || "";
  return detail.timeline.filter((entry) =>
    (isAgentComment(entry) || isAgentCreatedEvent(entry)) && asksTheUser(entry) && afterMark(entry.id, mark));
};

/** The earlier rule's unread comment. A cached inbox row is proof the user is
 *  watching the task. Its unread count alone may be an event such as a move,
 *  so prefer the cached timeline when there is one and require an unread agent
 *  comment there. */
export function hasUnreadInboxComment(task, detail, inboxRow) {
  if (!inboxRow || inboxRow.done_until_next === true || !(Number(inboxRow.unread) > 0)) return false;
  return unreadAgentComments(task, detail).length > 0;
}

/** Every agent the project's feed rows list, keyed by agent id so a task's
 *  tagged assignee can find its own digest, working or not. */
export function knownAgentsOf(feed, projectKey) {
  const known = new Map();
  for (const row of itemsOf(feed)) {
    if (row.projectKey !== projectKey) continue;
    for (const agent of row.agents || []) {
      if (agent?.id) known.set(agent.id, { agent, row });
    }
  }
  return known;
}

/** A project-agent assignment has no agent id on the task. When the feed
 *  carries the project owner's row, its owner id from project.list identifies
 *  that row without mistaking a workspace agent for the project's own. Some
 *  bridge snapshots omit the owner row; then the project agent is unknown. The
 *  row's working agent is preferred, else its first. */
function projectAgentOf(feed, projectKey) {
  const ownerId = projectOwnerId(feed, projectKey);
  if (!ownerId) return null;
  const row = itemsOf(feed).find((item) => item.projectKey === projectKey && entityIdOf(item) === ownerId);
  const agents = row?.agents || [];
  const agent = agents.find((candidate) => candidate.working === true) || agents[0];
  return agent ? { agent, row } : null;
}

const projectOwnerId = (feed, projectKey) => {
  const owner = projectsOf(feed).find((project) => project.projectKey === projectKey);
  return owner?.entity_id || owner?.run_id || null;
};

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

const reasonsOf = (task, hasUnreadComment, askedOnly) => {
  if (isFinished(task)) return [];
  const reasons = [];
  if (!askedOnly && task?.status === "in_review") reasons.push(ATTENTION_REASONS.inReview);
  if (hasUnreadComment) reasons.push(ATTENTION_REASONS.inbox);
  if (task?.assignee?.kind === "user") reasons.push(ATTENTION_REASONS.assigned);
  return reasons;
};

/** Whether an unread comment puts the task in Needs you. The narrow rule reads
 *  the cached task and timeline alone, as the inbox does: the board's feed row
 *  is only re-read with the board, so it can still count nothing unread long
 *  after an `tasks` push cached the comment that asked, and still be there
 *  long after Stop watching cached `watched: false`. A bridge that announces
 *  the rule always says `watched`, so the task's own is the watch. */
const hasUnreadAsk = (task, detail, inboxRow, askedOnly) => (askedOnly
  ? task?.watched === true && unreadAsks(task, detail, true).length > 0
  : hasUnreadInboxComment(task, detail, inboxRow));

const attentionReasonsOf = (task, detail, inboxRow, askedOnly) =>
  reasonsOf(task, hasUnreadAsk(task, detail, inboxRow, askedOnly), askedOnly);

/** Why a watched task is in the inbox (#125): the same reasons as Needs you,
 *  read from the cached task records alone. The task's own `watched` is the
 *  watch, so no feed row is consulted. None for a task nobody watches. */
export const watchedTaskReasons = (task, detail, askedOnly = false) =>
  task?.watched === true ? reasonsOf(task, unreadAsks(task, detail, askedOnly).length > 0, askedOnly) : [];

/** One task's attention, with every reason available to a Dashboard row.
 *  Held by a known agent wins for list placement, but the reasons are retained so the
 *  Dashboard can still explain what needs the user's look. */
export function taskAttention(task, {
  knownAgents = new Map(), projectAgent = null, detail = null, inboxRow = null, askedOnly = false,
} = {}) {
  const holdingAgent = holdingAgentOf(task, knownAgents, projectAgent);
  const reasons = attentionReasonsOf(task, detail, inboxRow, askedOnly);
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
export function attentionGroups(tasks, { feed = null, projectKey = "", detailById = new Map(), askedOnly = false } = {}) {
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
      askedOnly,
    });
    groups.attentionById.set(task.id, attention);
    groups[attentionGroupOf(attention)].push(task);
  }
  return groups;
}
