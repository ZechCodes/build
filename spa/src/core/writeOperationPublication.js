// Publication shares the task cache's admission clock and retained field floors.
// Its canonical record, detail read note and layer retirement commit together.
import { recordKey } from "./idbRecords.js";
import { recordWriteOf } from "./localCache.js";
import { TASK_READ_COUNT_ADDRESS, readsAddress, withWritten } from "./taskReadOrder.js";
import { TRACKER_TASK_KIND, preserveTaskFloors } from "./trackerCache.js";
import { acceptsTaskContentRead, observeTaskContent, reconcileTaskContent, taskContentFragment, withTaskContentComponents } from "./taskContentAuthority.js";
import { reconcileTaskReadFacts, summaryAddress, summaryTaskReadMutation } from "./taskSummaryCache.js";
import { taskHistoryReadOf } from "./writeOperationRefresh.js";

const isTask = ({ address }) => address.kind === TRACKER_TASK_KIND;
const taskScope = ({ deviceId, entityId }) => ({ deviceId, projectId: entityId });
const contentFragment = (address) => taskContentFragment(taskScope(address), { watchTaskIds: [address.sub] });

export function taskPublicationEvidenceAddresses(address) {
  return [address, readsAddress(address), summaryAddress(taskScope(address), address.sub), ...contentFragment(address).addresses];
}

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

export function publicationIsActive(entry, accept) {
  const history = taskHistoryReadOf(entry);
  return accept() && (!history || history.active());
}

export function preparePublication(entry, held, accept) {
  if (!isTask(entry)) return { writes: [entry], receiptPublished: true };
  const { address } = entry;
  const detail = held.get(recordKey(address));
  const fragment = contentFragment(address);
  const shared = fragment.addresses.map((target) => held.get(recordKey(target)));
  if (!entry.contentRead) return { writes: [], reason: "missing-content-authority" };
  const context = fragment.context(shared);
  const { summary, normalized } = normalizedTaskPublication(entry, held, context, detail);
  if (!normalized) return { writes: [], reason: "missing-content-authority" };
  const { content, facts } = selectTaskPublication(entry, detail, context, normalized[0]?.task);
  if (!content.task) return { writes: [], reason: "missing-content-authority" };
  const task = withTaskContentComponents(facts.task, content.task);
  const historyWritten = admitsTaskHistory(entry, held, context, accept);
  const receiptPublished = completeTaskReceipt(content, facts, historyWritten);
  const canonical = selectedTaskPublicationWrites(entry, held, { task, historyWritten, receiptPublished });
  return { receiptPublished, historyWritten, reason: receiptPublished ? undefined : "missing-complete-authority", writes: [...canonical,
    ...fragmentWrites(summary.addresses, normalized, true),
    ...fragmentWrites(fragment.addresses, fragment.finish(shared, { retire: content.retired }))] };
}

function normalizedTaskPublication(entry, held, context, detail) {
  const summary = summaryTaskReadMutation(taskScope(entry.address), entry.value.task, entry.contentRead);
  const normalized = summary.reduce(summary.addresses.map((target) => held.get(recordKey(target))), {
    context, headerBarrier: detail?.value?.task, ordinaryRead: entry.contentRead,
  });
  return { summary, normalized };
}

function selectedTaskPublicationWrites(entry, held, { task, historyWritten, receiptPublished }) {
  const detail = held.get(recordKey(entry.address));
  const publication = receiptPublished ? entry.publication : [];
  // History has its own genuine read owner and exact baseline. Ordinary body
  // timestamp selection neither grants nor prevents that history admission.
  const selected = { ...detail?.value, task, ...(historyWritten ? { timeline: entry.value.timeline } : {}) };
  return historyWritten ? taskPublicationWrites({ ...entry, publication }, held, selected)
    : [{ address: entry.address, value: preserveTaskFloors(detail?.value, selected), publication }];
}

function admitsTaskHistory(entry, held, context, accept) {
  const history = taskHistoryReadOf(entry);
  if (!history || history.value !== entry.value || !publicationIsActive(entry, accept)) return false;
  return publicationGuardMatches(entry, held) && acceptsTaskContentRead(entry.contentRead, {
    context, address: entry.address, heldWritten: recordWriteOf(held.get(recordKey(entry.address))),
  });
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

function completeTaskReceipt(content, facts, historyWritten) {
  // No task route may mint a vector without genuinely admitted owned history.
  // Optional summary-only counters remain independent refresh obligations.
  return historyWritten && content.acceptedHeader && content.acceptedBody && !content.needsFullRefresh && facts.watchCertain;
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
