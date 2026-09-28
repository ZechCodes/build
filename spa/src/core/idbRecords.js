// What every record database here shares: how an address becomes a key, how a
// write is stamped, and how a change is announced to this tab and the others.
// The replica cache (core/localCache.js) and the local UI store
// (core/localUiStore.js) each key, stamp and announce the same way, on their
// own channel.

// A board record and a row can be written in the same clock millisecond.
// Keep their order beside the timestamp without changing the timestamp used
// for cache lifetime and page-exit journal comparisons.
let lastWriteOrder = 0;
const nextWriteOrder = () => {
  const clock = globalThis.performance;
  const now = Number.isFinite(clock?.timeOrigin) ? clock.timeOrigin + clock.now() : Date.now();
  lastWriteOrder = Math.max(now, lastWriteOrder + 0.001);
  return lastWriteOrder;
};
export const randomToken = () => globalThis.crypto?.randomUUID?.() ||
  `${Date.now()}-${Math.random()}-${Math.random()}`;
// `order` is a clock, and two tabs can read the same tick. `write` names the
// write itself — this page's own id and its count of writes — so a reader
// that captured it can tell whether any writer, in any tab, has put the
// record since. Every record written here carries both.
const WRITER = randomToken();
let writesMade = 0;
export const writeStamp = () => ({ at: Date.now(), order: nextWriteOrder(), write: `${WRITER}:${(writesMade += 1)}` });

/** The record key. Every part is URI-encoded so a separator inside a branch
 *  name, path, or hash cannot make one record's key a prefix of another's. */
export const recordKey = ({ deviceId, entityId, kind, sub = "" }) =>
  [deviceId, entityId, kind, sub].map(encodeURIComponent).join("|");

export const prefixRange = (prefix) => IDBKeyRange.bound(prefix, `${prefix}￿`, false, true);

/** The encoded key parts an address names, in key order, stopping at the
 *  first one it leaves out. Leaving `sub` out means "every sub-key under this
 *  kind"; naming it, even as `""`, means that one record. */
export function addressParts(address) {
  const named = [];
  for (const part of [address.deviceId, address.entityId, address.kind, address.sub]) {
    if (part === undefined || part === null) break;
    named.push(encodeURIComponent(part));
  }
  return named;
}

export const keyOfParts = (parts) => parts.join("|");
export const partsOfKey = (key) => (key ? String(key).split("|") : []);

export const addressOfParts = (parts) => {
  const names = ["deviceId", "entityId", "kind", "sub"];
  const address = {};
  parts.forEach((part, index) => {
    if (index < names.length) address[names[index]] = decodeURIComponent(part);
  });
  return address;
};

/** Two addresses touch when neither contradicts the other on a part they both
 *  name: `dev|run-1` covers `dev|run-1|status|`, and is covered by it. */
function partsTouch(one, other) {
  const depth = Math.min(one.length, other.length);
  for (let index = 0; index < depth; index += 1) if (one[index] !== other[index]) return false;
  return true;
}

// ─── Announcements ───────────────────────────────────────────────────────────
//
// Every writer says what it changed, and a surface that holds a record hears
// it. The message is the address and nothing else: the record is already on
// disk, and only the reader knows which shape it wants out of it — so a
// listener re-reads rather than being handed a body it may not want.
//
// An address is matched part by part, so a listener names as much of the key
// as it cares about: a device, an entity, a kind, or one sub-key. An eviction
// names a prefix, and reaches every listener under it as well as every
// listener it is under.

/** The announcements of one database, carried between tabs on `channelName`.
 *  Answers `announce(parts)` and `subscribe(prefixAddress, listener)`. */
export function createAnnouncer(channelName) {
  const listeners = new Set(); // { parts, listener }
  let channel; // undefined until first asked for, null where there is none

  /** The tab-to-tab channel, opened on the first subscription so a tab that
   *  never writes still hears. A browser without it simply has no cross-tab
   *  announcements; everything in this tab works the same. */
  function tabChannel() {
    if (channel !== undefined) return channel;
    channel = null;
    if (typeof BroadcastChannel === "undefined") return channel;
    try {
      const opened = new BroadcastChannel(channelName);
      opened.onmessage = (event) => announce(partsOfKey(event?.data?.key), false);
      opened.unref?.(); // node: never hold the process open for the cache
      channel = opened;
    } catch (error) {
      console.warn("cross-tab cache announcements unavailable:", error);
    }
    return channel;
  }

  /** Tell every listener the change is under or over, and — for a change
   *  made here — the other tabs. A listener that throws is the caller's
   *  problem, not the writer's: the record is already stored. */
  function announce(parts, broadcast = true) {
    for (const entry of [...listeners]) {
      if (!partsTouch(entry.parts, parts)) continue;
      try {
        entry.listener(addressOfParts(parts));
      } catch (error) {
        console.warn("a cache listener threw:", error);
      }
    }
    if (broadcast) tabChannel()?.postMessage({ key: keyOfParts(parts) });
  }

  /** Hear every write and eviction at or under `prefixAddress`, from this tab
   *  and from every other tab on this browser profile. The listener is handed
   *  the address that changed and reads what it wants; answers the way to
   *  stop listening. */
  function subscribe(prefixAddress, listener) {
    const entry = { parts: addressParts(prefixAddress), listener };
    listeners.add(entry);
    tabChannel();
    return () => listeners.delete(entry);
  }

  return { announce, subscribe };
}
