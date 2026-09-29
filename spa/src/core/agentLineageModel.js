// Who made whom among a project's agents, and what that makes running (#216).
//
// An agent that asks Build for another agent — `add_workspace_agent`, or
// `assign_task` to a new agent or a new workspace — is named on the new one's
// digest as `created_by`. Its activity panel lists those Build agents beside
// its harness sub-agents, and it counts as running while any agent in that
// panel runs, even with its own loop waiting.
//
// The rollup is transitive, and has to be: it applies to the panel's rows as
// well as to the agent. If A made B and B made C, and C is running, then B's
// row on A's panel says running — so A has a running agent in its panel, and
// by the same rule is running. Stopping at one level would have A's panel
// show a running row under an agent that says it is idle.
//
// Read off the cached rows alone; no DOM, no app imports.

import { agentDisplayName } from "./agentName.js";
import { entityIdOf } from "./entityId.js";
import { conversationRoute } from "./router.js";

const NOTHING_RUNNING = Object.freeze({ running: false, agentsRunning: 0, since: null });

const SUBAGENT_RUNNING_STATE = "running";

/** Every agent the project's rows carry, with where it lives: the entity its
 *  conversation is on and, for a workspace's agent, the workspace. */
export function lineageMembers(rows, { projectId } = {}) {
  return (rows || [])
    .filter((row) => row && Array.isArray(row.agents) && (!projectId || row.project_id === projectId))
    .flatMap((row) => row.agents.filter((agent) => agent && agent.id).map((agent) => ({
      agent,
      kind: row.kind || "",
      entityId: entityIdOf(row),
      workspaceId: row.workspace_id || null,
      workspaceName: row.title || row.name || "",
    })));
}

const runningSubagents = (agent) => (Array.isArray(agent?.surfaces?.subagents) ? agent.surfaces.subagents : [])
  .filter((entry) => entry && entry.state === SUBAGENT_RUNNING_STATE);

const isoOfMs = (ms) => (Number.isFinite(ms) ? new Date(ms).toISOString() : null);

/** The earliest of some moments, as the ISO string it started as. */
function earliest(moments) {
  const known = moments.filter((moment) => Number.isFinite(Date.parse(moment || "")));
  if (!known.length) return null;
  return known.reduce((first, moment) => (Date.parse(moment) < Date.parse(first) ? moment : first));
}

const ownSince = (agent) => (agent.working ? agent.working_time?.since || null : null);

/** Whether an agent's line of makers leads back to itself: a cycle no bridge
 *  should write. Each agent names one maker, so its line is one walk up. */
function onMakerCycle(agentId, byId) {
  const seen = new Set();
  for (let at = byId.get(agentId)?.agent.created_by; at && !seen.has(at); at = byId.get(at)?.agent.created_by) {
    if (at === agentId) return true;
    seen.add(at);
  }
  return false;
}

export function agentLineage(members) {
  const byId = new Map(members.map((member) => [member.agent.id, member]));
  // An agent on a cycle counts as made by no one. Cut there, who made whom is
  // a forest, so every rollup below is the same whichever agent is asked
  // about first (#221).
  const children = new Map();
  for (const member of members) {
    const creator = member.agent.created_by;
    if (!creator || onMakerCycle(member.agent.id, byId)) continue;
    if (!children.has(creator)) children.set(creator, []);
    children.get(creator).push(member);
  }
  const rollups = new Map();

  /** One agent's rollup. */
  const rollupOf = (agentId) => {
    if (rollups.has(agentId)) return rollups.get(agentId);
    const member = byId.get(agentId);
    if (!member) return NOTHING_RUNNING;
    const subagents = runningSubagents(member.agent);
    const madeRunning = (children.get(agentId) || [])
      .map((child) => ({ child, rollup: rollupOf(child.agent.id) }))
      .filter(({ rollup }) => rollup.running);
    const agentsRunning = subagents.length + madeRunning.length;
    const answer = {
      running: !!member.agent.working || agentsRunning > 0,
      agentsRunning,
      since: ownSince(member.agent) || earliest([
        ...subagents.map((entry) => isoOfMs(entry.started_at)),
        ...madeRunning.map(({ rollup }) => rollup.since),
      ]),
    };
    rollups.set(agentId, answer);
    return answer;
  };

  return {
    /** The Build agents this agent made, as members, in the rows' order —
     *  none that sit on a cycle of makers. */
    createdBy: (agentId) => (agentId && children.get(agentId)) || [],
    /** Whether the agent counts as running, how many agents in its panel
     *  run, and since when the earliest running thing has been going. */
    rollup: rollupOf,
  };
}

/** An agent as the rail paints it: its own digest, with how many agents in
 *  its panel are running laid beside `working` — never over it, because
 *  `working` is the agent's own loop, which the composer's interrupt asks
 *  about. The same object when nothing in its panel runs. */
export function withRollup(agent, lineage) {
  if (!agent || !lineage) return agent;
  const rollup = lineage.rollup(agent.id);
  if (!rollup.agentsRunning) return agent;
  return { ...agent, agents_running: rollup.agentsRunning, agents_since: rollup.since };
}

/** Where a Build agent stands on its creator's panel. */
const buildAgentState = (member, rollup) => {
  if (rollup.running) return "running";
  return member.agent.start_error ? "failed" : "idle";
};

/** The Build agents this agent made, as entries on its Agents surface — or
 *  none at all for an agent that made nothing, which keeps the pill away. */
export function buildAgentEntries(lineage, agentId) {
  const made = lineage ? lineage.createdBy(agentId) : [];
  if (!made.length) return null;
  return made.map((member) => {
    const rollup = lineage.rollup(member.agent.id);
    const since = Date.parse(rollup.since || "");
    return {
      id: member.agent.id,
      name: agentDisplayName(member.agent),
      state: buildAgentState(member, rollup),
      started_at: Number.isFinite(since) ? since : null,
      entity_id: member.entityId,
      workspace_id: member.workspaceId,
      workspace_name: member.workspaceName,
      kind: member.kind,
    };
  });
}

/** The pages a Build agent's chat opens on (#221): its workspace's, or the
 *  project's for an agent that has no workspace. */
const CHAT_PAGES = {
  workspace: ({ workspaceId }) => !!workspaceId,
  project: () => true,
};

/** Which page a Build agent's chat opens on, or null when it is on neither —
 *  its row is then not pressable at all, rather than pressable for nothing. */
export const buildAgentChatKind = ({ kind, workspaceId }) => (CHAT_PAGES[kind]?.({ workspaceId }) ? kind : null);

/** The route a press on a Build agent's row goes to: the same conversation
 *  route every other link to an agent's chat is written from. */
export function buildAgentChatRoute({ agentId, kind, workspaceId }, { deviceId, projectId }) {
  const page = buildAgentChatKind({ kind, workspaceId });
  return page ? conversationRoute({ kind: page, projectId, deviceId, workspaceId, agentId }) : null;
}

/** What the lineage answers depend on, as one string: who made whom, where
 *  each lives, and what runs. Two reads with the same print answer alike, so
 *  a row rewritten for anything else (a message, a read cursor) moves nothing
 *  drawn from here. */
export function lineagePrint(members) {
  return JSON.stringify(members.map(({ agent, entityId, workspaceId, workspaceName }) => [
    agent.id, agent.created_by || null, agent.name || null, agent.ordinal || null, !!agent.working,
    agent.working_time?.since || null, agent.start_error || null,
    runningSubagents(agent).map((entry) => entry.started_at ?? null),
    entityId, workspaceId, workspaceName,
  ]));
}
