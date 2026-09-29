// Local interaction state uses the same address → write → announcement → read
// path as bridge records, in its own store (core/localUiStore.js) that no
// replica schema bump, sweep or eviction reaches. A view owns its address and
// decides how to paint it. The helper never hands a proposed value straight
// to the painter.

import { readUiRecord, subscribeUiRecords, writeUiRecord, writeUiRecordIfNewer } from "./localUiStore.js";

// IndexedDB cannot finish a new transaction once the document is torn down.
// A page exit puts only its unfinished draft in this tab's synchronous journal;
// the next mount commits that entry to IndexedDB before its first paint.
const PENDING_PREFIX = "build.ui.pending:";
export const UI_JOURNAL_ENTRY_MAX_BYTES = 64 * 1024;
export const UI_JOURNAL_TOTAL_MAX_BYTES = 256 * 1024;
const pendingKey = (address) => `${PENDING_PREFIX}${JSON.stringify([
  address.deviceId || "", address.entityId || "", address.kind || "", address.sub || "",
])}`;
const bytes = (text) => new TextEncoder().encode(text).length;
const pendingEntries = (storage) => Array.from({ length: storage.length }, (_, index) => storage.key(index))
  .filter((key) => key?.startsWith(PENDING_PREFIX));
const clearJournal = (storage) => pendingEntries(storage).forEach((key) => storage.removeItem(key));
const isBinary = (value) => value instanceof ArrayBuffer || ArrayBuffer.isView(value)
  || (globalThis.Blob && value instanceof Blob);
const isBodyKey = (key) => /^(content_b64|content_base64|data_url|dataurl|patch|diff|hunks|file_body|file_content|filecontent)$/i.test(key);
// This small journal is only for interaction state. Neither a file/diff body
// nor a live binary resource belongs in session storage, even briefly.
const hasBody = (value) => {
  const stack = [value];
  const seen = new Set();
  while (stack.length) {
    const item = stack.pop();
    if (!item || typeof item !== "object") continue;
    if (isBinary(item)) return true;
    if (seen.has(item)) continue;
    seen.add(item);
    for (const [key, nested] of Object.entries(item)) {
      if (isBodyKey(key)) return true;
      stack.push(nested);
    }
  }
  return false;
};
const journalTotalBytes = (storage) => pendingEntries(storage).reduce((sum, entryKey) => {
  const held = storage.getItem(entryKey) || "";
  return sum + bytes(entryKey) + (held.length > UI_JOURNAL_TOTAL_MAX_BYTES ? UI_JOURNAL_TOTAL_MAX_BYTES + 1 : bytes(held));
}, 0);
const validEntry = (entry) => entry && Number.isFinite(entry.at) && entry.source
  && Number.isFinite(entry.sequence) && !hasBody(entry.value);
