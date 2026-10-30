// Cached task details for list and Dashboard attention. The list supplies the
// task ids and their update stamps; the cache supplies the task and timeline.
// Wire answers only write records. Cache announcements are the sole path by
// which a fetched detail reaches a reader.

import { readCached, readCachedMany, recordWriteOf, subscribeCache } from "./localCache.js";
import { TRACKER_TASK_KIND, taskAddress, taskRecord, writeTaskRecord, captureTaskDetailRead } from "./trackerCache.js";
import { changedSince } from "./trackerModel.js";
import { trailingRead } from "./trailingRead.js";
import { deviceSession } from "./deviceReconnect.js";
import { contextFor, deviceContextIdentity, onDeviceStateChanged } from "./deviceContexts.js";
import { taskFullRefreshKey } from "./taskContentAuthority.js";
import { captureTaskGetRead, dispatchTaskGetRead } from "./taskWatchSnapshotRead.js";
import { readTaskWatchState, taskWatchControlAddress } from "./taskWatchAuthority.js";
import { subscribeTaskMutations, taskMutationsPendingFor } from "./taskMutationFence.js";

const MAX_READS = 4;


/**
 * Follow the details for a project's current tasks. `read()` is synchronous
 * and contains only records actually read from IndexedDB. `updateTasks()`
 * resolves after the first cache scan, while any needed live reads continue in
 * the background. Omit callRpc for a cache-only observer. Call `dispose()`
 * when the pane unmounts.
 */
