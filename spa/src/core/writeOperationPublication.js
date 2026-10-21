// Publication shares the task cache's admission clock and retained field floors.
// Its canonical record, detail read note and layer retirement commit together.
import { recordKey } from "./idbRecords.js";
import { recordWriteOf } from "./localCache.js";
import { TASK_READ_COUNT_ADDRESS, readsAddress, withWritten } from "./taskReadOrder.js";
import { TRACKER_TASK_KIND, preserveTaskFloors } from "./trackerCache.js";

const isTask = ({ address }) => address.kind === TRACKER_TASK_KIND;

export function publicationReadAddresses(entries) {
  const addresses = entries.flatMap((entry) => isTask(entry)
    ? [entry.address, readsAddress(entry.address), TASK_READ_COUNT_ADDRESS] : [entry.address]);
  return [...new Map(addresses.map((address) => [recordKey(address), address])).values()];
}

export function publicationGuardMatches(entry, held) {
  const { address, guard } = entry;
  if (!guard) return true;
  if (Object.hasOwn(guard, "written") && recordWriteOf(held.get(recordKey(address))) !== guard.written) return false;
  if (!isTask(entry) || !Object.hasOwn(guard, "detailBasis")) return true;
  const note = held.get(recordKey(readsAddress(address)));
  return guard.detailBasis === (note?.value?.rows?.[address.sub] || 0);
}

export function preparePublicationWrites(entry, held) {
  if (!isTask(entry)) return [entry];
  const { address, value } = entry;
  const detail = held.get(recordKey(address));
  const noteAddress = readsAddress(address);
  const note = held.get(recordKey(noteAddress));
  const count = held.get(recordKey(TASK_READ_COUNT_ADDRESS));
  const read = Math.max((Number(count?.value) || 0) + 1, Date.now());
  return [
    { address: TASK_READ_COUNT_ADDRESS, value: read, notify: false },
    { address: noteAddress, value: withWritten(note?.value, [address.sub], read) },
    { ...entry, value: preserveTaskFloors(detail?.value, value) },
  ];
}