const readPendingFromStorage = (storage, key) => {
  const raw = storage?.getItem(key);
  if (raw === null || raw === undefined) return undefined;
  if (journalTotalBytes(storage) > UI_JOURNAL_TOTAL_MAX_BYTES) {
    clearJournal(storage);
    return undefined;
  }
  if (raw.length > UI_JOURNAL_ENTRY_MAX_BYTES || bytes(raw) > UI_JOURNAL_ENTRY_MAX_BYTES) {
    storage.removeItem(key);
    return undefined;
  }
  const entry = JSON.parse(raw);
  if (validEntry(entry)) return entry;
  storage.removeItem(key);
  return undefined;
};
const readPending = (address) => {
  if (!address.kind?.startsWith("ui-")) return undefined;
  const key = pendingKey(address);
  try {
    return readPendingFromStorage(globalThis.sessionStorage, key);
  } catch {
    try { globalThis.sessionStorage?.removeItem(key); } catch { /* Storage may be refused. */ }
    return undefined;
  }
};
const uiWriterId = () => globalThis.crypto?.randomUUID?.() || `${Date.now()}:${Math.random()}`;
const pendingFor = (address, debounceMs) => (debounceMs > 0 ? readPending(address) : undefined);
const journalFits = (storage, key, raw) => {
  if (raw.length > UI_JOURNAL_ENTRY_MAX_BYTES || bytes(raw) > UI_JOURNAL_ENTRY_MAX_BYTES) return false;
  const otherBytes = pendingEntries(storage).filter((entryKey) => entryKey !== key)
    .reduce((sum, entryKey) => sum + bytes(entryKey) + bytes(storage.getItem(entryKey) || ""), 0);
  return otherBytes + bytes(key) + bytes(raw) <= UI_JOURNAL_TOTAL_MAX_BYTES;
};
const holdPending = (address, value, source, sequence, at) => {
  const key = pendingKey(address);
  try {
    if (!address.kind?.startsWith("ui-") || hasBody(value)) {
      globalThis.sessionStorage?.removeItem(key);
      return;
    }
    const storage = globalThis.sessionStorage;
    const raw = JSON.stringify({ at, source, sequence, value });
    if (!journalFits(storage, key, raw)) {
      storage.removeItem(key);
      return;
    }
    storage.setItem(key, raw);
  } catch {
    try { globalThis.sessionStorage?.removeItem(key); } catch { /* Storage may be refused. */ }
  }
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
  const source = uiWriterId();
  let disposed = false;
  let pending;
  let pendingAt;
  let timer;
  let writes = Promise.resolve();
  let reads = Promise.resolve();
  let revision = 0;
  let dirty = false;
  let unresolved;
  let unresolvedAt;

  const read = (mountedRevision = revision) => {
    const next = reads.then(async () => {
      const record = await readUiRecord(address);
      if (!disposed && !dirty && mountedRevision === revision && record) paint(record.value);
      return record?.value;
    });
    reads = next.catch(() => {});
    return next;
  };
  const unwatch = subscribeUiRecords(address, () => { void read(); });
  const journaled = pendingFor(address, debounceMs);

  const commit = (value, editAt = Date.now()) => {
    revision += 1;
    dirty = true;
    unresolved = value;
    unresolvedAt = editAt;
    const committedRevision = revision;
    const next = writes.then(async () => {
      await writeUiRecord(address, value, { source, sequence: committedRevision });
      await reads;
      // Announcements from earlier writes can arrive after a newer local edit.
      // Only the newest committed value may repaint the active control.
      if (committedRevision !== revision || pending !== undefined) return;
      dirty = false;
      const cached = await read();
      if (committedRevision !== revision || pending !== undefined) return;
      // A competing committed value can replace ours between the write and
      // this readback. It is now the painted truth, so the old edit must not
      // survive in the page-exit journal and resurrect on reload.
      if (cached !== undefined) {
        unresolved = undefined;
        unresolvedAt = undefined;
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
    const editAt = pendingAt;
    pending = undefined;
    pendingAt = undefined;
    return commit(value, editAt);
  };
  const schedule = (value) => {
    pending = value;
    pendingAt = Date.now();
    revision += 1; // an in-flight mount read must not erase active typing
    dirty = true;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => { void flush(); }, debounceMs);
  };
  const flushOnPageExit = () => {
    const unfinished = pending === undefined ? unresolved : pending;
    const editAt = pending === undefined ? unresolvedAt : pendingAt;
    if (unfinished !== undefined) holdPending(address, unfinished, source, revision, editAt);
    if (pending !== undefined) void flush();
  };
  const flushWhenHidden = () => {
    if (globalThis.document?.visibilityState === "hidden") flushOnPageExit();
  };
  if (debounceMs > 0) {
    globalThis.addEventListener?.("pagehide", flushOnPageExit);
    globalThis.document?.addEventListener?.("visibilitychange", flushWhenHidden);
  }
  const ready = journaled === undefined ? read() : (async () => {
    await writeUiRecordIfNewer(address, journaled.value, journaled);
    releasePending(address);
    return read();
  })();

  return {
    ready,
    write(value) {
      pending = undefined;
      pendingAt = undefined;
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
