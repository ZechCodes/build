// core/localCache.js over a Map, for the suites that run on fake timers.
//
// The real store is IndexedDB, and IndexedDB settles on the event loop rather
// than on a timer: a suite that has faked the loop never sees a record land, so
// a surface that paints from the cache paints nothing and a test about anything
// else fails for a reason that is not about it. This double answers the same
// addresses in the same order with the same announcements, on promises alone.
//
// It is a double, not a second implementation of the rules: the addressing,
// the prefix match and the announcement fan-out are the ones core/localCache.js
// is written with, and nothing here stands down, fails, or is unavailable.

const records = new Map(); // key → { at, value }
const listeners = new Set(); // { parts, listener }

export const DEVICES_ADDRESS = Object.freeze({ deviceId: "", entityId: "", kind: "devices" });

const recordKey = ({ deviceId, entityId, kind, sub = "" }) =>
  [deviceId, entityId, kind, sub].map(encodeURIComponent).join("|");

/** The encoded key parts an address names, stopping at the first it leaves
 *  out — so a prefix address reaches everything under it. */
function addressParts(address) {
  const named = [];
  for (const part of [address.deviceId, address.entityId, address.kind, address.sub]) {
    if (part === undefined || part === null) break;
    named.push(encodeURIComponent(part));
  }
  return named;
}

const partsOfKey = (key) => (key ? String(key).split("|") : []);

const addressOfParts = (parts) => {
  const names = ["deviceId", "entityId", "kind", "sub"];
  const address = {};
  parts.forEach((part, index) => {
    if (index < names.length) address[names[index]] = decodeURIComponent(part);
  });
  return address;
};

function partsTouch(one, other) {
  const depth = Math.min(one.length, other.length);
  for (let index = 0; index < depth; index += 1) if (one[index] !== other[index]) return false;
  return true;
}

function announce(parts) {
  for (const entry of [...listeners]) {
    if (partsTouch(entry.parts, parts)) entry.listener(addressOfParts(parts));
  }
}

const keysUnder = (prefixAddress) => {
  const prefix = `${addressParts(prefixAddress).join("|")}|`;
  return [...records.keys()].filter((key) => key.startsWith(prefix)).sort();
};

export function subscribeCache(prefixAddress, listener) {
  const entry = { parts: addressParts(prefixAddress), listener };
  listeners.add(entry);
  return () => listeners.delete(entry);
}

export const readCached = async (address) => records.get(recordKey(address));

export const readCachedMany = async (addresses) => addresses.map((address) => records.get(recordKey(address)));

export async function writeCached(address, value) {
  const key = recordKey(address);
  records.set(key, { at: Date.now(), value });
  announce(partsOfKey(key));
}

export async function mergeCached(address, merge) {
  const held = records.get(recordKey(address));
  const merged = merge(held?.value);
  if (merged === undefined || merged === null) return;
  await writeCached(address, merged);
}

// A Map read and write run synchronously in this double. IndexedDB's version
// keeps those steps in one readwrite transaction across browser tabs.
export const mergeCachedAtomically = mergeCached;

export async function evictEntity(deviceId, entityId) {
  for (const key of keysUnder({ deviceId, entityId })) records.delete(key);
  announce([encodeURIComponent(deviceId), encodeURIComponent(entityId)]);
}

export async function cachedEntityIds(deviceId) {
  const ids = new Set();
  for (const key of keysUnder({ deviceId })) {
    const entityPart = key.split("|")[1];
    if (entityPart) ids.add(decodeURIComponent(entityPart));
  }
  return [...ids];
}

export async function cachedSubKeys(deviceId, entityId, kind) {
  const prefix = `${[deviceId, entityId, kind].map(encodeURIComponent).join("|")}|`;
  return keysUnder({ deviceId, entityId, kind }).map((key) => decodeURIComponent(key.slice(prefix.length)));
}

export const cachedAddresses = async (prefixAddress) =>
  keysUnder(prefixAddress).map((key) => addressOfParts(partsOfKey(key)));

export const cachedAddressesWrittenBefore = async (prefixAddress, writtenBefore) =>
  keysUnder(prefixAddress)
    .filter((key) => records.get(key).at < writtenBefore)
    .sort((one, other) => records.get(one).at - records.get(other).at)
    .map((key) => addressOfParts(partsOfKey(key)));

export const cachedRecords = async (prefixAddress) =>
  keysUnder(prefixAddress).map((key) => ({
    address: addressOfParts(partsOfKey(key)),
    at: records.get(key).at,
    value: records.get(key).value,
  }));

export async function deleteCached(addresses) {
  const keys = addresses.map(recordKey).filter((key) => records.delete(key));
  for (const key of keys) announce(partsOfKey(key));
}

export async function wipeCache() {
  records.clear();
  announce([]);
}

/** Between cases: the store empty and nobody listening to it. */
export function resetMemoryCache() {
  records.clear();
  listeners.clear();
}

/** Empty records while retaining module-lifetime production subscriptions. */
export function clearMemoryCacheRecords() {
  records.clear();
}
