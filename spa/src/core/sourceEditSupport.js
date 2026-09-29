// Whether a machine's bridge edits a project source in place (#228), held in
// the cache so Project settings offers the edits before the bridge answers.
//
// The fact is the bridge's: one that serves `project.update_source` names it
// in its greeting. A greeting writes it here; the settings sheet reads it from
// here, never from the live greeting, so a cold reload offers what the bridge
// will take once connected. A machine never greeted in this browser is read
// as an older one, whose sources can only be added and removed.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const SOURCE_EDIT_SUPPORT_KIND = "source-edit-support";

export const sourceEditSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: SOURCE_EDIT_SUPPORT_KIND });

/** Write what a greeted bridge's capabilities say, when that changes it. */
export function rememberSourceEditSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const editsSources = capabilities?.projects?.updateSource === true;
  return mergeCachedAtomically(sourceEditSupportAddress(deviceId), (held) =>
    (held?.editsSources === editsSources ? null : { editsSources }));
}

/** Whether this machine's project sources can be edited in place. */
export async function readSourceEditSupport(deviceId) {
  if (!deviceId) return false;
  const record = await readCached(sourceEditSupportAddress(deviceId));
  return record?.value?.editsSources === true;
}
