// M3 writes publish every existing project projection together. A settings
// response alone must not leave the switcher or an open task on an old name.
import { bridgeCapabilities } from "./changeEvents.js";
import { mergeCachedRecordsTogether } from "./localCache.js";
import { deviceProjectsAddress, projectSettingsAddress } from "./settingsRecords.js";
import { admitEnvironmentListWrite, environmentsAddress } from "./environmentCache.js";

export { projectIsArchived } from "./archivedProjectModel.js";
export const projectLists = deviceId => ["projects", "environment-settings-projects"].map(kind => ({ deviceId, entityId: "", kind }));

export function requireEnvironmentAction(deviceId, flag) {
  if (bridgeCapabilities(deviceId).environments?.[flag] !== true) throw new Error("Update Build on this device to use this action.");
}

export function writeEnvironmentProject(deviceId, reply) {
  const changed = reply.project || reply;
  const addresses = [projectSettingsAddress(deviceId, changed.project_id), deviceProjectsAddress(deviceId), ...projectLists(deviceId)];
  return mergeCachedRecordsTogether(addresses, held => {
    const current = held[0]?.value || held.slice(1).flatMap(record => record?.value || []).find(row => row.project_id === changed.project_id);
    const project = { ...current, ...changed };
    return [project, ...held.slice(1).map(record => Array.isArray(record?.value)
      ? record.value.map(row => row.project_id === changed.project_id ? { ...row, ...changed } : row) : null)];
  });
}

export async function writeEnvironmentSetting(deviceId, changed) {
  // Invalidate earlier list pulls before publishing the successful write.
  await admitEnvironmentListWrite(deviceId);
  return mergeCachedRecordsTogether([environmentsAddress(deviceId)], ([held]) => {
    const rows = held?.value || [];
    const exists = rows.some(row => row.environment_id === changed.environment_id);
    return [exists ? rows.map(row => row.environment_id === changed.environment_id ? { ...row, ...changed } : row) : [...rows, changed]];
  });
}

export async function restoreEnvironmentProject(deviceId, projectId, callRpc) {
  requireEnvironmentAction(deviceId, "archive");
  await writeEnvironmentProject(deviceId, await callRpc("project.restore", { project_id: projectId }));
}
