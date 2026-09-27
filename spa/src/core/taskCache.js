// The task surface's records.
//
// Tasks left the board (bridge `board_list`: "Legacy tasks remain available
// through their direct read APIs, but no longer participate in this active-work
// surface"), so no ordered pass fills them and no push carries their bodies.
// The rule the rest of the client keeps — the cache is the only thing a view
// reads — is kept here by reading through: the surface asks for a record, a
// cold one is filled from the wire on the way, and every paint after that is
// the record's.
//
// What says a record is behind is the `state` push naming the task, or the
// mount: nothing else fills these records, so an approval given from another
// device while the tab was shut is news no later word will ever carry. That is
// when a read is forced; nothing here runs on a timer.
//
// One kind, `task`, with a sub-key per thing the surface holds:
//
//   get            the task itself — its stages' states, its lineage, its goal
//   stages         the stage manifest
//   doc            the plan of a task with no stage manifest at all
//   stage:<id>     one stage's doc
//   stagediff:<id> one stage's stable diff, read when the reader asks for it
//
// A failed read never blanks what is held: the record on disk is the last
// thing that machine said, and it is still the best answer there is. A cold
// read that fails has nothing behind it, so it is raised — the surface has an
// empty state for exactly that.

import { cachedAddresses, deleteCached, readCached, readCachedMany, writeCached } from "./localCache.js";

export const TASK_RECORD_KIND = "task";

/** Where one thing a task holds lives. A route that names no device (a
 *  legacy `#/plan/<id>` or `#/issue/<id>` link) addresses the device-less
 *  slot rather than another machine's. */
export const taskAddress = (deviceId, taskId, sub) => ({
  deviceId: deviceId || "",
  entityId: taskId,
  kind: TASK_RECORD_KIND,
  sub,
});

/** One record exactly as the cache holds it. Views use this on cache
 *  announcements; wire readers never hand their response object to a paint. */
export async function readStoredTaskRecord(deviceId, taskId, sub) {
  return (await readCached(taskAddress(deviceId, taskId, sub)))?.value;
}

function rethrowReadError(error, held) {
  if (held === undefined) throw error;
  throw Object.assign(error, { heldRecord: held });
}

/**
 * One record, read through.
 *
 * Answers the record when it holds one and nothing says it is behind;
 * otherwise `read()` is awaited, written down and answered. `force` is the
 * push saying the task moved.
 */
export async function readTaskRecord({ deviceId, taskId, sub, read, force = false }) {
  const address = taskAddress(deviceId, taskId, sub);
  const heldRecord = await readCached(address);
  const held = heldRecord?.value;
  if (held !== undefined && !force) return held;
  let answer;
  try {
    answer = await read();
  } catch (error) {
    rethrowReadError(error, held);
  }
  // A newer cache writer won the race while this request was in flight. Its
  // record is the one the view must keep; a late response must not roll it
  // backwards.
  const current = await readCached(address);
  if (current?.at !== heldRecord?.at) return current?.value;
  await writeCached(address, answer);
  // Deliberately read back. Even the caller that initiated the fill receives
  // the cache's record, never the pulled payload object.
  return readStoredTaskRecord(deviceId, taskId, sub);
}

/** Whether all of these records are already on disk.
 *
 *  What a mount asks to know which frame it is about to paint. A cold surface
 *  has to read the machine to paint at all, and what it paints is that read. A
 *  warm one paints the records first and reads behind them — the round trip is
 *  what this stage took out of the first frame, and the catch-up is what keeps
 *  the frame from being last week's. */
export async function taskRecordsHeld(deviceId, taskId, subs) {
  const records = await readCachedMany(subs.map((sub) => taskAddress(deviceId, taskId, sub)));
  // Indexed off `subs`, never walked off `records`: a cache that answered
  // nothing hands back an array of holes, and `every` walks past a hole as
  // though it had agreed.
  return subs.every((sub, index) => records[index]?.value !== undefined);
}

/** Everything one task holds, gone: it was deleted, or the reader asked for
 *  a stage's doc to be read again. */
export async function forgetTaskRecords(deviceId, taskId) {
  const held = await cachedAddresses({ deviceId: deviceId || "", entityId: taskId, kind: TASK_RECORD_KIND });
  if (held.length) await deleteCached(held);
}
