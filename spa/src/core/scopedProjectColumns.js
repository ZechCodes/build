// Columns are project metadata, independent of a journal query's membership.
// Updating them must never make an old whole-list replica look newly read.
import { captureTaskContentRead, mergeTaskContentRecordsTogether } from "./taskContentAuthority.js";
import { tasksAddress } from "./trackerCache.js";

const hasColumns = (value) => Boolean(value?.columns?.length);
const sameLifetime = (read, context) => ["storeGeneration", "scopeGeneration", "retirementEpoch"]
  .every((field) => read.ticket?.[field] && read.ticket[field] === context[field]);

const unchangedColumns = (current, answer) => JSON.stringify(current?.value?.columns) === JSON.stringify(answer.columns);
const columnsOrder = (record) => Number(record?.value?.columns_read_order) || 0;
const supersededColumns = (current, clock, read) => Math.max(columnsOrder(current), columnsOrder(clock)) > read.readOrder;
function withColumns(current, answer, readOrder) {
  const value = current?.value;
  return { ...value, tasks: value?.tasks || [], columns: answer.columns,
    read_order: value?.read_order || current?.at || -1, columns_read_order: readOrder };
}

export async function refreshScopedProjectColumns({ deviceId, projectId, callRpc, active }) {
  if (!active()) return false;
  const address = tasksAddress(deviceId, projectId);
  const scope = { deviceId, projectId };
  const contentRead = await captureTaskContentRead(scope, { revalidate: "summary" });
  if (!active()) return false;
  const answer = await callRpc("tasks.columns", { project_id: projectId });
  if (!hasColumns(answer) || !active()) return false;
  const clock = { ...address, kind: "tracker-columns-order" };
  return mergeTaskContentRecordsTogether(scope, [address, clock], ([current, order], context) => {
    if (!active() || !sameLifetime(contentRead, context) || supersededColumns(current, order, contentRead)) return null;
    const value = unchangedColumns(current, answer) ? null : withColumns(current, answer, contentRead.readOrder);
    return { values: [value, { columns_read_order: contentRead.readOrder }] };
  });
}
