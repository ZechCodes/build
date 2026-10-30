// An admitted mounted project owns one bounded counts reader. This producer
// writes authoritative cache totals; the rail's unread follower stays cache-only.
import { createTaskAggregateReader } from "./taskAggregateReads.js";
import { contextFor, deviceContextIdentity, whenGreeted } from "./deviceContexts.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import { onBridgeGreeted, watchChanges } from "./changeEvents.js";
import { tasksPushKinds } from "./trackerPush.js";
import { taskMutationEpoch, taskMutationsPending, subscribeTaskMutations } from "./taskMutationFence.js";
import { trailingRead } from "./trailingRead.js";

// A same-session re-greeting revokes read authority too. Reuse the token until
// one captured component changes, so equality is meaningful for queued reads.
function authorityOf(deviceId, context) {
  let held;
  return () => {
    const session = deviceSession(deviceId), greeting = context?.greeted;
    if (!held || held.session !== session || held.greeting !== greeting) held = { session, greeting };
    return held;
  };
}

export function followProjectTaskCounts({ deviceId, projectId, context }) {
  const identity = deviceContextIdentity(deviceId), watch = deviceWatch(deviceId);
  const authority = authorityOf(deviceId, context);
  let disposed = false, greetingStands = () => false, waitingGreetingAuthority = null;
  const ownsScope = () => !disposed && contextFor(deviceId) === context
    && deviceContextIdentity(deviceId) === identity && context?.active?.() === true;
  const canRead = () => ownsScope() && !watch.away() && Boolean(deviceSession(deviceId))
    && taskMutationsPending(deviceId) === 0;
  const reader = createTaskAggregateReader({
    scope: { deviceId, projectId }, callRpc: (...args) => context.rpc(...args),
    capabilities: () => context?.adapter?.capabilities?.tasks,
    sessionOf: authority, active: () => canRead() && greetingStands(),
    mutationEpoch: () => taskMutationEpoch(deviceId),
  });

  async function read() {
    if (!canRead()) return false;
    const issued = authority();
    waitingGreetingAuthority = issued;
    try {
      const request = await whenGreeted(context, (stands) => {
        if (!canRead() || authority() !== issued) return false;
        greetingStands = stands;
        return reader.refresh();
      });
      return request ? await request.sent : false;
    } finally {
      if (waitingGreetingAuthority === issued) waitingGreetingAuthority = null;
    }
  }

  const run = trailingRead(read, { generationOf: authority });
  let scheduled = null;
  // Synchronous bursts share this microtask. Invalidations while a read is out
  // request one trailing read; errors never request another read themselves.
  function refresh() {
    if (disposed) return Promise.resolve(false);
    if (!scheduled) scheduled = Promise.resolve().then(() => {
      scheduled = null;
      return run();
    }).catch(() => false);
    return scheduled;
  }

  let watcher;
  function bindPush() {
    watcher?.dispose();
    watcher = watchChanges({ deviceId, entity: projectId, kinds: tasksPushKinds(deviceId), mode: "realtime",
      refresh: () => void refresh(),
      onChanges: (items) => { if (items.some((item) => Object.hasOwn(item, "tasks"))) void refresh(); },
    });
  }
  bindPush();

  let lastAuthority = authority(), lastAway = watch.away();
  const stopMoved = watch.moved(() => {
    const nextAuthority = authority(), away = watch.away();
    if (nextAuthority === lastAuthority && away === lastAway) return;
    lastAuthority = nextAuthority; lastAway = away;
    if (!away) void refresh();
  });
  const stopGreeting = onBridgeGreeted((greetedDevice) => {
    if (greetedDevice !== deviceId || !ownsScope()) return;
    bindPush();
    // The opening read already waits for this greeting and owns its result.
    // A replacement greeting has a different authority and starts at once.
    if (waitingGreetingAuthority !== authority()) void refresh();
  });
  const stopMutations = subscribeTaskMutations(deviceId, ({ phase, pending }) => {
    if (phase === "settled" && pending === 0) void refresh();
  });
  void refresh();

  return { refresh, dispose() {
    disposed = true;
    reader.dispose(); watcher?.dispose(); stopMoved(); stopGreeting(); stopMutations();
  } };
}
