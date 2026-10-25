// Feed and route readers share one device snapshot. Announcements only mark
// addresses dirty; the next reader composes pending layers in one cache read.
import { cachedAddresses, readCached, readCachedMany, subscribeCache } from "./localCache.js";

const rootKinds = ["feed", "projects", "workspaces", "project-agents-support", "agent-lineage-support"];
const rowKinds = new Set(["row", "project-agents"]);
const relevantKinds = new Set([...rootKinds, ...rowKinds]);
const keyOf = (address) => JSON.stringify([address.entityId || "", address.kind, address.sub || ""]);
const projections = new Map();
const READ_BATCH_SIZE = 64;

function createProjection(deviceId) {
  const state = { full: true, dirty: new Map(), records: new Map(), snapshot: null, reading: null, version: 0, snapshotVersion: -1 };
  subscribeCache({ deviceId }, (address) => {
    if (address.kind && !relevantKinds.has(address.kind)) return;
    state.version += 1;
    if (!address.kind) state.full = true;
    else state.dirty.set(keyOf(address), address);
  });
  return state;
}

async function addressesToRead(deviceId, state) {
  if (!state.full) {
    const addresses = [...state.dirty.values()];
    addresses.forEach((address) => state.dirty.delete(keyOf(address)));
    return addresses;
  }
  state.full = false;
  state.dirty.clear();
  state.records.clear();
  return [
    ...(await cachedAddresses({ deviceId })).filter((address) => rowKinds.has(address.kind)),
    ...rootKinds.map((kind) => ({ deviceId, entityId: "", kind })),
  ];
}

// Independent roots begin immediately beside the cold row transaction, so
// route discovery does not hold owner-list reads behind a large row sweep.
async function readInitialRecords(addresses) {
  const rootOffset = addresses.length - rootKinds.length;
  const [rows, ...roots] = await Promise.all([
    readCachedMany(addresses.slice(0, rootOffset)),
    ...addresses.slice(rootOffset).map((address) => readCached(address)),
  ]);
  return [...rows, ...roots];
}

async function readDirtyRecords(addresses) {
  const records = [];
  for (let offset = 0; offset < addresses.length; offset += READ_BATCH_SIZE) {
    records.push(...await readCachedMany(addresses.slice(offset, offset + READ_BATCH_SIZE)));
  }
  return records;
}

async function drainRecords(deviceId, state) {
  do {
    const version = state.version;
    const full = state.full;
    const addresses = await addressesToRead(deviceId, state);
    const records = await (full ? readInitialRecords(addresses) : readDirtyRecords(addresses));
    // Prefix eviction invalidates the held index and any response captured
    // before it. Ordinary writes remain dirty for the next finite pass.
    if (state.full) continue;
    addresses.forEach((address, index) => {
      if (records[index]) state.records.set(keyOf(address), { address, record: records[index] });
      else state.records.delete(keyOf(address));
    });
    state.snapshotVersion = version;
  } while (state.full);
  const entries = [...state.records.values()];
  state.snapshot = {
    rows: entries.filter(({ address }) => rowKinds.has(address.kind)),
    roots: new Map(entries.filter(({ address }) => !rowKinds.has(address.kind))
      .map(({ address, record }) => [address.kind, record])),
  };
  return state.snapshot;
}

function startRead(deviceId, state) {
  return drainRecords(deviceId, state).catch((error) => {
    state.full = true;
    throw error;
  }).finally(() => { state.reading = null; });
}

export async function readFeedCacheRecords(deviceId) {
  if (!projections.has(deviceId)) projections.set(deviceId, createProjection(deviceId));
  const state = projections.get(deviceId);
  // Finish the synchronous announcement before observing dirtiness: a view
  // listener can be registered before this shared projection listener.
  await Promise.resolve();
  // Each caller covers dirtiness known at its call. Later announcements do
  // not extend its wait, even if another caller starts the trailing pass.
  const version = state.version;
  while (!state.snapshot || state.snapshotVersion < version) {
    if (!state.reading) state.reading = startRead(deviceId, state);
    await state.reading;
  }
  return state.snapshot;
}
