// Project columns have independent authority from task rows and membership.
// Keep the existing metadata clock address so an upgrade preserves its order.
export const projectColumnsAddress = (deviceId, projectId) => ({
  deviceId, entityId: String(projectId || ""), kind: "tracker-columns-order",
});

export function withProjectColumns(list, columns) {
  if (!Array.isArray(list?.tasks) || !Array.isArray(columns?.columns)) return list;
  return { ...list, columns: columns.columns, columns_read_order: columns.columns_read_order };
}
