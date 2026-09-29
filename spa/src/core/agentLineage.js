// Who made whom among a project's agents, read off this device's cached rows
// (#216).
//
// It asks the bridge for nothing. Every work item's row record carries its
// agents' digests, each naming the agent that made it (`created_by`); the
// sync layer rewrites those records on every pull and push, so listening to
// the device's row records gives a live answer with no read of its own. A
// cold or offline start answers off whatever rows are on disk.
//
// Nothing here knows which agent is in focus: the rail asks per agent at
// paint time (core/agentLineageModel.js holds the answers).

import { subscribeCache } from "./localCache.js";
import { ROW_RECORD_KIND, cachedFeedView } from "./cachedRows.js";
import { agentLineage, buildAgentEntries, lineageMembers, withRollup } from "./agentLineageModel.js";

/**
 * Mount the reader. `onChanged` is called whenever a read lands, and the rail
 * repaints what it drew from the answers.
 */
export function mountAgentLineage({ deviceId, projectId, onChanged } = {}) {
  const state = { lineage: null, disposed: false, generation: 0 };
  const named = Boolean(deviceId) && Boolean(projectId);

  async function reread() {
    const generation = ++state.generation;
    const { items } = await cachedFeedView(deviceId);
    if (state.disposed || generation !== state.generation) return;
    state.lineage = agentLineage(lineageMembers(items, { projectId }));
    onChanged?.();
  }

  const unsubscribe = named && subscribeCache({ deviceId }, (address) => {
    if (address?.kind === ROW_RECORD_KIND) void reread();
  });

  if (named) void reread();

  return {
    /** The Build agents this agent made, as Agents-surface entries, or none
     *  at all — which keeps the pill away from an agent that made nothing. */
    buildAgentsFor: (agentId) => buildAgentEntries(state.lineage, agentId),
    /** The agent with how many agents in its panel run laid beside it. */
    decorate: (agent) => withRollup(agent, state.lineage),
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
    },
  };
}
