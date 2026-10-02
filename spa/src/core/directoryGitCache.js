// A directory read can invalidate records measured from the same Git state.
// Keep the update and those invalidations together, guarded against late reads.
import { mergeCachedRecordsTogether, recordWriteOf } from "./localCache.js";

export async function writeDirectoryGit(address, value, { before, active, staleKinds = [], guarded = true }) {
  const addresses = [address, ...staleKinds.map((kind) => ({ ...address, kind }))];
  await mergeCachedRecordsTogether(addresses, (records) => {
    if (!active() || (guarded && recordWriteOf(records[0]) !== recordWriteOf(before))) return records.map(() => null);
    return [value, ...records.slice(1).map((record) => record?.value ? { ...record.value, stale: true } : null)];
  });
}
