// Project columns have independent authority from task rows and membership.
// Keep the existing metadata clock address so an upgrade preserves its order.
import { readCachedMany } from "./localCache.js";

export const projectColumnsAddress = (deviceId, projectId) => ({
  deviceId, entityId: String(projectId || ""), kind: "tracker-columns-order",
});

export function withProjectColumns(list, columns) {
  if (!Array.isArray(list?.tasks) || !Array.isArray(columns?.columns)) return list;
  return { ...list, columns: columns.columns, columns_read_order: columns.columns_read_order };
}


/** Display metadata never certifies that a task list has been observed. */
export async function readProjectColumns(deviceId, projectId) {
  const address = projectColumnsAddress(deviceId, projectId);
  const [metadata, list] = await readCachedMany([address, { ...address, kind: "tracker-tasks" }]);
  const columns = metadata?.value?.columns;
  if (Array.isArray(columns)) return columns;
  const embedded = list?.value?.columns;
  return embedded?.length ? embedded : null;
}
