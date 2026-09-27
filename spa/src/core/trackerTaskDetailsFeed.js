// Cached task details for list and Dashboard attention. The list supplies the
// task ids and their update stamps; the cache supplies the task and timeline.
// Wire answers only write records. Cache announcements are the sole path by
// which a fetched detail reaches a reader.

import { readCached, readCachedMany, subscribeCache } from "./localCache.js";
import { TRACKER_TASK_KIND, taskAddress, taskRecord, writeTaskRecord } from "./trackerCache.js";
import { changedSince } from "./trackerModel.js";

const MAX_READS = 4;


/**
 * Follow the details for a project's current tasks. `read()` is synchronous
 * and contains only records actually read from IndexedDB. `updateTasks()`
 * resolves after the first cache scan, while any needed live reads continue in
 * the background. Call `dispose()` when the pane unmounts.
 */
export function createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc, onChange = () => {} }) {
  const listed = new Map();
  const cached = new Map(); // id -> { at, value }
  const readEpoch = new Map();
  const pending = new Map(); // id -> list update stamp
  const active = new Set();
  const attempted = new Map(); // one failed or stale response per list version
  let signature = null;
  let generation = 0;
  let readSerial = 0;
  let disposed = false;

  const address = (id) => taskAddress(deviceId, projectId, id);
  const nextEpoch = (id) => {
    const epoch = ++readSerial;
    readEpoch.set(id, epoch);
    return epoch;
  };
  const versionOf = (task) => String(task?.updated_at || "");
  const stillWanted = (id, version) => !disposed && listed.has(id) && version === versionOf(listed.get(id));

  function currentRecordWins(current, before, answer) {
    if ((current?.at || 0) > before) return true;
    const heldTask = current?.value?.task;
    const answerTask = answer?.task;
    return Boolean(heldTask && answerTask
      && !changedSince(answerTask, current.value)
      && heldTask.updated_at !== answerTask.updated_at);
  }

  function pump() {
    if (disposed) return;
    for (const [id, version] of pending) {
      if (active.size >= MAX_READS) break;
      if (active.has(id)) continue;
      pending.delete(id);
      if (!stillWanted(id, version)) continue;
      active.add(id);
      attempted.set(id, version);
      void fetchDetail(id, version);
    }
  }

  async function writeAnswer(id, version, answer, before) {
    if (!stillWanted(id, version)) return;
    if (answer?.task?.id !== id) return;
    // A page or another tab may have written a newer detail while this read
    // was in flight. Leave its record in place rather than replacing it.
    const current = await readCached(address(id));
    if (!stillWanted(id, version) || currentRecordWins(current, before, answer)) return;
    await writeTaskRecord(deviceId, projectId, id, taskRecord(answer?.task, answer?.timeline));
  }

  async function fetchDetail(id, version) {
    try {
      const before = cached.get(id)?.at || 0;
      const answer = await callRpc("tasks.get", { task_id: id });
      await writeAnswer(id, version, answer, before);
    } catch {
      // The list remains useful offline. A later list version can retry.
    } finally {
      active.delete(id);
      pump();
    }
  }

  function queueIfBehind(id, detail) {
    const task = listed.get(id);
    if (!changedSince(task, detail)) {
      pending.delete(id);
      return;
    }
    const version = versionOf(task);
    if (attempted.get(id) === version) return;
    pending.set(id, version);
    pump();
  }

  function replaceCached(id, row) {
    const previous = cached.get(id);
    if (row?.value) cached.set(id, row);
    else cached.delete(id);
    return previous?.at !== row?.at || Boolean(previous) !== Boolean(row?.value);
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
    const nextSignature = JSON.stringify([...next].map(([id, task]) => [id, versionOf(task)]));
    if (signature === nextSignature) return;
    signature = nextSignature;
    generation += 1;
    let removed = false;
    for (const id of listed.keys()) {
      if (next.has(id)) continue;
      cached.delete(id);
      readEpoch.delete(id);
      pending.delete(id);
      attempted.delete(id);
      removed = true;
    }
    listed.clear();
    for (const [id, task] of next) listed.set(id, task);
    if (removed) onChange();
    await scan();
  }

  return {
    updateTasks,
    read: () => new Map([...cached].map(([id, row]) => [id, row.value])),
    dispose: () => {
      disposed = true;
      generation += 1;
      pending.clear();
      unsubscribe();
    },
  };
}
