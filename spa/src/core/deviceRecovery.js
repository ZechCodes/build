/** How long a device waits between dials, by how many have failed.
 *
 *  The ceiling is ten seconds, not thirty. A phone is the device this ladder is
 *  really for, and a phone's failures are almost never the machine being
 *  unreachable — they are the phone itself having been asleep. The maintainer's
 *  had climbed to the old thirty-second step across two earlier failures and
 *  was still there when they picked it up, so waking the screen bought a
 *  half-minute of nothing (issue #60). Ten seconds is still a real back-off for
 *  a machine that is genuinely gone, and a tenth of the wait for the case that
 *  actually happens. */
const BACKOFF_MS = [2000, 4000, 8000, 10000];
const JITTER_FRACTION = 0.1;

/**
 * Account-level recovery policy for devices the presence API still calls online.
 * Transport ownership stays with the supplied attempt callback; this module owns
 * only when another authoritative attempt may begin.
 */
export function createDeviceRecoverySupervisor({
  attempt,
  cancelAttempt = () => {},
  now = () => Date.now(),
  setTimer = (fn, delay) => setTimeout(fn, delay),
  clearTimer = (timer) => clearTimeout(timer),
  random = Math.random,
} = {}) {
  const online = new Set();
  const records = new Map();
  const epochs = new Map();
  const listeners = new Set();

  const notify = () => listeners.forEach((listener) => listener());
  const nextEpoch = (deviceId) => {
    const value = (epochs.get(deviceId) || 0) + 1;
    epochs.set(deviceId, value);
    return value;
  };
  const cancelTimer = (record) => {
    if (record?.timer != null) clearTimer(record.timer);
  };
  const forget = (deviceId, { cancel = false } = {}) => {
    const record = records.get(deviceId);
    cancelTimer(record);
    records.delete(deviceId);
    nextEpoch(deviceId);
    if (cancel) cancelAttempt(deviceId);
    if (record) notify();
  };
  const publicRecord = (record) => record && ({
    deviceId: record.deviceId,
    status: record.status,
    failedAttempts: record.failedAttempts,
    nextAttemptAt: record.nextAttemptAt,
  });
  const start = (deviceId, failedAttempts, epoch, { defer = false } = {}) => {
    if (!online.has(deviceId) || epochs.get(deviceId) !== epoch) return;
    const prior = records.get(deviceId);
    cancelTimer(prior);
    records.set(deviceId, { deviceId, status: "attempting", failedAttempts, nextAttemptAt: null, timer: null, epoch });
    notify();
    const run = () => {
      if (!online.has(deviceId) || epochs.get(deviceId) !== epoch) return;
      attempt(deviceId, epoch);
    };
    if (defer) setTimer(run, 0);
    else run();
  };

  return {
    syncPresence(devices) {
      const nextOnline = new Set(devices.filter((device) => device.status === "online").map((device) => device.id));
      const previouslyTracked = new Set([...online, ...records.keys()]);
      // Publish ineligibility before cancellation: cancelling provisional
      // resources may synchronously report their loss, and that report must not
      // be able to re-arm recovery for a device this read just stood down.
      online.clear();
      for (const deviceId of nextOnline) online.add(deviceId);
      for (const deviceId of previouslyTracked) {
        if (!nextOnline.has(deviceId)) forget(deviceId, { cancel: true });
      }
    },

    /**
     * Something changed that makes the current wait pointless: retry now, from
     * the floor.
     *
     * The screen came back, the network came back, the radio changed. None of
     * those is evidence about the machine on the other end, but all of them mean
     * the reason the last dial failed has probably gone — and a ladder climbed
     * while the phone was asleep is a ladder built out of the phone's own
     * absence. So the failure count is dropped as well as the timer: waiting
     * longer each time is only sound when each failure told us something.
     *
     * Devices mid-attempt are left alone. One is already dialling, and starting a
     * second would be the double mint this issue also asks about.
     *
     * Returns the devices it woke, which is what the test reads and what the
     * diagnostics record.
     */
    wake(reason = "wake") {
      const waiting = [...records.values()].filter((record) => record.status === "waiting");
      for (const record of waiting) {
        const epoch = nextEpoch(record.deviceId);
        start(record.deviceId, 0, epoch, { defer: true });
      }
      return { reason, woke: waiting.map((record) => record.deviceId) };
    },

    recoverNow(deviceId) {
      if (!online.has(deviceId)) return null;
      const epoch = nextEpoch(deviceId);
      start(deviceId, 0, epoch, { defer: true });
      return epoch;
    },

    /** Mark a caller-owned immediate attempt, such as the reader's Retry. */
    beginAttempt(deviceId, { resetFailures = false } = {}) {
      if (!online.has(deviceId)) return null;
      const previous = records.get(deviceId);
      cancelTimer(previous);
      const epoch = nextEpoch(deviceId);
      records.set(deviceId, {
        deviceId,
        status: "attempting",
        failedAttempts: resetFailures ? 0 : previous?.failedAttempts || 0,
        nextAttemptAt: null,
        timer: null,
        epoch,
      });
      notify();
      return epoch;
    },

    failed(deviceId, { epoch = epochs.get(deviceId), retryable = true } = {}) {
      if (epochs.get(deviceId) !== epoch) return;
      if (!retryable || !online.has(deviceId)) {
        forget(deviceId);
        return;
      }
      const previous = records.get(deviceId);
      const failedAttempts = (previous?.failedAttempts || 0) + 1;
      const base = BACKOFF_MS[Math.min(failedAttempts - 1, BACKOFF_MS.length - 1)];
      const jitter = Math.round(base * JITTER_FRACTION * (random() * 2 - 1));
      const delay = Math.min(BACKOFF_MS.at(-1), Math.max(0, base + jitter));
      const nextAttemptAt = now() + delay;
      const timer = setTimer(() => start(deviceId, failedAttempts, epoch), delay);
      cancelTimer(previous);
      records.set(deviceId, { deviceId, status: "waiting", failedAttempts, nextAttemptAt, timer, epoch });
      notify();
    },

    connected(deviceId, { epoch } = {}) {
      if (epoch != null && epochs.get(deviceId) !== epoch) return;
      forget(deviceId);
    },

    stop(deviceId, { cancel = true } = {}) {
      online.delete(deviceId);
      forget(deviceId, { cancel });
    },

    reset() {
      const deviceIds = new Set([...online, ...records.keys()]);
      online.clear();
      for (const deviceId of deviceIds) forget(deviceId, { cancel: true });
    },

    epoch: (deviceId) => epochs.get(deviceId) || 0,
    snapshot(deviceId) {
      if (deviceId !== undefined) return publicRecord(records.get(deviceId)) || null;
      return [...records.values()].map(publicRecord);
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