export function createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc, onChange = () => {} }) {
  const listed = new Map();
  const cached = new Map(); // id -> { at, value }
  const readEpoch = new Map();
  const pending = new Map(); // id -> list update stamp
  const active = new Map();
  const refreshRequested = new Set();
  const refreshTasks = new Map(); // explicitly refreshed tasks can leave the active filter
  const attempted = new Map(); // one failed or stale response per list version
  const controlReadOrigins = new Map(); // admitted id -> queued control owner
  const normalReadDemand = new Set();
  const deferredReads = new Map(); // committed deferral -> captured read owner
  const deferredReadOrigins = new Map(); // queued recovery keeps its owner until dispatch
  const ordinaryScope = { deviceId, projectId };
  let signature = null;
  let generation = 0;
  let readSerial = 0;
  let disposed = false;
  let controlOwner = null, controlSerial = 0, lastControlHint = null;
  let stopControl = () => {};

  const address = (id) => taskAddress(deviceId, projectId, id);
  const nextEpoch = (id) => {
    const epoch = ++readSerial;
    readEpoch.set(id, epoch);
    return epoch;
  };
  const versionOf = (task) => String(task?.updated_at || "");
  function taskForRead(id) {
    const task = listed.get(id);
    const refreshed = refreshTasks.get(id);
    return changedSince(refreshed, { task }) ? refreshed || task : task;
  }
  const stillWanted = (id, version) => !disposed && Boolean(taskForRead(id)) && version === versionOf(taskForRead(id));
  const attemptKey = (id, detail = cached.get(id)?.value) => JSON.stringify([
    versionOf(taskForRead(id)), taskFullRefreshKey(detail?.task, taskForRead(id)),
  ]);
  const detailReads = trailingRead(fetchDetail, { generationOf: () => deviceSession(deviceId) });

  function startRead(id) {
    const reading = detailReads(id);
    active.set(id, reading);
    void reading.finally(() => {
      if (active.get(id) !== reading) return;
      active.delete(id);
      refreshTasks.delete(id);
      pump();
    });
    return reading;
  }

  function pump() {
    if (disposed) return;
    for (const [id, version] of pending) {
      if (active.size >= MAX_READS) break;
      if (active.has(id)) continue;
      pending.delete(id);
      if (!stillWanted(id, version)) continue;
      refreshRequested.delete(id);
      attempted.set(id, attemptKey(id));
      startRead(id);
    }
  }

  async function writeAnswer(id, answer, basis, acceptsRead) {
    if (!acceptsRead()) return;
    if (answer?.task?.id !== id) return;
    // The transaction selects ordinary content and independent read/watch/PR
    // families separately, retaining newer content without dropping new facts.
    await writeTaskRecord(deviceId, projectId, id, taskRecord(answer?.task, answer?.timeline), {
      detailBasis: basis.detailBasis, contentRead: basis.contentRead,
      accept: acceptsRead, onOrdinaryDeferred: () => deferRead(id, acceptsRead),
    });
  }

  function acceptsQueuedRead(id) {
    const origin = controlReadOrigins.get(id), recovery = deferredReadOrigins.get(id);
    const normal = normalReadDemand.has(id);
    controlReadOrigins.delete(id); deferredReadOrigins.delete(id); normalReadDemand.delete(id);
    if (normal) return true;
    if (origin && !currentControlOwner(origin)) return false;
    return !recovery || recovery();
  }

  async function fetchDetail(id) {
    if (!acceptsQueuedRead(id)) return;
    const version = versionOf(taskForRead(id));
    const context = contextFor(deviceId), identity = deviceContextIdentity(deviceId), session = deviceSession(deviceId);
    const acceptsOwner = () => !disposed && contextFor(deviceId) === context && deviceContextIdentity(deviceId) === identity
      && deviceSession(deviceId) === session && (!context || context.active());
    try {
      const [detailBasis, taskRead] = await Promise.all([
        captureTaskDetailRead(deviceId, projectId, id),
        captureTaskGetRead({ address: address(id), call: callRpc, accept: acceptsOwner }),
      ]);
      if (!acceptsOwner() || !stillWanted(id, version)) return;
      const { answer } = await dispatchTaskGetRead(taskRead.request);
      await writeAnswer(id, answer, { detailBasis, contentRead: taskRead.contentRead }, () => acceptsOwner() && stillWanted(id, version));
    } catch {
      // The list remains useful offline. A later list version can retry.
    }
  }

  /** A partial mutation response makes the task current without supplying
   * its timeline. Ask after that response even at the same task version;
   * an outstanding read trails once and retains the mutation read fence. */
  function refresh(id, task = null) {
    return queueRefresh(id, task, null);
  }

  function queueRefresh(id, task, origin) {
    if (disposed || !callRpc) return;
    if (task?.id === id) refreshTasks.set(id, task);
    if (!taskForRead(id)) return;
    if (origin) {
      if (!normalReadDemand.has(id)) {
        controlReadOrigins.set(id, origin); deferredReadOrigins.delete(id);
      }
    } else {
      normalReadDemand.add(id); controlReadOrigins.delete(id); deferredReadOrigins.delete(id);
    }
    return enqueueRefresh(id);
  }

  function enqueueRefresh(id) {
    if (active.has(id)) return startRead(id);
    refreshRequested.add(id);
    pending.set(id, versionOf(taskForRead(id)));
    pump();
  }

  function recoverDeferredRead(id) {
    const acceptsRead = deferredReads.get(id);
    if (!acceptsRead) return;
    if (!acceptsRead() || !taskForRead(id)) { deferredReads.delete(id); return; }
    if (taskMutationsPendingFor(ordinaryScope, id)) return;
    deferredReads.delete(id);
    if (!normalReadDemand.has(id)) {
      deferredReadOrigins.set(id, acceptsRead); controlReadOrigins.delete(id);
    }
    enqueueRefresh(id);
  }

  function deferRead(id, acceptsRead) {
    if (!acceptsRead()) return;
    deferredReads.set(id, acceptsRead);
    // This check also closes settlement between commit and observer delivery.
    recoverDeferredRead(id);
  }

  const stopOrdinaryMutations = subscribeTaskMutations(deviceId, (event) => {
    if (event.phase === "retired" || event.phase === "session") { deferredReads.clear(); return; }
    if (event.task?.projectId !== projectId) return;
    if (event.phase === "settled" && event.task.pending === 0) recoverDeferredRead(event.task.taskId);
  });

  function queueIfBehind(id, detail) {
    if (!callRpc) return;
    const task = listed.get(id);
    if (refreshRequested.has(id)) {
      pending.set(id, versionOf(taskForRead(id)));
      pump();
      return;
    }
    if (!changedSince(task, detail) && !taskFullRefreshKey(detail?.task, task)) {
      pending.delete(id);
      return;
    }
    const version = versionOf(task);
    if (attempted.get(id) === attemptKey(id, detail)) return;
    normalReadDemand.add(id); controlReadOrigins.delete(id); deferredReadOrigins.delete(id);
    pending.set(id, version);
    pump();
  }

  function replaceCached(id, row) {
    const previous = cached.get(id);
    if (row?.value) cached.set(id, row);
    else cached.delete(id);
    return recordWriteOf(previous) !== recordWriteOf(row) || Boolean(previous) !== Boolean(row?.value);
  }

  function accept(id, row, epoch) {
    if (disposed || !listed.has(id) || readEpoch.get(id) !== epoch) return false;
    const changed = replaceCached(id, row);
    if (!row?.value) attempted.delete(id);
    queueIfBehind(id, row?.value);
    return changed;
  }

  async function rereadOne(id) {
    if (!listed.has(id) || disposed) return;
    const epoch = nextEpoch(id);
    const row = await readCached(address(id));
    if (accept(id, row, epoch)) onChange();
  }

  const unsubscribe = subscribeCache({ deviceId, entityId: String(projectId || ""), kind: TRACKER_TASK_KIND }, (changed) => {
    if (changed.sub && listed.has(changed.sub)) void rereadOne(changed.sub);
    else if (!changed.sub) void scan(); // project eviction may remove every detail
  });

  const controlLifetime = () => ({ context: contextFor(deviceId), identity: deviceContextIdentity(deviceId), session: deviceSession(deviceId) });
  const sameControlLifetime = (owner, next = controlLifetime()) => owner?.context === next.context
    && owner?.identity === next.identity && owner?.session === next.session;
  const currentControlOwner = (owner) => !disposed && owner === controlOwner && sameControlLifetime(owner)
    && (!owner.context || owner.context.active());

  async function readWatchControl(owner) {
    const serial = ++controlSerial;
    try {
      const watch = await readTaskWatchState({ deviceId, projectId });
      if (serial !== controlSerial || !currentControlOwner(owner) || !watch.eligible) return;
      const hint = JSON.stringify([watch.controlGeneration, watch.watchEpoch]);
      if (hint === lastControlHint) return;
      lastControlHint = hint;
      // The existing queue owns admission and its four-read bound. A control
      // notification never adds a task which this mounted feed did not admit.
      for (const id of listed.keys()) queueRefresh(id, null, owner);
    } catch { /* Unknown eligibility is not a reason to retry or ask the wire. */ }
  }

  function bindWatchControl() {
    const next = controlLifetime();
    if (disposed || controlOwner && sameControlLifetime(controlOwner, next)) return;
    stopControl(); controlSerial += 1; lastControlHint = null; controlOwner = next;
    stopControl = subscribeCache(taskWatchControlAddress({ deviceId, projectId }), () => void readWatchControl(next));
  }
  bindWatchControl();
  const stopControlLifetime = onDeviceStateChanged(bindWatchControl);

  async function scan() {
    if (disposed) return;
    const current = generation;
    const ids = [...listed.keys()];
    const epochs = ids.map(nextEpoch);
    const rows = await readCachedMany(ids.map(address));
    if (disposed || generation !== current) return;
    let changed = false;
    ids.forEach((id, index) => { if (accept(id, rows[index], epochs[index])) changed = true; });
    if (changed) onChange();
  }

  async function updateTasks(tasks) {
    if (disposed) return;
    const next = new Map((tasks || []).filter((task) => task?.id).map((task) => [task.id, task]));
    const nextSignature = JSON.stringify([...next].map(([id, task]) => [
      id, versionOf(task), taskFullRefreshKey(cached.get(id)?.value?.task, task),
    ]));
    if (signature === nextSignature) return;
    signature = nextSignature;
    generation += 1;
    let removed = false;
    for (const id of listed.keys()) {
      if (next.has(id)) continue;
      cached.delete(id);
      readEpoch.delete(id);
      if (!refreshTasks.has(id)) {
        pending.delete(id);
        refreshRequested.delete(id);
      }
      attempted.delete(id);
      deferredReads.delete(id);
      controlReadOrigins.delete(id); deferredReadOrigins.delete(id); normalReadDemand.delete(id);
      removed = true;
    }
    listed.clear();
    for (const [id, task] of next) listed.set(id, task);
    for (const id of refreshRequested) pending.set(id, versionOf(taskForRead(id)));
    if (removed) onChange();
    await scan();
  }

  return {
    updateTasks,
    refresh,
    read: () => new Map([...cached].map(([id, row]) => [id, row.value])),
    dispose: () => {
      disposed = true;
      generation += 1;
      pending.clear();
      refreshRequested.clear();
      refreshTasks.clear();
      controlReadOrigins.clear(); normalReadDemand.clear();
      deferredReads.clear(); deferredReadOrigins.clear();
      stopOrdinaryMutations();
      unsubscribe();
      stopControl();
      stopControlLifetime();
    },
  };
}
