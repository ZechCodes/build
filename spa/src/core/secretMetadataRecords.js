// Authoritative list publication, revision/status ordering and deadline-driven
// cache writes. Views never infer secret availability from their own clocks.
import { mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { randomToken } from "./idbRecords.js";

const orderAddress = address => ({ ...address, kind: "secret-metadata-order" });
const terminal = entry => ["expired", "revoked"].includes(entry.status);
const byId = (a, b) => a.secret_id.localeCompare(b.secret_id);

export function secretDeadline(entry) {
  if (terminal(entry) || entry.revoked_at !== null) return null;
  return entry.attached_at === null ? entry.created_at + 86400 : entry.expires_at;
}

export function secretAtTime(entry, serverNow) {
  if (entry.revoked_at !== null && !terminal(entry)) return { ...entry, status: "revoked" };
  const deadline = secretDeadline(entry);
  if (deadline !== null && Number.isFinite(serverNow) && deadline <= serverNow) return { ...entry, status: "expired" };
  return entry;
}

function newerEntry(previous, entry) {
  if (!previous) return entry;
  if (previous.revision > entry.revision) return previous;
  if (previous.revision === entry.revision && terminal(previous)) return previous;
  return entry;
}

const selectedEntry = (entry, selection) => selection?.secretIds.includes(entry.secret_id)
  || selection?.requestIds.includes(entry.request_id);

function reconciledEntries(held = [], incoming, replace, serverNow, selection) {
  const previous = new Map(held.map(entry => [entry.secret_id, entry]));
  const entries = replace ? new Map() : new Map([...previous].filter(([, entry]) => !selectedEntry(entry, selection)));
  for (const entry of incoming) entries.set(entry.secret_id, newerEntry(entries.get(entry.secret_id) || previous.get(entry.secret_id), entry));
  return [...entries.values()].map(entry => secretAtTime(entry, serverNow)).sort(byId);
}

// Only the queried address gets a new admission. Incidental project/environment
// rows in a stage response capture existing tokens, so they cannot cancel an
// authoritative read of those scopes or outlive its completed publication.
export async function admitSecretRead(addresses, active, primary = addresses[0]) {
  const tokens = new Map();
  const primaryKey = JSON.stringify(primary);
  await mergeCachedRecordsTogether(addresses.map(orderAddress), held => {
    if (!active()) return null;
    return addresses.map((address, index) => {
      const key = JSON.stringify(address);
      const token = key === primaryKey ? randomToken() : held[index]?.value?.token || randomToken();
      tokens.set(key, token);
      return { token };
    });
  }, { notify: false });
  return tokens;
}

function carriedObservation(held = {}) {
  return { server_now: held.server_now ?? null, clock_now: held.clock_now ?? held.server_now ?? null,
    observed_at_ms: held.observed_at_ms ?? null };
}

function nextObservation(held, serverNow, observedNow) {
  if (!Number.isSafeInteger(serverNow) || serverNow < held?.server_now) return carriedObservation(held);
  return { server_now: serverNow, clock_now: Math.max(serverNow, observedNow ?? held?.clock_now ?? serverNow), observed_at_ms: Date.now() };
}

function nextRecord(held, entries, serverNow, replace, selection, observedNow) {
  const observation = nextObservation(held, serverNow, observedNow);
  return { secrets: reconciledEntries(held?.secrets, entries, replace, observation.clock_now, selection), ...observation };
}

// Acknowledged writes are unconditional revision-aware merges. The same
// transaction invalidates outstanding reads; another admission cannot suppress
// this write because mutations never depend on an admission token.
export function mergeSecretWrite(address, entries, active) {
  return mergeCachedRecordsTogether([orderAddress(address), address], held => active()
    ? [{ token: randomToken() }, nextRecord(held[1]?.value, entries, undefined, false)] : null);
}

// Consume the primary token on publication too: a delayed incidental read that
// captured it while this request was in flight cannot restore removed rows.
const admittedPublication = (held, expected) => Boolean(expected && held[0]?.value?.token === expected);

function publication(group, held, token, serverNow, replace, selection, observe) {
  const expected = token.get(JSON.stringify(group.address));
  if (!admittedPublication(held, expected)) return [null, null];
  const heldRecord = held[1]?.value;
  const observedNow = heldRecord ? observe?.(group.address, heldRecord) : null;
  const record = nextRecord(heldRecord, group.entries, serverNow, replace && group.primary, group.primary ? selection : undefined, observedNow);
  return [group.primary ? { token: randomToken() } : null, record];
}

export function publishSecretGroups(groups, { token, serverNow, replace, selection, active, observe }) {
  const addresses = groups.flatMap(group => [orderAddress(group.address), group.address]);
  return mergeCachedRecordsTogether(addresses, held => active() ? groups.flatMap((group, index) =>
    publication(group, held.slice(index * 2, index * 2 + 2), token, serverNow, replace, selection, observe)) : null);
}

function estimatedServerNow(record) {
  if (!Number.isSafeInteger(record.server_now)) return null;
  const elapsed = Number.isFinite(record.observed_at_ms) ? Math.max(0, Date.now() - record.observed_at_ms) / 1000 : 0;
  return (record.clock_now ?? record.server_now) + elapsed;
}

async function expireRecord(address, serverNow, active) {
  await mergeCachedRecordsTogether([address], ([held]) => {
    if (!active() || !held?.value) return null;
    const record = held.value;
    const time = Math.max(record.server_now, serverNow);
    const secrets = record.secrets.map(entry => secretAtTime(entry, time));
    if (secrets.every((entry, index) => entry === record.secrets[index])) return null;
    return [{ ...record, secrets, server_now: Math.floor(time), clock_now: time, observed_at_ms: Date.now() }];
  });
}

function createAnchoredClock() {
  const anchors = new Map();
  return {
    now(key, record) {
      const monotonic = performance.now();
      const previous = anchors.get(key);
      const projected = previous ? previous.time + Math.max(0, monotonic - previous.monotonic) / 1000 : 0;
      const stamp = `${record.server_now}:${record.clock_now}:${record.observed_at_ms}`;
      if (previous?.stamp === stamp) return projected;
      const observed = estimatedServerNow(record);
      if (observed === null) return null;
      const time = Math.max(projected, observed);
      anchors.set(key, { stamp, time, monotonic });
      return time;
    },
    clear() { anchors.clear(); },
  };
}

/** Timers advance only from a bridge-observed time anchor. The stored anchor
 * also permits a cold/offline reader to expire its retained metadata. While
 * running, elapsed time uses a monotonic clock so wall-clock changes cannot
 * extend or shorten a known deadline. */
export function createSecretExpiry(active) {
  const timers = new Map();
  const clock = createAnchoredClock();
  let disposed = false;
  const current = () => !disposed && active();
  const owns = (key, observation, record) => Boolean(record && current() && timers.get(key) === observation);
  function schedule(address, record, observation, key) {
    const deadlines = record.secrets.map(secretDeadline).filter(value => value !== null);
    if (!deadlines.length) return;
    const delay = Math.max(0, Math.ceil((Math.min(...deadlines) - clock.now(key, record)) * 1000));
    observation.timer = setTimeout(() => { void watch(address).catch(() => {}); }, Math.min(delay, 2147483647));
    observation.timer.unref?.();
  }
  async function watch(address) {
    if (!current()) return;
    const key = JSON.stringify(address);
    clearTimeout(timers.get(key)?.timer);
    const observation = {};
    timers.set(key, observation);
    let record = (await readCached(address))?.value;
    if (!owns(key, observation, record)) return;
    const now = clock.now(key, record);
    if (now === null) return;
    await expireRecord(address, now, current);
    record = (await readCached(address))?.value;
    if (owns(key, observation, record)) schedule(address, record, observation, key);
  }
  return Object.freeze({ watch, now: (address, record) => clock.now(JSON.stringify(address), record), dispose() { disposed = true; timers.forEach(entry => clearTimeout(entry.timer)); timers.clear(); clock.clear(); } });
}
