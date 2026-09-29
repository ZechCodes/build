// core/localUiStore.js over the same Map as ./memoryCache.js, for the suites
// that run on fake timers (see memoryCache.js for why IndexedDB cannot serve
// them). One Map serves both here: that the UI store is a database apart from
// the replica is proven against real IndexedDB in localUiStore.test.js, and a
// suite on this double asks about something else.

import { readCached, subscribeCache, wipeCache, writeCached } from "./memoryCache.js";

export const readUiRecord = readCached;
export const writeUiRecord = (address, value) => writeCached(address, value);
export const subscribeUiRecords = subscribeCache;
export const wipeUiRecords = wipeCache;
export const adoptCachedUiRecords = async () => 0;

export async function writeUiRecordIfNewer(address, value, { at }) {
  const held = await readCached(address);
  if (held && !(held.at < at)) return false;
  await writeCached(address, value);
  return true;
}
