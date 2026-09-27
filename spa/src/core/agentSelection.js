// Which agent the surfaces on screen are speaking to.
//
// The rail owns the choice — its bubble strip IS the agent selector — but the
// rail is not the only thing that talks to an agent. The branch's Changes
// surface reads a conversation for its diff revision and posts review comments
// into it; the task's viewer does the same for its stage docs. All of them
// have to mean the SAME agent as the bubble that is open, or a comment written
// under one conversation lands in another.
//
// So the choice lives here, in one handle the rail writes and the surfaces
// read, and this is the only place a selection becomes wire params.
//
// Pure — no DOM, no wire.

/** Params naming the conversation a call is about, or nothing at all when no
 *  agent is chosen yet: the daemon then answers with the entity's first agent,
 *  which is what every surface before the rail asked for. */
export function agentScope(agentId) {
  return agentId ? { agent_id: String(agentId) } : {};
}

/**
 * A shared handle on the rail's choice.
 *
 * `set` answers whether the choice CHANGED, because that is the question its
 * readers actually have: the state a surface holds per conversation — a thread
 * cursor, a pending review's revision — belongs to the agent it was built
 * against and has to be dropped when a different agent's bubble is opened.
 */
export function createAgentSelection(initial = null) {
  let agentId = initial || null;
  return {
    get: () => agentId,
    set(next) {
      const wanted = next || null;
      if (wanted === agentId) return false;
      agentId = wanted;
      return true;
    },
    scope: () => agentScope(agentId),
  };
}
