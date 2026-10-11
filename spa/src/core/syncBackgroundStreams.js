// Two ordered checkout workers share their device's wire slots with inbox and
// push refreshes. Foreground reads use the planner's barrier and no wire slot.

const STREAM_COUNT = 2;
const NOTHING = () => {};
const WITHOUT_SLOT = { reserved: false, release: NOTHING };
const RETRY = Symbol("background admission required");
const background = (envelope) => envelope?.priority === "background";

async function sendWithSlot({ active, priority, before, load }, slot) {
  try {
    if (!active()) return null;
    // Navigation can change while a read waits for a slot. Check promotion
    // before following the new barrier, which may be waiting for this read.
    if (background(priority())) await before?.();
    const envelope = priority();
    if (background(envelope) && !slot.reserved) return RETRY;
    if (!background(envelope)) slot.release();
    return active() ? await load(envelope) : null;
  } finally {
    slot.release();
  }
}

function deviceStreams() {
  const waiting = new Set();
  let occupied = 0, closed = false;
  const reserve = () => {
    occupied += 1;
    let released = false;
    return {
      reserved: true,
      release() {
        if (released) return;
        released = true;
        occupied -= 1;
        wake();
      },
    };
  };
  const wake = () => {
    for (const read of waiting) {
      if (!read.active() || !background(read.priority())) {
        waiting.delete(read);
        read.resolve(WITHOUT_SLOT);
      } else if (occupied < STREAM_COUNT) {
        waiting.delete(read);
        read.resolve(reserve());
      }
    }
  };
  return {
    wake,
    close() { closed = true; wake(); },
    async read(operation) {
      const active = () => !closed && operation.active();
      while (active()) {
        if (background(operation.priority())) await operation.before?.();
        const slot = background(operation.priority()) ? await new Promise((resolve) => {
          waiting.add({ ...operation, active, resolve });
          wake();
        }) : WITHOUT_SLOT;
        const answer = await sendWithSlot({ ...operation, active }, slot);
        if (answer !== RETRY) return answer;
      }
      return null;
    },
  };
}

const sameGeneration = (one, two) => one.session === two.session
  && one.requestScope === two.requestScope && one.lifetime === two.lifetime;

/** Passes and pushes on the same device/path share two background admissions.
 * Replaced sessions, restored paths and stopped lifetimes discard queued work
 * and start with fresh slots, even when abandoned RPCs never answer. */
export function createBackgroundStreams() {
  const devices = new Map();
  return {
    forContext(owner) {
      let held = devices.get(owner.deviceId);
      if (!held || !sameGeneration(held, owner)) {
        held?.streams.close();
        held = { ...owner, streams: deviceStreams() };
        devices.set(owner.deviceId, held);
      }
      return held.streams;
    },
    clear() {
      for (const held of devices.values()) held.streams.close();
      devices.clear();
    },
  };
}

/** The owning pass supplies its captured lifetime, so abandoned RPCs cannot
 *  keep a later session's streams occupied. A failed stream waits for its
 *  sibling to finish before the pass can hand off to a queued refresh. */
export async function drainBackgroundStreams({ items, active, read }) {
  const queue = items[Symbol.iterator]();
  let failed = false;
  const drain = async () => {
    let recovered = true;
    try {
      while (active() && !failed) {
        const next = queue.next();
        if (next.done) return recovered;
        const refreshed = await read(next.value);
        recovered = refreshed && recovered;
      }
      return false;
    } catch (error) {
      failed = true;
      throw error;
    }
  };
  const streams = await Promise.allSettled([drain(), drain()]);
  const failure = streams.find((stream) => stream.status === "rejected");
  if (failure) throw failure.reason;
  return active() && streams.every((stream) => stream.value);
}
