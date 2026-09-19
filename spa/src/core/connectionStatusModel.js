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
export function connectionStatus({ devices = [], recoveries = [], nowMs = Date.now() } = {}) {
  const connectedCount = devices.filter((device) => device.live).length;
  const recovering = recoveries.filter((record) => RECOVERING.has(record.status));
  const attempting = recovering.filter((record) => record.status === ATTEMPTING);
  const waiting = recovering.filter((record) => record.status === WAITING);
  const visible = devices.length > 0 || recovering.length > 0;

  if (attempting.length) {
    return {
      state: ATTEMPTING,
      connectedCount,
      seconds: null,
      centre: "",
      label: `Reconnecting to ${namesOf(devices, attempting)}`,
      ticking: false,
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
      ticking: true,
      visible,
    };
  }
  return {
    state: CONNECTED,
    connectedCount,
    seconds: null,
    centre: String(connectedCount),
    label: connectedLabel(connectedCount),
    ticking: false,
    visible,
  };
}
