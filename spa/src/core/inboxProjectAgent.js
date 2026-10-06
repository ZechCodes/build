// The project agent's entry in the inbox (#103), as a pure model.
//
// One row per project, for the agent you talk to about the project: just the
// project's name — no second line, no Done. It sits among the workspace rows,
// ordered by the #98 rule over the project agent's own conversation (the
// bridge's session summary on that conversation's feed row, since wire 1.28.0),
// and after a day without activity it moves to Recent the way a workspace row
// does. On the projects face it is not a row at all: it is the head of its
// project's block (core/inboxProjects.js). The inbox tally carries the project's
// watched tasks too, the ones no workspace row wears (#104); the expanded
// project head reads the conversation's own unread separately (#380).
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these, and
// the row itself is painted by core/inbox.js with the rest.

import { entityIdOf } from "./entityId.js";
import { PROJECT_AGENT } from "./inbox.js";
import { agentsUnreadCount, freshestRosters } from "./inboxRoster.js";
import { projectNameOf } from "./inboxProjects.js";
import { projectRoute } from "./projectModel.js";

/** How the project agent's row is named in the rail's DOM. */
export const projectAgentEntryKey = (projectKey) => `project-agent:${projectKey}`;

/** The conversation a project holds, as `project.list` names it. */
const conversationIdOf = (project) => project.entity_id || project.run_id || null;

const isoMs = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

const validSession = (row) => Number.isSafeInteger(row?.session_started_ms)
  && Number.isSafeInteger(row?.last_activity_ms)
  && row.session_started_ms <= row.last_activity_ms;

/** When the project agent's conversation began its current session and last
 *  said something. A bridge before 1.28.0 sends no session on the row, so its
 *  own anchor and activity stand in; a project nobody has talked to has
 *  neither. */
function conversationTimes(row) {
  if (validSession(row)) return { anchorMs: row.session_started_ms, lastActivityMs: row.last_activity_ms };
  return { anchorMs: isoMs(row?.anchor), lastActivityMs: isoMs(row?.last_activity) };
}

/** Its state, in the three words every row's dot says. */
function agentState(unreadCount, working) {
  if (unreadCount > 0) return "unread";
  return working ? "working" : "inactive";
}

/** The conversation's feed row, and the freshest roster the board holds for
 *  it — `runs` carries a conversation `items` left out. */
function conversationOf(project, rows, rosterOf) {
  const entityId = conversationIdOf(project);
  if (!entityId) return { entityId: null, row: null, agents: [] };
  const mine = (row) => row.projectKey === project.projectKey && entityIdOf(row) === entityId;
  const row = rows.items.find(mine) || rows.runs.find(mine) || null;
  const agents = rosterOf(project.projectKey, entityId)?.agents || [];
  return { entityId, row, agents };
}

function toProjectAgentEntry(project, rows, rosterOf, taskUnreadOf) {
  const { entityId, row, agents } = conversationOf(project, rows, rosterOf);
  const agentUnread = agents.length ? agentsUnreadCount(agents) : row?.unread_count || 0;
  const unreadCount = agentUnread + taskUnreadOf(project.projectKey);
  const working = agents.length ? agents.some((agent) => agent.working) : !!row?.working;
  const watchedWorking = agents.length ? agents.some((agent) => agent.watched !== false && agent.working) : working;
  const name = projectNameOf(project);
  return {
    key: projectAgentEntryKey(project.projectKey),
    kind: PROJECT_AGENT,
    deviceId: project.deviceId,
    projectId: project.id,
    projectKey: project.projectKey,
    project: name,
    name,
    title: "Project agent",
    entityId,
    state: agentState(unreadCount, working),
    ownUnreadCount: agentUnread,
    unreadCount,
    working,
    watchedWorking,
    muted: false,
    dismissed: false,
    facts: "",
    route: projectRoute(project),
    ...conversationTimes(row),
  };
}

/** One row per project the rail lists, for its project agent.
 *  `taskUnreadOf(projectKey)` is the unread of the project's watched tasks
 *  no workspace row wears (#104, core/taskUnread.js): they count on the
 *  project's own badge like its agent's. */
export function projectAgentEntries(projects = [], items = [], runs = [], taskUnreadOf = () => 0) {
  const rosterOf = freshestRosters([...runs, ...items]);
  const rows = { items, runs };
  return projects.map((project) => toProjectAgentEntry(project, rows, rosterOf, taskUnreadOf));
}
