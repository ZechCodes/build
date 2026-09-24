// What the connection icon says: one state, one thing in the middle of the
// ring, and the words for both.
//
// The whole of the decision lives here so it can be read and tested without a
// document: the icon (core/connectionStatusIcon.js) draws what this answers and
// decides nothing of its own.
//
// Three states, which are the three things that can be true of an account's
// machines: every one of them is connected (green, with how many), one of them
// is being dialled right now (yellow, radiating, no number — nobody can say how
// long it will take), or one of them is between tries (yellow, counting down to
// the next). A machine that is simply blocked is none of these: recovery has
// stood down, and the rail says so where that machine's rows are.

export const CONNECTED = "connected";
export const ATTEMPTING = "attempting";
export const WAITING = "waiting";

/** How many whole seconds are left before the next attempt. Rounded up, so the
 *  countdown never shows a second it has already spent, and floored at zero:
 *  a record whose moment has passed is a try about to begin, not a negative
 *  number on the ring. */
export function secondsUntilAttempt(nextAttemptAt, nowMs) {
  if (!nextAttemptAt) return 0;
  return Math.max(0, Math.ceil((nextAttemptAt - nowMs) / 1000));
}

const RECOVERING = new Set([ATTEMPTING, WAITING]);

/** Whether one machine's recovery record says it is being reconnected to: a
 *  dial in flight, or a wait between tries. Both read as "reconnecting" to
 *  anybody looking at a surface over that machine — the difference between
 *  them is only whether the ring has a number to show. Asked by the surfaces
 *  that hold a copy quietly while the wire is being made again
 *  (core/transientRead.js), so the ring and they agree on the word. */
export const isRecovering = (record) => RECOVERING.has(record?.status);

/** What a machine is called, as the account knows it. A record can name a
 *  machine the device list has not caught up with — a connection lost before
 *  its first list answered — and such a machine is spoken of as what it is. */
const nameOf = (devices, deviceId) =>
  devices.find((device) => device.id === deviceId)?.name || "a device";

/** The machines being recovered, said the way a sentence would say them: one by
 *  name, two by both names, and more than two by how many. Past two, names stop
 *  being the useful fact and start being a list nobody reads. */
