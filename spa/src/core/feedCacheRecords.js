// Feed and route readers share device row/owner records. Auxiliary feed roots
// refresh independently, so a held background read cannot delay route discovery.
import { cachedAddresses, readCached, readCachedMany, subscribeCache } from "./localCache.js";

const ownerKinds = ["projects", "workspaces"];
const auxiliaryKinds = ["feed", "project-agents-support", "agent-lineage-support"];
const rowKinds = new Set(["row", "project-agents"]);
const keyOf = (address) => JSON.stringify([address.entityId || "", address.kind, address.sub || ""]);
const projections = new Map();
const READ_BATCH_SIZE = 64;

function createProjection(deviceId, rootKinds, withRows) {
  const relevantKinds = new Set([...rootKinds, ...(withRows ? rowKinds : [])]);
  const state = { rootKinds, withRows, full: true, dirty: new Map(), records: new Map(), snapshot: null, reading: null, version: 0, snapshotVersion: -1 };
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
    ...(state.withRows ? (await cachedAddresses({ deviceId })).filter((address) => rowKinds.has(address.kind)) : []),
    ...state.rootKinds.map((kind) => ({ deviceId, entityId: "", kind })),
  ];
}

// Independent roots begin immediately beside the cold row transaction, so
// route discovery does not hold owner-list reads behind a large row sweep.
async function readInitialRecords(addresses, rootCount) {
  const rootOffset = addresses.length - rootCount;
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
    const records = await (full ? readInitialRecords(addresses, state.rootKinds.length) : readDirtyRecords(addresses));
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

async function readProjection(deviceId, state) {
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

const combinedSnapshots = new WeakMap();

function combineSnapshots(owners, auxiliary) {
  const held = combinedSnapshots.get(owners);
  if (held?.auxiliary === auxiliary) return held.snapshot;
  const snapshot = { rows: owners.rows, roots: new Map([...owners.roots, ...auxiliary.roots]) };
  combinedSnapshots.set(owners, { auxiliary, snapshot });
  return snapshot;
}

// Route discovery shares the row sweep with feed readers, but needs only the
// owner lists. A held board/support read must not hold an already cached chat.
export async function readFeedCacheRecords(deviceId, { routeOnly = false } = {}) {
  if (!projections.has(deviceId)) projections.set(deviceId, createDeviceProjections(deviceId));
  const state = projections.get(deviceId);
  if (routeOnly) return readProjection(deviceId, state.owners);
  for (;;) {
    const epoch = state.epoch;
    const [owners, auxiliary] = await Promise.all([
      readProjection(deviceId, state.owners), readProjection(deviceId, state.auxiliary),
    ]);
    // One side may have finished before a reset while the other was held.
    // Never combine snapshots from opposite sides of a prefix eviction.
    if (epoch === state.epoch) return combineSnapshots(owners, auxiliary);
  }
}

function createDeviceProjections(deviceId) {
  const state = {
    epoch: 0,
    owners: createProjection(deviceId, ownerKinds, true),
    auxiliary: createProjection(deviceId, auxiliaryKinds, false),
  };
  subscribeCache({ deviceId }, (address) => {
    if (!address.kind) state.epoch += 1;
  });
  return state;
}
