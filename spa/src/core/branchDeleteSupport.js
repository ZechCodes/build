// Whether a machine's bridge deletes the branch when Done asks it to (#87),
// held in the cache so the branch surface and the inbox say what Done will do
// before the bridge answers.
//
// The fact is the bridge's: one that honours `action: "delete"` on
// `branch.finish` announces `branches.finishDelete`. An older bridge takes the
// same word and drops it, removing the checkout and keeping the branch. A
// greeting writes it here; the confirmation and the Done control read it from
// here, never from the live greeting, so a cold reload promises what the
// bridge will do once connected. A machine never greeted in this browser is
// read as an older one: Build does not promise a deletion it has not seen
// the bridge offer.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const BRANCH_DELETE_KIND = "branch-delete";

export const branchDeleteAddress = (deviceId) => ({ deviceId, entityId: "", kind: BRANCH_DELETE_KIND });

/** Write what a greeted bridge's capabilities say, when that changes it. */
export function rememberBranchDelete(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const deletes = capabilities?.branches?.finishDelete === true;
  return mergeCachedAtomically(branchDeleteAddress(deviceId), (held) =>
    (held?.deletes === deletes ? null : { deletes }));
}

/** Whether Done on this machine deletes the branch. */
export async function readBranchDelete(deviceId) {
  if (!deviceId) return false;
  const record = await readCached(branchDeleteAddress(deviceId));
  return record?.value?.deletes === true;
}
