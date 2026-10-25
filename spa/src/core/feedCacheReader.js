// Feed and route projections share changed-address reads of canonical records
// plus local pending layers. A stable record snapshot also shares its view.
import { cachedFeedSnapshotView } from "./cachedRows.js";
import { readFeedCacheRecords } from "./feedCacheRecords.js";
import { AGENT_LINEAGE_SUPPORT_KIND } from "./agentLineageSupport.js";

const views = new WeakMap();

export function feedSnapshotFromRecords(snapshot) {
  if (!views.has(snapshot)) {
    const { roots } = snapshot;
    views.set(snapshot, {
      record: roots.get("feed"),
      namesMakers: roots.get(AGENT_LINEAGE_SUPPORT_KIND)?.value?.namesMakers === true,
      view: cachedFeedSnapshotView(snapshot),
    });
  }
  return views.get(snapshot);
}

export function createFeedCacheReader(deviceId) {
  return { refresh: async () => feedSnapshotFromRecords(await readFeedCacheRecords(deviceId)) };
}
