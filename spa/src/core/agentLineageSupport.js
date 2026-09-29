// Whether a machine's bridge names the agent that made each agent (#216,
// #221), held in the cache so the Agents panel lists Build agents before the
// bridge answers.
//
// The fact is the bridge's: one that writes `created_by` on an agent's digest
// announces `agents.createdBy`. A greeting writes it here; the lineage reader
// (core/agentLineage.js) reads it from here with the rows it reads, never
// from the live greeting, so a cold reload lists what it will list once
// connected. A machine never greeted in this browser is read as one whose
// bridge names no makers.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const AGENT_LINEAGE_SUPPORT_KIND = "agent-lineage-support";

export const agentLineageSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: AGENT_LINEAGE_SUPPORT_KIND });

/** Write what a greeted bridge's capabilities say, when that changes it. */
export function rememberAgentLineageSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const namesMakers = capabilities?.agents?.createdBy === true;
  return mergeCachedAtomically(agentLineageSupportAddress(deviceId), (held) =>
    (held?.namesMakers === namesMakers ? null : { namesMakers }));
}

/** Whether agents on this machine name the agent that made them. */
export async function readAgentLineageSupport(deviceId) {
  if (!deviceId) return false;
  const record = await readCached(agentLineageSupportAddress(deviceId));
  return record?.value?.namesMakers === true;
}
