// One snapshot of every device's board, read off the cache and nothing else —
// for the surfaces that live on every page (the sidebar, the nav badge) and for
// the inbox itself. The plan/run split means the feed carries both collections;
// consumers read `plans` and `runs` separately.
//
// NOTHING HERE READS A BRIDGE. The sync layer is the one reader of the wire
// (core/cacheSync.js) and it writes what it reads; this module holds each
// device's records as a snapshot and re-reads them when the cache announces
// that one of them moved. There is no timer: a push lands in the cache within a
// second for the workspace on screen and within the background cooldown for the
// rest, and the announcement behind that write is what moves the rail.
//
// A device's rows are the `feed` record's list with each row's OWN record laid
// over it. Both, because neither is the whole answer: the board pushes deltas,
// so a workspace made since the last pass rode in on its own `state` item and
// lives as a row record and nowhere else — and a bare checkout nobody has
// claimed names no entity at all, so it has no record of its own and lives in
// the board's list and nowhere else. The legacy collections the board still
// carries (plans, runs, external worktrees, and the lifecycle verbs running
// right now) come from the `feed` record, which is the only place they are.
//
// The merge is pure and lives in core/feedMerge.js.

import { App } from "../app.js";
import { liveContexts } from "./deviceContexts.js";
import { cachedFeedView } from "./cachedRows.js";
import { entityIdOf } from "./entityId.js";
import { mergeFeeds, withoutProject } from "./feedMerge.js";
import { syncDevice } from "./cacheSync.js";
import { DEVICES_ADDRESS, readCached, subscribeCache } from "./localCache.js";

const subscribers = new Set();
const byDevice = new Map(); // deviceId → that device's last snapshot
const watchers = new Map(); // deviceId → the cache subscription behind it
const readers = new Map(); // deviceId → the read queued or running for it
let running = false; // whether startFeed has been called and not stopped
let unwatchDevices = null;

/** The record kinds a device's snapshot is made of. Everything else under a
 *  device — a status, a diff, a conversation — moves several times a second
 *  while an agent works and says nothing about the rail, so an announcement
 *  naming one of those is not a re-read. */
const FEED_KINDS = new Set(["feed", "projects", "workspaces", "row"]);

/** Subscribe to feed snapshots ({items, plans, runs, externalWorktrees,
 *  pending, projects, devices}); the current snapshot (if any) is delivered
 *  immediately. Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (byDevice.size) fn(merged());
  return () => subscribers.delete(fn);
}

const merged = () => mergeFeeds(byDevice, App.devices.map((device) => device.id));

/** Hand every subscriber the merge as it stands, with nothing read. The cache
 *  reads deliver after they land; this is for the other reason a surface's
 *  answer changes — the home device moved, so the slice each here-surface keeps
 *  is about a different machine now. */
export function deliverFeed() {
  const snapshot = merged();
  subscribers.forEach((fn) => fn(snapshot));
}

// ─── One device's snapshot ───────────────────────────────────────────────────

/**
 * One device's view, out of its records.
 *
 * `cached` says this is the boot paint — the records as the last session left
 * them, with nobody having written to them since this tab opened. Whoever is
 * waiting for a device to speak reads it (views/resolving.js), and the sync
 * layer reads it as "not my own echo". A write heard through the cache clears
 * it, whichever tab made the write.
 *
 * `pending` is the lifecycle verbs a board read found running. Those settled
 * long ago in a record nobody has rewritten, so a boot paint carries none; once
 * a pass has written the record, what it says is about now.
 */
function feedSnapshot(held, view, live) {
  if (!held && !view.items.length && !view.projects.length && !view.workspaces.length) return null;
  return {
    ...(held || {}),
    items: rowsOverBoard(held?.items, view.items),
    projects: view.projects,
    workspaces: view.workspaces,
    pending: live ? held?.pending || [] : [],
    cached: !live,
  };
}

/** The board's list, with every row the cache holds a record of replaced by
 *  that record, and the records the list does not name after it. The records
 *  are the fresher of the two — a push rewrites one the moment an agent moves
 *  — and the list is the wider: a row naming no entity has no record. */
function rowsOverBoard(listed, rows) {
  const byEntity = new Map(rows.map((row) => [entityIdOf(row), row]));
  const items = (listed || []).map((item) => byEntity.get(entityIdOf(item)) || item);
  const named = new Set(items.map(entityIdOf).filter(Boolean));
  return [...items, ...rows.filter((row) => !named.has(entityIdOf(row)))];
}

async function readDeviceOnce(deviceId, live) {
  const [record, view] = await Promise.all([
    readCached({ deviceId, entityId: "", kind: "feed" }),
    cachedFeedView(deviceId),
  ]);
  if (!watchers.has(deviceId)) return;
  const snapshot = feedSnapshot(record?.value, view, live);
  if (!snapshot) return;
  byDevice.set(deviceId, snapshot);
  deliverFeed();
}

