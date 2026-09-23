// Local interaction state uses the same address → write → announcement → read
// path as bridge records. A view owns its address and decides how to paint it.
// The helper never hands a proposed value straight to the painter.

import { readCached, subscribeCache, writeCached } from "./localCache.js";

// IndexedDB cannot finish a new transaction once the document is torn down.
// A page exit puts only its unfinished draft in this tab's synchronous journal;
// the next mount commits that entry to IndexedDB before its first paint.
const pendingKey = (address) => `build.ui.pending:${JSON.stringify([
  address.deviceId || "", address.entityId || "", address.kind || "", address.sub || "",
])}`;
const readPending = (address) => {
  try {
    const raw = globalThis.sessionStorage?.getItem(pendingKey(address));
    return raw === null || raw === undefined ? undefined : JSON.parse(raw);
  } catch { return undefined; }
};
const holdPending = (address, value) => {
  try { globalThis.sessionStorage?.setItem(pendingKey(address), JSON.stringify(value)); } catch { /* Storage may be refused. */ }
};
const releasePending = (address) => {
  try { globalThis.sessionStorage?.removeItem(pendingKey(address)); } catch { /* Storage may be refused. */ }
};

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
  let unresolved;

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
  const journaled = debounceMs > 0 ? readPending(address) : undefined;

  const commit = (value) => {
    revision += 1;
    dirty = true;
    unresolved = value;
    const committedRevision = revision;
    const next = writes.then(async () => {
      await writeCached(address, value);
      await reads;
      // Announcements from earlier writes can arrive after a newer local edit.
      // Only the newest committed value may repaint the active control.
      if (committedRevision !== revision || pending !== undefined) return;
      dirty = false;
      const cached = await read();
      if (cached !== undefined && JSON.stringify(cached) === JSON.stringify(value)) {
        unresolved = undefined;
        releasePending(address);
      }
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
  const flushOnPageExit = () => {
    const unfinished = pending === undefined ? unresolved : pending;
    if (unfinished !== undefined) holdPending(address, unfinished);
    if (pending !== undefined) void flush();
  };
  const flushWhenHidden = () => {
    if (globalThis.document?.visibilityState === "hidden") flushOnPageExit();
  };
  if (debounceMs > 0) {
    globalThis.addEventListener?.("pagehide", flushOnPageExit);
    globalThis.document?.addEventListener?.("visibilitychange", flushWhenHidden);
  }
  const ready = journaled === undefined ? read() : commit(journaled);

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
    settled: () => reads,
    dispose({ flushPending = true } = {}) {
      if (debounceMs > 0) {
        globalThis.removeEventListener?.("pagehide", flushOnPageExit);
        globalThis.document?.removeEventListener?.("visibilitychange", flushWhenHidden);
      }
      if (flushPending) void flush();
      else if (timer) clearTimeout(timer);
      disposed = true;
      unwatch();
    },
  };
}
