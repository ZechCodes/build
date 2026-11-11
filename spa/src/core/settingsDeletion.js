// Delete attempts outlive sheets and failed replies. The bridge resumes its
// saved deletion by project identity and mode; these verbs reject frame write
// metadata and answer synchronously rather than using ack/confirm.
import { updateUiRecord, readUiRecord } from "./localUiStore.js";
import { environmentAction, projectLists } from "./environmentWrites.js";
import { DELETE_CACHED_RECORD, mergeCachedRecordsTogether } from "./localCache.js";
import { deviceProjectsAddress, projectSettingsAddress } from "./settingsRecords.js";
import { admitEnvironmentListWrite, environmentsAddress } from "./environmentCache.js";

export async function deleteFromSettings({ deviceId, method, params, callRpc }) {
  const entityId = params.project_id || params.environment_id;
  const address = { deviceId, entityId, kind: "ui-settings-delete", sub: method };
  // One transaction chooses the original intent, including across tabs.
  await updateUiRecord(address, held => held || { operationId: crypto.randomUUID(), params });
  const attempt = (await readUiRecord(address))?.value;
  if (!attempt) throw new Error("Could not save this deletion. Try again.");
  const flag = method === "project.delete" ? "deleteProject" : "deleteEnvironment";
  await environmentAction(deviceId, flag, () => callRpc(method, attempt.params, { operationId: attempt.operationId }));
  if (method === "project.delete") await removeDeletedProject(deviceId, attempt.params.project_id);
  else await removeDeletedEnvironment(deviceId, attempt.params.environment_id);
}

async function removeDeletedEnvironment(deviceId, environmentId) {
  await admitEnvironmentListWrite(deviceId);
  await mergeCachedRecordsTogether([environmentsAddress(deviceId)], ([held]) => [
    (held?.value || []).filter(row => row.environment_id !== environmentId),
  ]);
}

async function removeDeletedProject(deviceId, projectId) {
  await admitEnvironmentListWrite(deviceId);
  const lists = [deviceProjectsAddress(deviceId), ...projectLists(deviceId)];
  await mergeCachedRecordsTogether([...lists, environmentsAddress(deviceId), projectSettingsAddress(deviceId, projectId)], held => [
    ...held.slice(0, lists.length).map(record => Array.isArray(record?.value)
      ? record.value.filter(row => (row.project_id || row.id) !== projectId) : null),
    held[lists.length]?.value?.map(row => ({ ...row, project_ids: row.project_ids?.filter(id => id !== projectId) })) ?? null,
    // Announce the removal now, fencing settings reads already in flight.
    DELETE_CACHED_RECORD,
  ]);
}
