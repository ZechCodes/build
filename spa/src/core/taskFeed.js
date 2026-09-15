// One board.list-feed poller per device, and one snapshot out of all of them —
// for the surfaces that live on every page (the sidebar, the nav badge); views
// keep their own detail polls. The plan/run split means the feed carries both
// collections; consumers read `plans` and `runs` separately.
//
// The reading of a device's wire and the merge are pure and live in
// core/feedMerge.js. What is here is the polling: which contexts answer, what
// their answers are kept in, and who is told when one of them moves.

import { App } from "../app.js";
import { canAnswer, contextFor, liveContexts } from "./deviceContexts.js";
import { watchChanges } from "./changeEvents.js";
import { liveFeedSnapshot, mergeFeeds } from "./feedMerge.js";
import { readCached } from "./localCache.js";

const subscribers = new Set();
const byDevice = new Map(); // deviceId → that device's last snapshot
const watchers = new Map(); // deviceId → the board watcher polling it
let cadenceMs = null; // the interval the feed is running at, or null while stopped

/** Subscribe to feed snapshots ({items, plans, runs, externalWorktrees,
 *  pending, projects, primaryChanges, devices}); the current snapshot (if any)
 *  is delivered immediately. Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (byDevice.size) fn(merged());
  return () => subscribers.delete(fn);
}

const merged = () => mergeFeeds(byDevice, App.devices.map((device) => device.id));

/** Hand every subscriber the merge as it stands, with nothing asked of any
 *  bridge. The polls deliver after they read; this is for the other reason a
 *  surface's answer changes — the home device moved, so the slice each
 *  here-surface keeps is about a different machine now. */
export function deliverFeed() {
  const snapshot = merged();
  subscribers.forEach((fn) => fn(snapshot));
}

/** The run that owns a project's primary checkout, or null while nobody has
 *  adopted it. The bridge stamps the owner onto the feed's primary-changes
 *  entry, which makes this a READ: a surface can bind to an existing owner
 *  without minting one. A terminal run has let go, so it is reported as null. */
export function primaryRunIdFor(feed, projectKey) {
  const entry = ((feed && feed.primaryChanges) || []).find((e) => e.projectKey === projectKey);
  return (entry && entry.run_id) || null;
}

/** Read one device — if it can be asked anything at all. A machine that is away,
 *  or whose bridge answers in a shape this tab cannot read, is not read: the
 *  rows it last gave stay in the merge, greyed by the rail, until it can answer
 *  again. A context that was retired (or whose scope stopped addressing the
 *  cache) while its answer was in flight has nothing to say about now, so its
 *  answer is dropped rather than merged.
 *
 *  This is the one read that goes out on the session rather than through the
 *  context's caller, so it is also the one that has to wait: a bridge says which
 *  API major it speaks in its greeting, and until that has settled asking it
 *  anything is asking for an answer in a shape this tab may not be able to read.
 *  `context.greeted` is that machine's greeting and no other's (connection.js),
 *  so a slow bridge holds up its own device and nobody else's. */
async function tick(context) {
  const greeting = context?.greeted;
  await greeting;
  // A reconnect landed while this one waited: what to wait for now is the
  // greeting of the session the device is on, and the watcher asks again.
  if (greeting !== context?.greeted) return;
  if (!canAnswer(context) || !context.active()) return;
  try {
    const [board, projectList, workspaceList] = await Promise.all([
      context.call("board.list"),
      context.call("project.list"),
      // A workspace-list failure must not take that device's board and agent
      // rails down with it — a bridge that does not serve the verb still feeds
      // the board — so it answers none and the next tick asks again.
      Promise.resolve(context.call("workspace.list")).catch(() => ({ workspaces: [] })),
    ]);
    if (!context.active()) return;
    byDevice.set(context.deviceId, liveFeedSnapshot(board, projectList, workspaceList, context.deviceId));
    deliverFeed();
  } catch {
    /* offline / transient — the next tick retries */
  }
}

// Refocusing a tab refreshes immediately instead of waiting out the interval —
// the visible counterpart of hidden tabs skipping their ticks.
const onVisibilityChange = () => {
  if (!document.hidden) refreshFeed();
};

/** The last snapshot the syncer persisted for a device, painted while that
 *  bridge is still being asked. Marked `cached: true` so the sync layer does not
 *  treat its own echo as news; a live answer that gets there first wins
 *  outright. */
async function seedDeviceFromCache(deviceId) {
  const record = await readCached({ deviceId, entityId: "", kind: "feed" });
  if (!record || byDevice.has(deviceId)) return;
  // Nothing in flight survives a reload: the verbs the last session watched
  // settled long ago, and the live answer names whatever is running now.
  byDevice.set(deviceId, { ...record.value, pending: [], cached: true });
  deliverFeed();
}

/** Every device the account knows paints from its own cache — the rail is the
 *  whole account's, so a device whose session is still opening is not a gap. */
function seedFromCache() {
  return Promise.all(App.devices.map((device) => device.id).filter(Boolean).map(seedDeviceFromCache));
}

/**
 * Poll a device the feed is not already polling: read it once, and register the
 * board watcher that keeps reading it.
 *
 * Idempotent, and a no-op before `startFeed` — the seam a device joining or
 * resuming after the feed started comes in through, so a bridge that arrives
 * late still has something listening for its `board.changed`.
 */
export function joinFeed(context) {
  if (cadenceMs === null || !context || watchers.has(context.deviceId)) return;
  // The feed is the board, so `board.changed` is its event and this interval is
  // the safety poll behind it. It owns its own visible-again catch-up above —
  // which reads whether or not anything was pushed — so the registry leaves
  // that alone rather than reading twice. Board tier (wire spec step 1.6):
  // feed-level state, realtime, foreground — the only kind board scope carries.
  watchers.set(
    context.deviceId,
    watchChanges({
      refresh: () => tick(context),
      intervalMs: cadenceMs,
      deviceId: context.deviceId,
      catchUpOnVisible: false,
      kinds: ["state"],
      mode: "realtime",
    }),
  );
  tick(context);
}

export function startFeed(intervalMs = 2000) {
  stopFeed();
  cadenceMs = intervalMs;
  seedFromCache();
  liveContexts().forEach((context) => joinFeed(context));
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
  }
}

export function stopFeed() {
  watchers.forEach((watcher) => watcher.dispose());
  watchers.clear();
  cadenceMs = null;
  if (typeof document !== "undefined") {
    document.removeEventListener("visibilitychange", onVisibilityChange);
  }
}

/** Forget a device: its watcher stops, its rows leave the merge, and everyone
 *  is told what is left. Called when its context is retired. */
export function dropFeedDevice(deviceId) {
  watchers.get(deviceId)?.dispose();
  watchers.delete(deviceId);
  if (byDevice.delete(deviceId)) deliverFeed();
}

/** Force an immediate refresh (after adding a project, adopting, …) — every
 *  live device, or just the one named. */
export function refreshFeed(deviceId = null) {
  const contexts = deviceId === null ? liveContexts() : [contextFor(deviceId)];
  return Promise.all(contexts.map((context) => tick(context)));
}
