// One shape's bodies, fetched per unit and kept.
//
// The wire carries shape — a status names its files, a page names its runs —
// and the body of each unit is fetched on its own and cached under its own
// record. Two surfaces need exactly that: the per-file diffs behind a status,
// and the activity items behind a run. Both do the same four things in the
// same order: decide what is missing, cross ONE async boundary, write what came
// back through to the local cache, then answer bodies from that stored record.
//
// The three things a configuration has to say:
//   `addressOf(key)`      the local-cache address for that key, or null where
//                         the surface has no entity to cache under.
//   `fetchMissing(keys)`  the one wire call, given every key it must answer.
//   `valueOf(item)`       one answered item as `{ key, value }` — the value is
//                         what is cached and what `read` hands back.
//
// `ensure` is the only place that waits. `read` and `has` answer from what is
// held right now, because a paint asks them.
//
// A body `cacheable` refuses — over its cap, or cut short by the bridge — is
// kept in pages instead (#95, core/bodyPages.js), when the configuration says
// how (`pages`):
//   `field`               the value's field the body's text rides in
//   `split(value, of)`    the answer cut into pages of `of`, each `{ of,
//                         offset, end, total, body }`
//   `join(pages)`         the pages' text, joined (patch text by default)
//   `readPage(key, offset, value)`  the page a bridge cuts from `offset`, or
//                         null where it cannot page (asked when reading, so
//                         a bridge that greets later is read on from)
// The record then holds the value without its text, `paged: true`, and `of`,
// which body its pages are (the head). Where the bridge can page, the pages
// are its own from the first, named by the version it gives them — a content
// key does not name a patch, which moves with HEAD while the file stands
// still; where it cannot, the answer is split here and named `WHOLE_ANSWER`.
// Each page is a record of its own, and `read` answers the value with the
// held pages joined into `field` and `pages: { end, total, complete }`.
// `more(key)` reads the next page. Nothing is ever painted from an answer.

import { deleteCached, mergeCachedRecordsTogether, readCached, recordWriteOf, subscribeCache } from "./localCache.js";
import {
  bodyPagePut,
  dropBodyPages,
  joinedText,
  pageFollows,
  readBodyPages,
  subscribeBodyPages,
} from "./bodyPages.js";

/** What the pages of an answer split here are of: an answer read whole names
 *  no version, and any page a bridge answers later is of one, so it is never
 *  joined to these. */
export const WHOLE_ANSWER = "whole";

