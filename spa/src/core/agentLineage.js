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
import { agentLineage, buildAgentEntries, lineageMembers, lineagePrint, withRollup } from "./agentLineageModel.js";

/**
 * Mount the reader. `onChanged` is called when a read lands whose answers
 * differ from the last one's, and the rail repaints what it drew from them.
 */
export function mountAgentLineage({ deviceId, projectId, onChanged } = {}) {
  const state = { lineage: null, print: null, disposed: false, reading: false, again: false };
  const named = Boolean(deviceId) && Boolean(projectId);

  /** One read at a time: writes that land during a read ask for one more
   *  after it, however many there were — a pass writes many rows at once. */
  async function reread() {
    if (state.reading) {
      state.again = true;
      return;
    }
    state.reading = true;
    try {
      do {
        state.again = false;
        await readOnce();
      } while (state.again && !state.disposed);
    } finally {
      state.reading = false;
    }
  }

  async function readOnce() {
    const { items } = await cachedFeedView(deviceId);
    if (state.disposed) return;
    const members = lineageMembers(items, { projectId });
    const print = lineagePrint(members);
    if (print === state.print) return;
    state.print = print;
    state.lineage = agentLineage(members);
    onChanged?.();
  }

  let unsubscribe = null;

  return {
    /** Begin reading. Asked for by the rail once its own row has painted: the
     *  lineage reads every row on the device, and a reader first opening a
     *  chat is owed that chat before who made whom. Asking again does nothing. */
    start() {
      if (!named || unsubscribe || state.disposed) return;
      unsubscribe = subscribeCache({ deviceId }, (address) => {
        if (address?.kind === ROW_RECORD_KIND) void reread();
      });
      void reread();
    },
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
