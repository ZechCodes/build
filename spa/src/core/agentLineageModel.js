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

export function agentLineage(members) {
  const children = new Map();
  for (const member of members) {
    const creator = member.agent.created_by;
    if (!creator) continue;
    if (!children.has(creator)) children.set(creator, []);
    children.get(creator).push(member);
  }
  const byId = new Map(members.map((member) => [member.agent.id, member]));
  const rollups = new Map();

  /** One agent's rollup. `visiting` guards a cycle no bridge should write:
   *  an agent met again on its own line counts as not running there. */
  const rollupOf = (agentId, visiting) => {
    if (rollups.has(agentId)) return rollups.get(agentId);
    const member = byId.get(agentId);
    if (!member || visiting.has(agentId)) return NOTHING_RUNNING;
    visiting.add(agentId);
    const subagents = runningSubagents(member.agent);
    const madeRunning = (children.get(agentId) || [])
      .map((child) => ({ child, rollup: rollupOf(child.agent.id, visiting) }))
      .filter(({ rollup }) => rollup.running);
    visiting.delete(agentId);
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
    /** The Build agents this agent made, as members, in the rows' order. */
    createdBy: (agentId) => (agentId && children.get(agentId)) || [],
    /** Whether the agent counts as running, how many agents in its panel
     *  run, and since when the earliest running thing has been going. */
    rollup: (agentId) => rollupOf(agentId, new Set()),
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
    };
  });
}
