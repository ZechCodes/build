// Activity bodies belong to the same retained generation as their parent
// transcript. Observe its cache record, and publish floors learned by exact
// reads through that parent before admitting child bodies.
import { mergeCachedAtomically, readCached, subscribeCache } from "./localCache.js";
import { threadHistoryStart, trimThreadHistory } from "./threadRetention.js";

export const historyGenerationMatches = (thread, threadId) => !threadId || thread?.thread_id === threadId;
export const retainedActivityItems = (items, floor) =>
  (items || []).filter((item) => Number(item.data?.sequence || 0) >= floor);

export function createThreadHistoryGuard({ address, threadId, onChange = () => {} }) {
  let floor = 0;
  let matches = true;
  let disposed = false;
  let reading = 0;
  const acceptsPage = (page) => !threadId || !page?.thread_id || page.thread_id === threadId;
  const refresh = async () => {
    if (!address) return;
    const version = ++reading;
    const parent = (await readCached(address))?.value;
    if (disposed || version !== reading) return;
    const previousFloor = floor;
    const previouslyMatched = matches;
    matches = historyGenerationMatches(parent, threadId);
    if (matches) floor = Math.max(floor, Number(parent?.historyStartSequence || 0));
    if (floor !== previousFloor || matches !== previouslyMatched) onChange();
  };
  const unwatch = address ? subscribeCache(address, () => void refresh()) : () => {};
  const ready = refresh();
  return {
    ready,
    acceptsPage,
    floor: () => floor,
    filter: (items) => matches ? retainedActivityItems(items, floor) : [],
    accepts: (parent, value) => historyGenerationMatches(parent, threadId)
      && retainedActivityItems(value.items, Number(parent?.historyStartSequence || 0)).length === value.items.length,
    async observe(page) {
      if (!acceptsPage(page) || !threadHistoryStart(page) || disposed) return;
      if (!address) { floor = Math.max(floor, threadHistoryStart(page)); return; }
      await mergeCachedAtomically(address, (parent) =>
        !disposed && historyGenerationMatches(parent, threadId)
          ? trimThreadHistory(parent, { ...page, ...(threadId ? { thread_id: threadId } : {}) }) : null);
      await refresh();
    },
    dispose() { disposed = true; unwatch(); },
  };
}
