// A retention floor is authoritative deletion, scoped by the caller to one
// thread generation. Keep it monotonic so an older in-flight page cannot put
// removed rows back into the cache. Creation sequence decides retention;
// updating an old row does not make it a newly retained item.

export const threadHistoryStart = (page) => Number(page?.history_start_sequence || 0);

const mismatchedGeneration = (window, page) =>
  !!window.thread_id && !!page?.thread_id && window.thread_id !== page.thread_id;
const retainedItem = (item, floor) =>
  item.data?.provisional === true || Number(item.data?.sequence || 0) >= floor;
const historyRemainsAbove = (first, floor, window) =>
  !!first && Number(first.data.sequence) > floor && window.olderItemsRemain === true;

export function trimThreadHistory(window, page) {
  if (!window || mismatchedGeneration(window, page)) return window;
  const floor = Math.max(Number(window.historyStartSequence || 0), threadHistoryStart(page));
  if (!floor) return window;
  const items = (window.items || []).filter((item) => retainedItem(item, floor));
  const first = items.find((item) => item.data?.provisional !== true);
  const { repairThrough, ...retained } = window;
  return {
    ...retained,
    ...(first && repairThrough ? { repairThrough } : {}),
    items,
    historyStartSequence: floor,
    activityDigests: (window.activityDigests || []).filter((digest) => digest.from_sequence >= floor),
    deliveredSequence: Math.max(Number(window.deliveredSequence || 0), floor - 1),
    olderItemsRemain: historyRemainsAbove(first, floor, window),
  };
}
