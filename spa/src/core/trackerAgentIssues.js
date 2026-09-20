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

// ---- the agent's issues as one of its surfaces ----------------------------
//
// #34. These four questions used to answer a block drawn inline in the
// conversation, and Zech read it as noise: "I had envisioned the issues
// activity to be the same UX as agents/workflows/tasks/shells. So it's only
// visible when the user wants it to be and there's a clear pattern for done
// work."
//
// So they answer a different shape now: one flat list of rows the surfaces
// layer can draw (core/agentSurfacesModel.js), each carrying the state that
// decides whether it sits above the fold or under it.
//
// What "finished" means is wider here than it is for the badge. On an AGENT's
// list, In review is finished: the agent has said the work is ready to be
// looked at and has nothing more to do with it, and leaving it above the fold
// is what made this read as noise. It is still open, and the Issues tab still
// lists it as open — that is a different question, asked by a different
// reader.

export const ISSUE_SURFACE_IN_PROGRESS = "in_progress";
export const ISSUE_SURFACE_ASSIGNED = "assigned";
export const ISSUE_SURFACE_TRACKED = "tracked";
export const ISSUE_SURFACE_IN_REVIEW = "in_review";
export const ISSUE_SURFACE_DONE = "done";
export const ISSUE_SURFACE_CLOSED = "closed";

const IN_REVIEW = "in_review";

/** The order the rows read in: what it is doing, what it is holding, what it
 *  is only following, then the three it is finished with. */
const SURFACE_STATE_ORDER = [
  ISSUE_SURFACE_IN_PROGRESS,
  ISSUE_SURFACE_ASSIGNED,
  ISSUE_SURFACE_TRACKED,
  ISSUE_SURFACE_IN_REVIEW,
  ISSUE_SURFACE_DONE,
  ISSUE_SURFACE_CLOSED,
];

/**
 * Where one issue stands for this agent, as the surfaces layer names states.
 *
 * Closed is asked before the column because it is true wherever the card sits:
 * an issue closed from Backlog is finished, and calling it "assigned" would
 * put work nobody is doing at the top of the list.
 */
function surfaceStateOf(issue, agentId) {
  const group = groupOf(issue, agentId);
  if (group === null) return null;
  if (group === WATCHING_GROUP) return ISSUE_SURFACE_TRACKED;
  if (issue?.state === "closed") return ISSUE_SURFACE_CLOSED;
  if (issue?.status === DONE) return ISSUE_SURFACE_DONE;
  if (issue?.status === IN_REVIEW) return ISSUE_SURFACE_IN_REVIEW;
  return group === WORKING_GROUP ? ISSUE_SURFACE_IN_PROGRESS : ISSUE_SURFACE_ASSIGNED;
}

/**
 * One agent's issues as surface entries: flat, ordered, ready to draw.
 *
 * Most recently moved first within each standing, because this is an activity
 * surface and what moved is the question it answers.
 *
 * Deliberately NOT the raw issue record: an issue's own `state` is open or
 * closed, and the surfaces layer reads `state` as the mark that decides where
 * a row sits. Two meanings on one field is a bug waiting for somebody to read
 * the wrong one.
 */
export function agentIssueEntries(issues, agentId) {
  const entries = [];
  for (const issue of issues || []) {
    const state = surfaceStateOf(issue, agentId);
    if (!state) continue;
    entries.push({
      id: issue.id,
      state,
      number: issue.number ?? null,
      title: issue.title || "",
      status: issue.status || "",
      updated_at: issue.updated_at || null,
    });
  }
  return entries.sort(
    (left, right) =>
      SURFACE_STATE_ORDER.indexOf(left.state) - SURFACE_STATE_ORDER.indexOf(right.state) || byRecency(left, right),
  );
}

/** The open issues this agent holds — not finished, not merely tracked. What a
 *  badge counts when it is asking "is this agent carrying anything". */
export const agentOpenIssueCount = (issues, agentId) =>
  (issues || []).filter((issue) => {
    const group = groupOf(issue, agentId);
    return group === WORKING_GROUP || group === HOLDING_GROUP;
  }).length;
