// Cache-only observation of one explicitly selected aggregate. Network refresh
// belongs to the admitted consumer's controller, never to rendering this value.
import { subscribeCache } from "./localCache.js";
import { readTaskCounts, taskCountsAddress } from "./taskAggregateReads.js";
import { readTaskCountsSupport, taskCountsSupportAddress } from "./taskCountsSupport.js";

export function createTaskCountFollower({ scope: suppliedScope, agentIds = [], onChange = () => {} }) {
  const scope = { ...suppliedScope };
  let standing = null;
  let supported = false;
  let disposed = false;
  let serial = 0;

  async function reread(current) {
    const read = ++serial;
    const [result, renderingSupport] = await Promise.all([readTaskCounts(scope, current.agents), readTaskCountsSupport(scope.deviceId)]);
    if (disposed || standing !== current || read !== serial) return;
    const previous = JSON.stringify([supported, current.result]);
    supported ||= renderingSupport;
    // A failed read or eviction is not an authoritative zero aggregate.
    if (result !== null) current.result = result;
    if (JSON.stringify([supported, current.result]) !== previous) onChange();
  }

  function follow(agents = []) {
    const address = taskCountsAddress(scope, agents);
    if (disposed || standing?.key === address.sub) return;
    standing?.unsubscribe();
    const current = { key: address.sub, agents: JSON.parse(address.sub), result: null, unsubscribe: () => {} };
    standing = current;
    current.unsubscribe = subscribeCache(address, () => void reread(current));
    void reread(current);
  }

  const unsubscribeSupport = subscribeCache(taskCountsSupportAddress(scope.deviceId), () => {
    if (standing) void reread(standing);
  });
  follow(agentIds);
  return {
    read: () => standing?.result ?? null,
    supported: () => supported,
    follow,
    dispose() { disposed = true; serial += 1; standing?.unsubscribe(); unsubscribeSupport(); },
  };
}
