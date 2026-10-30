// Publication shares the task cache's admission clock and retained field floors.
// Its canonical record, detail read note and layer retirement commit together.
import { recordKey } from "./idbRecords.js";
import { recordWriteOf } from "./localCache.js";
import { TASK_READ_COUNT_ADDRESS, readsAddress, withWritten } from "./taskReadOrder.js";
import { TRACKER_TASK_KIND, preserveTaskFloors } from "./trackerCache.js";
import { observeTaskContent, reconcileTaskContent, taskContentFragment, withTaskContentComponents } from "./taskContentAuthority.js";
import { reconcileTaskReadFacts, summaryAddress, summaryTaskReadMutation } from "./taskSummaryCache.js";

const isTask = ({ address }) => address.kind === TRACKER_TASK_KIND;
const taskScope = ({ deviceId, entityId }) => ({ deviceId, projectId: entityId });
const contentFragment = (address) => taskContentFragment(taskScope(address), { watchTaskIds: [address.sub] });

export function publicationReadAddresses(entries) {
  const addresses = entries.flatMap((entry) => isTask(entry)
    ? [entry.address, readsAddress(entry.address), TASK_READ_COUNT_ADDRESS,
      ...publicationSummaryAddresses(entry), ...contentFragment(entry.address).addresses] : [entry.address]);
  return [...new Map(addresses.map((address) => [recordKey(address), address])).values()];
}

function publicationSummaryAddresses(entry) {
  return entry.contentRead
    ? summaryTaskReadMutation(taskScope(entry.address), entry.value.task, entry.contentRead).addresses
    : [summaryAddress(taskScope(entry.address), entry.address.sub)];
}

export function publicationGuardMatches(entry, held) {
  const { address, guard } = entry;
  if (!guard) return true;
  if (Object.hasOwn(guard, "written") && recordWriteOf(held.get(recordKey(address))) !== guard.written) return false;
  if (!isTask(entry) || !Object.hasOwn(guard, "detailBasis")) return true;
  const note = held.get(recordKey(readsAddress(address)));
  return guard.detailBasis === (note?.value?.rows?.[address.sub] || 0);
}

export function preparePublication(entry, held) {
  if (!isTask(entry)) return { writes: [entry], receiptPublished: true };
  const { address, value } = entry;
  const detail = held.get(recordKey(address));
  const fragment = contentFragment(address);
  const shared = fragment.addresses.map((target) => held.get(recordKey(target)));
  if (!entry.contentRead) return { writes: [], reason: "missing-content-authority" };
  const scope = taskScope(address), context = fragment.context(shared);
  const summary = summaryTaskReadMutation(scope, value.task, entry.contentRead);
  const normalized = summary.reduce(summary.addresses.map((target) => held.get(recordKey(target))), {
    context, headerBarrier: detail?.value?.task, ordinaryRead: entry.contentRead,
  });
  if (!normalized) return { writes: [], reason: "missing-content-authority" };
  const { content, facts } = selectTaskPublication(entry, detail, context, normalized[0]?.task);
  if (!content.task) return { writes: [], reason: "missing-content-authority" };
  const task = withTaskContentComponents(facts.task, content.task);
  const receiptPublished = completeTaskReceipt(content, facts);
  // Independently admitted ordinary fields can paint while watch authority is
  // unknown. They do not replace history, advance its note, or carry a receipt.
  const canonical = receiptPublished ? taskPublicationWrites(entry, held, { ...value, task })
    : [{ address, value: preserveTaskFloors(detail?.value, { ...detail?.value, task }), publication: [] }];
  return { receiptPublished, reason: receiptPublished ? undefined : "missing-complete-authority", writes: [...canonical,
    ...fragmentWrites(summary.addresses, normalized, true),
    ...fragmentWrites(fragment.addresses, fragment.finish(shared, { retire: content.retired }))] };
}

function selectTaskPublication(entry, detail, context, headerBarrier) {
  const { address, value, contentRead } = entry;
  const heldTask = detail?.value?.task;
  const candidate = observeTaskContent(value.task, { contentRead, coverage: "full" });
  const content = reconcileTaskContent(heldTask, candidate, {
    context, address, heldWritten: recordWriteOf(detail), headerBarrier,
  });
  const facts = reconcileTaskReadFacts(taskScope(address), heldTask, value.task, {
    context, contentRead, markerPolicy: "legacy", watchGuard: headerBarrier,
  });
  return { content, facts };
}

function completeTaskReceipt(content, facts) {
  // A complete receipt needs ordinary content and valid selected watch lineage.
  // Optional summary-only counters remain independent refresh obligations.
  return content.acceptedHeader && content.acceptedBody && !content.needsFullRefresh && facts.watchCertain;
}

function fragmentWrites(addresses, values, notify = false) {
  return values.flatMap((value, index) => value == null ? [] : [{ address: addresses[index], value, notify }]);
}

function taskPublicationWrites(entry, held, value) {
  const { address } = entry;
  const detail = held.get(recordKey(address));
  const noteAddress = readsAddress(address);
  const note = held.get(recordKey(noteAddress));
  const count = held.get(recordKey(TASK_READ_COUNT_ADDRESS));
  const read = Math.max((Number(count?.value) || 0) + 1, Date.now());
  return [
    { address: TASK_READ_COUNT_ADDRESS, value: read, notify: false },
    { address: noteAddress, value: { ...note?.value, ...withWritten(note?.value, [address.sub], read) } },
    { ...entry, value: preserveTaskFloors(detail?.value, value) },
  ];
}
