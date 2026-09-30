// Whether a machine's bridge edits a project source in place (#228), and
// whether it keeps a source's base branch in step with its remote (#267),
// held in the cache so Project settings offers both before the bridge
// answers.
//
// The facts are the bridge's: one that serves `project.update_source` names
// it in its greeting, and one that syncs bases names `sources.syncBase`. A
// greeting writes them here; the settings sheet reads them from here, never
// from the live greeting, so a cold reload offers what the bridge will take
// once connected. A machine never greeted in this browser is read as an
// older one, whose sources can only be added and removed.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const SOURCE_EDIT_SUPPORT_KIND = "source-edit-support";

export const sourceEditSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: SOURCE_EDIT_SUPPORT_KIND });

/** Write what a greeted bridge's capabilities say, when that changes it. */
export function rememberSourceEditSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const editsSources = capabilities?.projects?.updateSource === true;
  const syncsBase = capabilities?.projects?.syncBase === true;
  return mergeCachedAtomically(sourceEditSupportAddress(deviceId), (held) =>
    (held?.editsSources === editsSources && held?.syncsBase === syncsBase ? null : { editsSources, syncsBase }));
}

/** Both facts in one read: whether this machine's project sources can be
 *  edited in place, and whether it keeps their base branches in step with
 *  their remotes (and so offers the setting and Sync now). */
export async function readSourceSupport(deviceId) {
  const held = deviceId ? (await readCached(sourceEditSupportAddress(deviceId)))?.value : null;
  return { editsSources: held?.editsSources === true, syncsBase: held?.syncsBase === true };
}

/** Whether this machine's project sources can be edited in place. */
export async function readSourceEditSupport(deviceId) {
  return (await readSourceSupport(deviceId)).editsSources;
}

/** Whether this machine keeps its sources' base branches up to date. */
export async function readSourceSyncSupport(deviceId) {
  return (await readSourceSupport(deviceId)).syncsBase;
}
