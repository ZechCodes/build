// Local interaction state uses the same address → write → announcement → read
// path as bridge records. A view owns its address and decides how to paint it.
// The helper never hands a proposed value straight to the painter.

import { readCached, subscribeCache, writeCached } from "./localCache.js";

export const uiAddress = ({ deviceId = "", entityId = "", view, kind, sub = "" }) => ({
  deviceId,
  entityId,
  kind: `ui-${kind}`,
  sub: `${view}:${sub}`,
});

/** Watch one local record. `ready` is the mount's cache read; `write` waits for
 * the committed write and its announced readback. Drafts use `schedule`, then
 * `flush` on send or teardown so the last keystroke cannot be stranded. */
export function watchUiState(address, paint, { debounceMs = 0 } = {}) {
  let disposed = false;
  let pending;
  let timer;
  let writes = Promise.resolve();
  let reads = Promise.resolve();
  let revision = 0;
  let dirty = false;

  const read = (mountedRevision = revision) => {
    const next = reads.then(async () => {
      const record = await readCached(address);
      if (!disposed && !dirty && mountedRevision === revision && record) paint(record.value);
      return record?.value;
    });
    reads = next.catch(() => {});
    return next;
  };
  const unwatch = subscribeCache(address, () => { void read(); });
  const ready = read();

  const commit = (value) => {
    revision += 1;
    dirty = true;
    const committedRevision = revision;
    const next = writes.then(async () => {
      await writeCached(address, value);
      await reads;
      // Announcements from earlier writes can arrive after a newer local edit.
      // Only the newest committed value may repaint the active control.
      if (committedRevision !== revision || pending !== undefined) return;
      dirty = false;
      const cached = await read();
      // IndexedDB may be unavailable (private mode). Keep the active control
      // usable for this mount even though nothing can survive a reload there.
      if (!disposed && cached === undefined) paint(value);
    });
    writes = next.catch(() => {});
    return next;
  };
  const flush = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    if (pending === undefined) return writes;
    const value = pending;
    pending = undefined;
    return commit(value);
  };
  const schedule = (value) => {
    pending = value;
    revision += 1; // an in-flight mount read must not erase active typing
    dirty = true;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => { void flush(); }, debounceMs);
  };

  return {
    ready,
    write(value) {
      pending = undefined;
      if (timer) clearTimeout(timer);
      timer = null;
      return commit(value);
    },
    schedule,
    flush,
    dispose({ flushPending = true } = {}) {
      if (flushPending) void flush();
      else if (timer) clearTimeout(timer);
      disposed = true;
      unwatch();
    },
  };
}
