/** Whether an agent counts as running: its own loop, or a harness or Build
 * agent in its activity panel. Cached lineage lays those descendants beside
 * the digest as `agents_running`; `working` remains the agent's own loop. */
export const agentIsRunning = (agent) => !!agent && (!!agent.working || agent.agents_running > 0);
