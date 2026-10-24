// Keep the cache's feed records by address. Ordinary announcements reread only
// those records; prefix eviction rebuilds the index. Reads still compose local
// pending layers and receive cross-tab changes through the cache announcer.
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { feedViewFromRecords } from "./cachedRows.js";
import { AGENT_LINEAGE_SUPPORT_KIND } from "./agentLineageSupport.js";

const rootKinds = ["feed", "projects", "workspaces", "project-agents-support", AGENT_LINEAGE_SUPPORT_KIND];
const keyOf = (address) => JSON.stringify([address.entityId || "", address.kind, address.sub || ""]);
const rootAddress = (deviceId, kind) => ({ deviceId, entityId: "", kind });

export function createFeedCacheReader(deviceId) {
  const held = new Map();
  async function refresh(changed = null) {
    const addresses = changed || [
      ...(await cachedAddresses({ deviceId })).filter((address) => address.kind === "row" || address.kind === "project-agents"),
      ...rootKinds.map((kind) => rootAddress(deviceId, kind)),
    ];
    if (!changed) held.clear();
    const records = await readCachedMany(addresses);
    addresses.forEach((address, index) => {
      if (records[index]) held.set(keyOf(address), { address, record: records[index] });
      else held.delete(keyOf(address));
    });
    const rows = [...held.values()].filter(({ address }) => address.kind === "row" || address.kind === "project-agents");
    const recordOf = (kind) => held.get(keyOf(rootAddress(deviceId, kind)))?.record;
    return {
      record: recordOf("feed"),
      namesMakers: recordOf(AGENT_LINEAGE_SUPPORT_KIND)?.value?.namesMakers === true,
      view: feedViewFromRecords(rows.map(({ address }) => address),
        [...rows.map(({ record }) => record), recordOf("projects"), recordOf("workspaces"), recordOf("project-agents-support")]),
    };
  }
  return { refresh };
}
