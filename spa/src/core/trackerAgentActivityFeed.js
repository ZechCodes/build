// Latest activity for agents shown on the task Dashboard. A feed row names
// the conversation's cache address; the saved window supplies its words. The
// background thread sync owns wire reads, and this reader only follows cache
// writes and re-reads their records.

import { threadCacheAddress } from "./conversationCache.js";
import { entityIdOf } from "./entityId.js";
import { readCached, readCachedMany, subscribeCache } from "./localCache.js";
import { latestCachedAgentActivity } from "./trackerDashboardModel.js";

const keyOf = (address) => JSON.stringify([address.entityId, address.sub]);
const sameAddresses = (left, right) =>
  JSON.stringify([...left].map(([id, address]) => [id, keyOf(address)]))
  === JSON.stringify([...right].map(([id, address]) => [id, keyOf(address)]));
const workingAgents = (row) => (row.agents || []).filter((agent) => agent?.id && agent.working === true);

const addressesFor = (feed, projectKey, deviceId) => {
  const found = new Map();
  for (const row of feed?.items || []) {
    if (row.projectKey !== projectKey) continue;
    const entityId = entityIdOf(row);
    if (!entityId) continue;
    for (const agent of workingAgents(row)) {
      found.set(agent.id, threadCacheAddress({
        deviceId, entityId, agentId: agent.id, conversationId: agent.conversation_id,
      }));
    }
  }
  return found;
};

/** A cache-only reader for the project's currently working agents. */
export function createTrackerAgentActivityFeed({ deviceId, onChange = () => {} }) {
  let addresses = new Map();
  let snippets = new Map();
  let generation = 0;
  let disposed = false;
  const epochs = new Map();

  const accept = (id, record, epoch) => {
    if (disposed || epochs.get(id) !== epoch || !addresses.has(id)) return false;
    const line = latestCachedAgentActivity(record?.value);
    const previous = snippets.get(id) || "";
    if (line) snippets.set(id, line);
    else snippets.delete(id);
    return previous !== line;
  };

  async function reread(id) {
    const address = addresses.get(id);
    if (!address || disposed) return;
    const epoch = (epochs.get(id) || 0) + 1;
    epochs.set(id, epoch);
    if (accept(id, await readCached(address), epoch)) onChange();
  }

  const unsubscribe = subscribeCache({ deviceId, kind: "thread" }, (changed) => {
    for (const [id, address] of addresses) {
      if (address.entityId === changed.entityId && (!changed.sub || address.sub === changed.sub)) void reread(id);
    }
  });

  const dropMissing = (next) => {
    let changed = false;
    for (const id of snippets.keys()) {
      if (next.has(id)) continue;
      snippets.delete(id);
      epochs.delete(id);
      changed = true;
    }
    return changed;
  };

  const scan = async (next, current, changed) => {
    const pairs = [...next];
    const reads = pairs.map(([id]) => {
      const epoch = (epochs.get(id) || 0) + 1;
      epochs.set(id, epoch);
      return epoch;
    });
    const records = await readCachedMany(pairs.map(([, address]) => address));
    if (disposed || generation !== current) return;
    pairs.forEach(([id], index) => { if (accept(id, records[index], reads[index])) changed = true; });
    if (changed) onChange();
  };

  async function updateFeed(feed, projectKey) {
    if (disposed) return;
    const next = addressesFor(feed, projectKey, deviceId);
    if (sameAddresses(addresses, next)) return;
    addresses = next;
    const current = ++generation;
    await scan(next, current, dropMissing(next));
  }

  return {
    updateFeed,
    read: () => new Map(snippets),
    dispose: () => { disposed = true; generation += 1; unsubscribe(); },
  };
}
