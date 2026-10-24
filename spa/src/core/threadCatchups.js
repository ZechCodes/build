// One forward/repair reader per conversation generation. A burst contributes
// one trailing high-water round; callers never wait for a globally idle queue.
const MAX_ATTEMPTS = 2;
const sequenceOf = (request) => Number(request.sequence || 0);
const requestActive = (request) => !request.active || request.active();

function roundFor(request) {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { request, promise, resolve };
}

function abandon(entry) {
  entry.valid = false;
  entry.current?.resolve();
  entry.pending?.resolve();
  entry.current = null;
  entry.pending = null;
}

export function createThreadCatchups({ needed, read }) {
  const entries = new Map();

  async function stillNeeded(round, active) {
    if (!active()) return false;
    try { return Boolean(await needed(round.request, active)) && active(); }
    catch { return active(); }
  }

  async function attemptRound(entry, round, active) {
    let attempts = 0;
    while (await stillNeeded(round, active)) {
      if (sequenceOf(round.request) <= entry.exhaustedThrough || attempts === MAX_ATTEMPTS) {
        entry.exhaustedThrough = Math.max(entry.exhaustedThrough, sequenceOf(round.request));
        return;
      }
      attempts += 1;
      try { await read(round.request, active); }
      catch { /* The canonical obligation, rather than RPC success, settles work. */ }
      // Newer evidence has its own round. Let current callers finish rather
      // than repeatedly extending their wait with continuously arriving tips.
      if (entry.pending) return;
    }
  }

  async function run(entry, round) {
    const active = () => entry.valid && entry.current === round && requestActive(round.request);
    try { await attemptRound(entry, round, active); }
    finally {
      round.resolve();
      if (entry.current === round) {
        entry.current = null;
        const next = entry.pending;
        entry.pending = null;
        if (next && entry.valid) start(entry, next);
      }
    }
  }

  function start(entry, round) {
    entry.current = round;
    void run(entry, round);
    return round.promise;
  }

  function entryFor(key, request) {
    let entry = entries.get(key);
    if (!entry || entry.generation !== request.generation
      || entry.current && !requestActive(entry.current.request)) {
      if (entry) abandon(entry);
      entry = { generation: request.generation, valid: true, current: null, pending: null, exhaustedThrough: -1 };
      entries.set(key, entry);
    }
    return entry;
  }

  return {
    enqueue(key, request) {
      const entry = entryFor(key, request);
      if (!entry.current) return start(entry, roundFor(request));
      if (sequenceOf(request) <= sequenceOf(entry.current.request)) return entry.current.promise;
      entry.pending ||= roundFor(request);
      if (sequenceOf(request) > sequenceOf(entry.pending.request)) entry.pending.request = request;
      return entry.pending.promise;
    },
    snapshot: () => ({
      running: [...entries.values()].filter((entry) => entry.current).length,
      pending: [...entries.values()].filter((entry) => entry.pending).length,
    }),
    clear() {
      for (const entry of entries.values()) abandon(entry);
      entries.clear();
    },
  };
}
