// Columns are project metadata, independent of a journal query's membership.
// Updating them must never make an old whole-list replica look newly read.
import { mergeCachedRecordsTogether, readCanonicalCached } from "./localCache.js";
import { tasksAddress } from "./trackerCache.js";

export async function refreshScopedProjectColumns({ deviceId, projectId, callRpc, active }) {
  const address = tasksAddress(deviceId, projectId);
  const held = await readCanonicalCached(address);
  if (!active() || held?.value?.columns?.length) return false;
  const answer = await callRpc("tasks.columns", { project_id: projectId });
  if (!answer?.columns?.length || !active()) return false;
  return mergeCachedRecordsTogether([address], ([current]) => {
    if (!active()) return null;
    const value = current?.value;
    return [{ ...value, tasks: value?.tasks || [], columns: answer.columns,
      read_order: value?.read_order || current?.at || -1 }];
  });
}