function namesOf(devices, recoveries) {
  const names = recoveries.map((record) => nameOf(devices, record.deviceId));
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.length} devices`;
}

/** "in 3 seconds", or nothing at all once the wait is spent: a countdown at
 *  zero is an attempt already beginning, and saying "in 0 seconds" would be
 *  the one moment the words are wrong. */
const waitSuffix = (seconds) => {
  if (seconds <= 0) return "";
  return seconds === 1 ? " in 1 second" : ` in ${seconds} seconds`;
};

const connectedLabel = (connectedCount) => {
  if (!connectedCount) return "No devices connected";
  return connectedCount === 1 ? "Connected to 1 device" : `Connected to ${connectedCount} devices`;
};

/** A machine the account lists that holds no session and has nothing being
 *  done about it. Not every unconnected machine is being recovered: the
 *  supervisor stands down when the account stops calling a machine online, and
 *  never starts for one that was never online. That machine is off — a state,
 *  not a failure. */
export const OFFLINE = "offline";

/** What a connected machine's row says about HOW it is connected: straight to
 *  the machine, or through a TURN relay. A machine whose path nothing has
 *  answered for yet says the plain word — the reader is told what is known,
 *  and never guessed at. */
const connectedLabelFor = (path) => {
  if (path === "turn") return "Connected TURN";
  return path === "direct" ? "Connected WebRTC" : "Connected";
};

/** What one row says about one machine. Short by design: a row has the room
 *  for a state, and the ring's own label is where the sentence is. */
const rowLabel = (status, seconds, path) => {
  if (status === CONNECTED) return connectedLabelFor(path);
  if (status === OFFLINE) return "Offline";
  return status === WAITING && seconds > 0 ? `Reconnecting in ${seconds} s` : "Reconnecting";
};

const rowFor = (id, name, status, seconds, path = null) =>
  ({ id, name, status, seconds, path: status === CONNECTED ? path : null, label: rowLabel(status, seconds, path) });

/** Every machine, in the account's own order, each with what is true of IT —
 *  the same records the ring is derived from, read one machine at a time. A
 *  machine named only by a recovery record (a connection lost before the
 *  device list answered) is a machine being reconnected to, and is listed
 *  behind the ones the account has names for. */
function rowsFor(devices, recoveries, nowMs) {
  const recovering = new Map(recoveries.map((record) => [record.deviceId, record]));
  const rowOf = (id, name) => {
    const record = recovering.get(id);
    const device = devices.find((candidate) => candidate.id === id);
    if (!record) return rowFor(id, name, device?.live ? CONNECTED : OFFLINE, null, device?.path || null);
    const seconds = record.status === WAITING ? secondsUntilAttempt(record.nextAttemptAt, nowMs) : null;
    return rowFor(id, name, record.status, seconds);
  };
  const listed = devices.map((device) => rowOf(device.id, device.name || "a device"));
  const unlisted = [...recovering.keys()]
    .filter((id) => !devices.some((device) => device.id === id))
    .map((id) => rowOf(id, nameOf(devices, id)));
  return [...listed, ...unlisted];
}

/** The supervisor's records, and one more for each live machine whose link is
 *  putting a failed path right in place (#123). That machine still holds its
 *  session, so the supervisor has no record of it — but nothing it is asked is
 *  answered until the restart lands, and a restart against a bridge that has
 *  itself restarted never does. A green ring over that was the whole of what a
 *  reader was told. It is a dial in flight, and says so. */
function withRestoring(devices, recoveries) {
  const recorded = new Set(recoveries.map((record) => record.deviceId));
  const restoring = devices
    .filter((device) => device.live && device.restoring && !recorded.has(device.id))
    .map((device) => ({ deviceId: device.id, status: ATTEMPTING, failedAttempts: 0, nextAttemptAt: null }));
  return [...recoveries, ...restoring];
}

/**
 * The icon's whole state.
 *
 * `devices` are the account's machines, each said to be live or not — live
 * meaning it holds a session that can answer. `recoveries` are the supervisor's
 * records (core/deviceRecovery.js) exactly as it publishes them.
 *
 * An attempt in flight outranks a wait: with one machine being dialled and
 * another between tries, what is happening is the dial, and the ring shows the
 * thing that is happening.
 */
export function connectionStatus({ devices = [], recoveries: recorded = [], nowMs = Date.now() } = {}) {
  const recoveries = withRestoring(devices, recorded);
  const connectedCount = devices.filter((device) => device.live && !device.restoring).length;
  const recovering = recoveries.filter((record) => RECOVERING.has(record.status));
  const attempting = recovering.filter((record) => record.status === ATTEMPTING);
  const waiting = recovering.filter((record) => record.status === WAITING);
  const visible = devices.length > 0 || recovering.length > 0;
  const rows = rowsFor(devices, recovering, nowMs);
  // The clock is held open by the MENU's countdowns as much as the ring's: a
  // ring showing an attempt in flight has no number of its own while a second
  // machine behind it is counting down, and that row has to keep counting.
  const ticking = waiting.length > 0;

  if (attempting.length) {
    return {
      state: ATTEMPTING,
      connectedCount,
      seconds: null,
      centre: "",
      label: `Reconnecting to ${namesOf(devices, attempting)}`,
      ticking,
      rows,
      visible,
    };
  }
  if (waiting.length) {
    const soonest = Math.min(...waiting.map((record) => secondsUntilAttempt(record.nextAttemptAt, nowMs)));
    return {
      state: WAITING,
      connectedCount,
      seconds: soonest,
      centre: String(soonest),
      label: `Reconnecting to ${namesOf(devices, waiting)}${waitSuffix(soonest)}`,
      ticking,
      rows,
      visible,
    };
  }
  return {
    state: CONNECTED,
    connectedCount,
    seconds: null,
    centre: String(connectedCount),
    label: connectedLabel(connectedCount),
    ticking,
    rows,
    visible,
  };
}
