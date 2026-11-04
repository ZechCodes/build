// Columns are project metadata, independent of a journal query's membership.
// Updating them must never make an old whole-list replica look newly read.
import { readCanonicalCached } from "./localCache.js";
import { captureTaskContentRead, mergeTaskContentRecordsTogether } from "./taskContentAuthority.js";
import { tasksAddress } from "./trackerCache.js";

const hasColumns = (value) => Boolean(value?.columns?.length);
const sameLifetime = (read, context) => ["storeGeneration", "scopeGeneration", "retirementEpoch"]
  .every((field) => read.ticket?.[field] && read.ticket[field] === context[field]);

export async function refreshScopedProjectColumns({ deviceId, projectId, callRpc, active }) {
  const address = tasksAddress(deviceId, projectId);
  const held = await readCanonicalCached(address);
  if (!active() || hasColumns(held?.value)) return false;
  const scope = { deviceId, projectId };
  const contentRead = await captureTaskContentRead(scope, { revalidate: "summary" });
  if (!active()) return false;
  const answer = await callRpc("tasks.columns", { project_id: projectId });
  if (!hasColumns(answer) || !active()) return false;
  return mergeTaskContentRecordsTogether(scope, [address], ([current], context) => {
    if (!active() || !sameLifetime(contentRead, context)) return null;
    const value = current?.value;
    return { values: [{ ...value, tasks: value?.tasks || [], columns: answer.columns,
      read_order: value?.read_order || current?.at || -1 }] };
  });
}
