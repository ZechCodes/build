// What one agent's issues are, grouped the way a reader of its conversation
// wants them.
//
// Four groups, in this order: what it is working on, what it is holding, what
// it has finished, and what it is only watching. The order is the reader's
// question order — "what is this agent doing" first, "what did it do" last —
// and the two at the bottom are collapsed because they are history and
// background rather than news.
//
// An empty group is not drawn at all. A group with a heading and nothing under
// it tells the reader there is a category they have none of, which is not
// something anybody needs to be told repeatedly.
//
// This is also the grouping the workspace's issues view uses per agent (#16),
// which is why it takes an agent id rather than reading one: the same four
// questions asked of each agent in a workspace give that view its sections.
//
// No DOM, no app imports.

/** The column an issue is being worked in. The one status that means "this is
 *  happening now" rather than "this is somewhere on the board". */
const IN_PROGRESS = "in_progress";

/** The column that means the work is over. Independent of open/closed — an
 *  issue can be closed anywhere on the board, and can sit in Done still open —
 *  so both are asked, and either one makes it history. */
const DONE = "done";

export const WORKING_GROUP = "working";
export const HOLDING_GROUP = "holding";
export const FINISHED_GROUP = "finished";
export const WATCHING_GROUP = "watching";

/** What each group is called, and whether it opens collapsed. The two that
 *  collapse are the two that are not news: finished work, and work this agent
 *  is only following. */
const GROUP_COPY = Object.freeze({
  [WORKING_GROUP]: { label: "In progress", collapses: false },
  [HOLDING_GROUP]: { label: "Assigned", collapses: false },
  [FINISHED_GROUP]: { label: "Done", collapses: true },
  [WATCHING_GROUP]: { label: "Tracking", collapses: true },
});

/** The order they are drawn in, which is the order the questions are asked. */
export const AGENT_ISSUE_GROUPS = [WORKING_GROUP, HOLDING_GROUP, FINISHED_GROUP, WATCHING_GROUP];

/** Whether this issue names this agent as its assignee. The tagged actor shape
 *  is the wire's (core/trackerModel.js); only the `agent` kind can be one of
 *  these, since a project agent and the user are not agents of a workspace. */
export const assignedTo = (issue, agentId) =>
  Boolean(agentId) && issue?.assignee?.kind === "agent" && issue.assignee.agent_id === agentId;

/**
 * Whether this agent is following the issue.
 *
 * `trackers` is the list #13 puts on the record. A bridge that predates it
 * sends no such field, and every issue then reads as untracked — which is the
 * right answer for a bridge with no tracking in it, and means the Tracking
 * group simply never appears rather than the entry breaking.
 */
export const tracks = (issue, agentId) =>
  Boolean(agentId) && (issue?.trackers || []).includes(agentId);

/** Whether the work is over: closed anywhere, or sitting in Done. Both, because
 *  the two are independent and either one makes the issue history to a reader
 *  looking at what an agent is doing now. */
export const isFinished = (issue) => issue?.state === "closed" || issue?.status === DONE;

/**
 * Which group an issue falls in for this agent, or null for one that is
 * neither assigned to it nor tracked by it.
 *
 * Assignment wins over tracking: the assignee is tracked automatically (#13),
 * so almost every assigned issue is also a tracked one, and showing it twice
 * would double every row. Watching is therefore what this agent follows and
 * does NOT hold.
 */
export function groupOf(issue, agentId) {
  if (assignedTo(issue, agentId)) {
    if (isFinished(issue)) return FINISHED_GROUP;
    return issue?.status === IN_PROGRESS ? WORKING_GROUP : HOLDING_GROUP;
  }
  return tracks(issue, agentId) ? WATCHING_GROUP : null;
}

/** When an issue last moved, as a number. An issue with no readable stamp
 *  sorts oldest rather than throwing the order out: 0 is "unknown", not "now". */
const movedAt = (issue) => {
  const at = Date.parse(issue?.updated_at || "");
  return Number.isFinite(at) ? at : 0;
};

/**
 * Most recently moved first, and by number within a tie.
 *
 * NOT the Issues tab's order. That list answers newest-filed first because it
 * is a catalogue of the project; this is an activity entry, where the question
 * is what has moved, and each card wears the time it moved for exactly that
 * reason. The number breaks ties so two issues touched in the same write still
 * have one order every repaint agrees on.
 */
export const byRecency = (left, right) =>
  movedAt(right) - movedAt(left) || Number(right?.number || 0) - Number(left?.number || 0);

/**
 * One agent's issues, grouped and ordered, with the empty groups left out.
 *
 * `issues` is the project's whole list as the cache holds it
 * (core/trackerCache.js) — filtering per agent here rather than asking the
 * bridge per agent is what lets this repaint straight off a pushed list with
 * no read at all. When #13's `issues.for_agent` lands, the same grouping runs
 * over its answer instead; nothing about these four questions changes.
 */
export function agentIssueGroups(issues, agentId) {
  const byGroup = new Map(AGENT_ISSUE_GROUPS.map((id) => [id, []]));
  for (const issue of issues || []) {
    const group = groupOf(issue, agentId);
    if (group) byGroup.get(group).push(issue);
  }
  return AGENT_ISSUE_GROUPS.filter((id) => byGroup.get(id).length).map((id) => ({
    id,
    label: GROUP_COPY[id].label,
    collapses: GROUP_COPY[id].collapses,
    issues: byGroup.get(id).sort(byRecency),
  }));
}

/** How many issues this agent has anything to do with — what a count beside a
 *  collapsed entry says. Counts every group, because a reader deciding whether
 *  to open it wants the whole weight rather than the visible part. */
export const agentIssueCount = (issues, agentId) =>
  (issues || []).filter((issue) => groupOf(issue, agentId) !== null).length;

/** The open issues this agent holds — not finished, not merely tracked. What a
 *  badge counts when it is asking "is this agent carrying anything". */
export const agentOpenIssueCount = (issues, agentId) =>
  (issues || []).filter((issue) => {
    const group = groupOf(issue, agentId);
    return group === WORKING_GROUP || group === HOLDING_GROUP;
  }).length;
