// Whether a machine's bridge measures its workspaces' sizes when asked (#273),
// held in the cache so the Workspaces tab knows before the bridge answers.
//
// The fact is the bridge's: one that serves `workspace.measure_sizes` names it
// in its greeting. A greeting writes it here; the tab reads it from here,
// never from the live greeting, to decide whether a row with no size yet
// shows the quiet placeholder: a machine that will send the size says so, and
// an older one shows what it has today. A machine never greeted in this
// browser is read as an older one.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const WORKSPACE_SIZE_SUPPORT_KIND = "workspace-size-support";

export const workspaceSizeSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: WORKSPACE_SIZE_SUPPORT_KIND });

/** Write what a greeted bridge's capabilities say, when that changes it. */
export function rememberWorkspaceSizeSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const measuresSizes = capabilities?.workspaces?.measureSizes === true;
  return mergeCachedAtomically(workspaceSizeSupportAddress(deviceId), (held) =>
    (held?.measuresSizes === measuresSizes ? null : { measuresSizes }));
}

/** Whether this machine measures its workspaces' sizes when asked. */
export async function readWorkspaceSizeSupport(deviceId) {
  if (!deviceId) return false;
  const record = await readCached(workspaceSizeSupportAddress(deviceId));
  return record?.value?.measuresSizes === true;
}