/** A read per device, one at a time, with the announcements that arrive while
 *  one is running collapsed into the read that follows it. A pass writes the
 *  two lists, the feed and a row per work item; without this the rail would
 *  walk the device's records once per write. */
async function drainReads(deviceId, state) {
  state.busy = true;
  try {
    do {
      state.again = false;
      await readDeviceOnce(deviceId, state.live);
    } while (state.again && watchers.has(deviceId));
  } finally {
    state.busy = false;
  }
}

function readDevice(deviceId, live) {
  const state = readers.get(deviceId) || { busy: false, again: false, live: false, done: null };
  readers.set(deviceId, state);
  state.live = state.live || live;
  if (state.busy) {
    state.again = true;
    return state.done;
  }
  state.done = drainReads(deviceId, state);
  return state.done;
}

/** Whether an announcement is one the rail is made of. An eviction names a
 *  prefix and no kind — everything under an entity went — and that always is. */
const movesTheFeed = (address) => address.kind === undefined || FEED_KINDS.has(address.kind);

/**
 * Read one device's records and hear every later write to them.
 *
 * Idempotent, and a no-op before `startFeed` — the seam a device joining or
 * resuming after the feed started comes in through, so a bridge that arrives
 * late is painted as soon as its pass writes anything.
 */
export function joinFeed(context) {
  return watchDevice(context?.deviceId);
}

function watchDevice(deviceId) {
  if (!running || !deviceId || watchers.has(deviceId)) return undefined;
  watchers.set(
    deviceId,
    subscribeCache({ deviceId }, (address) => {
      if (movesTheFeed(address)) void readDevice(deviceId, true);
    }),
  );
  return readDevice(deviceId, false);
}

/** Every device the account knows paints from its own cache — the rail is the
 *  whole account's, so a device whose session is still opening is not a gap.
 *
 *  Which machines those are is read off disk as well as off the app: the boot
 *  paint runs before `GET /api/devices` has answered, so `App.devices` is
 *  still empty while the list the last read left is not. */
async function watchKnownDevices() {
  const cached = (await readCached(DEVICES_ADDRESS))?.value || [];
  const ids = [...new Set([...cached, ...App.devices].map((device) => device?.id).filter(Boolean))];
  return Promise.all(ids.map((deviceId) => watchDevice(deviceId)));
}

/** Start reading the cache. Answers when every known device's records have been
 *  read, so a caller painting a shell off disk can put the rail's rows in the
 *  same frame as the shell. */
export function startFeed() {
  stopFeed();
  running = true;
  unwatchDevices = subscribeCache(DEVICES_ADDRESS, () => void watchKnownDevices());
  liveContexts().forEach((context) => watchDevice(context.deviceId));
  return watchKnownDevices();
}

export function stopFeed() {
  watchers.forEach((unsubscribe) => unsubscribe());
  watchers.clear();
  readers.clear();
  running = false;
  if (unwatchDevices) unwatchDevices();
  unwatchDevices = null;
}

/** Forget a device: its records stop being heard, its rows leave the merge, and
 *  everyone is told what is left. Called when its context is retired. */
export function dropFeedDevice(deviceId) {
  watchers.get(deviceId)?.();
  watchers.delete(deviceId);
  readers.delete(deviceId);
  if (byDevice.delete(deviceId)) deliverFeed();
}

/**
 * Take one project's rows out of a device's snapshot, and tell everyone.
 *
 * The other half of hiding a project (core/projectHide.js): the records on disk
 * are where a gone machine's rows live between sessions, but the snapshot in
 * memory is what the rail is painting right now — and a hide that only cleared
 * the disk would repaint the block from memory before the reader's finger left
 * the button.
 *
 * Hands back the device's view AS IT STOOD, so the caller can read the entity
 * ids it is about to evict off the rows it just removed. Null when that device
 * has no snapshot here at all.
 */
export function dropFeedProject(deviceId, projectKey) {
  const view = byDevice.get(deviceId);
  if (!view || !projectKey) return null;
  byDevice.set(deviceId, withoutProject(view, projectKey));
  deliverFeed();
  return view;
}

/** Ask for a pass now (after adding a project, adopting, …) — every live
 *  device, or just the one named. The pass is the sync layer's; what it writes
 *  comes back here as an announcement, the same way a push does. */
export function refreshFeed(deviceId = null) {
  const ids = deviceId === null ? liveContexts().map((context) => context.deviceId) : [deviceId];
  return Promise.all(ids.map((id) => syncDevice(id, { fresh: true })));
}