export function createCachedBodies({
  addressOf,
  fetchMissing,
  valueOf,
  cacheable = () => true,
  pages = null,
  onChange = () => {},
}) {
  const held = new Map(); // key → body
  const consulted = new Set(); // keys whose stored record has been looked at
  const observedWrite = new Map(); // key → the write seen before a fetch (recordWriteOf)
  const unwatches = new Map();
  const rereads = new Map();
  let disposed = false;

  const join = pages?.join || joinedText;
  const joined = new Map(); // key → { headWrite, found, text }: the pages read so far, joined

  /** The held pages of a head joined, read on from what was joined under the
   *  same head before: its pages never change while it stands (a head is
   *  written after its pages, every time they are replaced), so a reader
   *  scrolling a long body costs each page once. Text joins by appending;
   *  anything else is joined whole again. */
  const joinPages = async (key, at, record) => {
    const before = joined.get(key);
    const known = before?.headWrite === recordWriteOf(record) ? before : null;
    const found = await readBodyPages(at, record.value.of, known?.found);
    const onward = known && found.pages.length >= known.found.pages.length && join === joinedText;
    const text = onward ? known.text + joinedText(found.pages.slice(known.found.pages.length)) : join(found.pages);
    if (!disposed) joined.set(key, { headWrite: recordWriteOf(record), found, text });
    return { found, text };
  };

  /** A head's value with the pages the cache holds joined back into it. */
  const withPages = async (key, at, record) => {
    const head = record.value;
    const { found, text } = await joinPages(key, at, record);
    const { end, total, complete } = found;
    return { ...record, value: { ...head, [pages.field]: text, pages: { end, total, complete } } };
  };

  const storedRecord = async (key) => {
    const at = addressOf(key);
    const record = at ? await readCached(at) : undefined;
    return record?.value?.paged && pages ? withPages(key, at, record) : record;
  };

  const takeRecord = (key, record) => {
    observedWrite.set(key, recordWriteOf(record));
    if (record) held.set(key, record.value);
    else held.delete(key);
    return Boolean(record);
  };

  /** Serialize announcement reads per body. An own write announces before its
   * promise settles; the explicit readback queues behind that announcement so
   * `ensure` cannot finish while `held` still contains the pulled object. */
  const reread = (key, notify = true) => {
    const stringKey = String(key);
    const run = async () => {
      const record = await storedRecord(stringKey);
      if (disposed) return false;
      const present = takeRecord(stringKey, record);
      if (notify) onChange(stringKey);
      return present;
    };
    const next = (rereads.get(stringKey) || Promise.resolve()).then(run, run);
    const settled = next.finally(() => {
      if (rereads.get(stringKey) === settled) rereads.delete(stringKey);
    });
    rereads.set(stringKey, settled);
    return settled;
  };

  const watch = (key) => {
    if (disposed || unwatches.has(key)) return;
    const at = addressOf(key);
    if (!at) return;
    const unwatchRecord = subscribeCache(at, () => void reread(key));
    const unwatchPages = pages ? subscribeBodyPages(at, () => void reread(key)) : () => {};
    unwatches.set(key, () => {
      unwatchRecord();
      unwatchPages();
    });
  };

  /** Fill from the local cache every key that has never been looked up there,
   *  and answer the ones it could. A key looked up once is never looked up
   *  again: the record is this session's own write after that. */
  async function hydrate(keys) {
    const fresh = keys.filter((key) => !consulted.has(key));
    fresh.forEach(watch);
    const stored = await Promise.all(fresh.map(storedRecord));
    if (disposed) return [];
    const filled = [];
    fresh.forEach((key, index) => {
      consulted.add(key);
      if (!takeRecord(key, stored[index])) return;
      filled.push(key);
      onChange(key);
    });
    return filled;
  }

  const acceptFetched = async (item, startedFrom) => {
    const { key, value } = valueOf(item);
    const stringKey = String(key);
    const at = addressOf(stringKey);
    const current = at ? await readCached(at) : undefined;
    if (disposed) return null;
    if (at && recordWriteOf(current) !== startedFrom.get(stringKey)) {
      await reread(stringKey);
      return stringKey;
    }
    if (!at) {
      held.set(stringKey, value);
      onChange(stringKey);
      return stringKey;
    }
    const stored = await storeFetched(stringKey, at, value, recordWriteOf(current));
    await reread(stringKey, !stored);
    return disposed ? null : stringKey;
  };

  /** Admit the answer and all its pages together, checking this reader's
   * lifetime inside the transaction as well as after its async reads. */
  const keepFetched = (at, value, startedFrom, bodyPages = []) => {
    const puts = [{ address: at, value }, ...bodyPages.map((page) => bodyPagePut(at, page))];
    return mergeCachedRecordsTogether(puts.map((put) => put.address), (records) =>
      disposed || recordWriteOf(records[0]) !== startedFrom
        ? records.map(() => null)
        : puts.map((put) => put.value));
  };

  /** Keep one answer: whole when it fits, in pages when it does not, and not
   *  at all when this configuration has no pages to keep it in — the older
   *  record goes, so a revisit cannot show what the file said before.
   *  Answers whether it kept it: the first page is one more wire call, and a
   *  record written or dropped while it was out (another write than
   *  `startedFrom`) is left as it is now, heads and pages alike. */
  const storeFetched = async (key, at, value, startedFrom) => {
    if (cacheable(value)) {
      if (pages) await dropBodyPages(at);
      return keepFetched(at, value, startedFrom);
    }
    if (!pages) {
      await deleteCached([at]);
      return true;
    }
    const first = await firstPage(key, value);
    if (disposed || recordWriteOf(await readCached(at)) !== startedFrom) return false;
    await dropBodyPages(at);
    const stored = first ? [first] : pages.split(value, WHOLE_ANSWER);
    const head = { ...value, paged: true, of: stored[0].of };
    delete head[pages.field];
    return keepFetched(at, head, startedFrom, stored);
  };

  /** The bridge's own first page of a body, where it can page. */
  const firstPage = async (key, value) => {
    if (!pages.readPage) return null;
    const page = await pages.readPage(key, 0, value).catch(() => null);
    return page && page.offset === 0 && page.of ? page : null;
  };

  /** Read the page after the last one held of `key`'s body, and keep it.
   *  Answers whether the held pages moved on. A page of another version of
   *  the body is news the body moved: the key is fetched whole again. */
  async function readNextPage(key) {
    const value = held.get(key);
    const at = addressOf(key);
    if (!readsOn(at, value)) return false;
    const from = value.pages.end;
    const page = await pages.readPage(key, from, value);
    if (!page || disposed) return false;
    if (page.of !== value.of) {
      await fetchBodies([key]);
      return false;
    }
    return keepPage(key, at, page, from);
  }

  /** Whether a held body has a page after its last that can be read. */
  const readsOn = (at, value) => Boolean(pages?.readPage && at && value?.pages && !value.pages.complete);

  const aliveHeadOf = (record, of) => !disposed && Boolean(record?.value?.paged) && record.value.of === of;

  const readKeptPage = async (key, from) => {
    if (disposed) return false;
    await reread(key, false);
    if (disposed) return false;
    onChange(key);
    return (held.get(key)?.pages?.end ?? from) > from;
  };

  /** Keep a page read at `from`, when it carries the body on and the body is
   *  still the cache's; answers whether the held pages moved on. */
  const keepPage = async (key, at, page, from) => {
    const of = held.get(key)?.of;
    if (!pageFollows(page, from, of)) return false;
    const current = await readCached(at);
    if (!aliveHeadOf(current, of)) return false;
    const put = bodyPagePut(at, page);
    const written = await mergeCachedRecordsTogether([at, put.address], (records) =>
      disposed || recordWriteOf(records[0]) !== recordWriteOf(current) ? [null, null] : [null, put.value]);
    return written ? readKeptPage(key, from) : false;
  };
  /** One page read of a body at a time. An ask that lands while one is out —
   *  the paint of the page it brought, or of the body read again from the top,
   *  with the reader still at the end — is answered by reading on once it is
   *  done: nothing else would ask. */
  const readingNext = new Map(); // key → { reading, again }
  const more = (key) => {
    const stringKey = String(key);
    const out = readingNext.get(stringKey);
    if (out) {
      out.again = true;
      return out.reading;
    }
    const read = { again: false };
    const of = held.get(stringKey)?.of;
    read.reading = readNextPage(stringKey)
      .catch(() => false)
      .then((moved) => {
        readingNext.delete(stringKey);
        const onward = moved || held.get(stringKey)?.of !== of;
        if (onward && read.again && !disposed) void more(stringKey);
        return moved;
      });
    readingNext.set(stringKey, read);
    return read.reading;
  };

  /** The wire, then the write-through. Batching belongs to the caller. */
  async function fetchBodies(keys) {
    if (disposed || !keys.length) return [];
    const startedFrom = new Map(keys.map((key) => [String(key), observedWrite.get(String(key)) ?? null]));
    const filled = [];
    for (const item of await fetchMissing(keys)) {
      if (disposed) break;
      const accepted = await acceptFetched(item, startedFrom);
      if (accepted) filled.push(accepted);
    }
    return filled;
  }

  /** Hold a body for each of `keys`, and answer the keys that were filled.
   *
   *  A key never seen before is taken from the local cache when it holds one.
   *  Everything else is fetched — including a key already held, which is how a
   *  caller replaces a body it has decided is stale. */
  async function ensure(keys) {
    const wanted = [...new Set(keys.map(String))];
    if (disposed || !wanted.length) return [];
    const hydrated = new Set(await hydrate(wanted));
    const fetched = await fetchBodies(wanted.filter((key) => !hydrated.has(key)));
    return disposed ? [] : [...hydrated, ...fetched];
  }

  /** Release a large body's joined pages without evicting its IndexedDB
   *  records. A later request hydrates them again from storage. */
  async function forget(key) {
    const named = String(key);
    await rereads.get(named)?.catch(() => {});
    unwatches.get(named)?.();
    unwatches.delete(named);
    held.delete(named);
    joined.delete(named);
    consulted.delete(named);
    observedWrite.delete(named);
  }

  return {
    read: (key) => held.get(String(key)),
    has: (key) => held.has(String(key)),
    ensure,
    forget,
    more,
    dispose: () => {
      disposed = true;
      for (const unwatch of unwatches.values()) unwatch();
      unwatches.clear();
      held.clear();
      joined.clear();
      consulted.clear();
      observedWrite.clear();
    },
  };
}
