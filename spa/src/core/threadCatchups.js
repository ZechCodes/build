// One forward/repair reader per conversation generation. A burst contributes
// one trailing high-water round; callers never wait for a globally idle queue.
const MAX_ATTEMPTS = 2;
const sequenceOf = (request) => Number(request.sequence || 0);
const requestActive = (request) => !request.active || request.active();
const floorOf = (request) => Number(request.historyStartSequence || 0);
const newerGoal = (request, held) => sequenceOf(request) > sequenceOf(held) || floorOf(request) > floorOf(held);
const combinedGoal = (held, request) => ({
  ...request,
  sequence: Math.max(sequenceOf(held), sequenceOf(request)),
  historyStartSequence: Math.max(floorOf(held), floorOf(request)),
});
const exhaustedThrough = (entry, request) => floorOf(request) > entry.exhaustedFloor ? -1 : entry.exhaustedThrough;

function exhaustGoal(entry, request) {
  if (floorOf(request) > entry.exhaustedFloor) entry.exhaustedThrough = -1;
  entry.exhaustedFloor = Math.max(entry.exhaustedFloor, floorOf(request));
  entry.exhaustedThrough = Math.max(entry.exhaustedThrough, sequenceOf(request));
}


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

export function createThreadCatchups({ needed, read, onChange = () => {} }) {
  const entries = new Map();

  async function stillNeeded(round, active) {
    if (!active()) return false;
    try { return Boolean(await needed(round.request, active)) && active(); }
    catch { return active(); }
  }

  async function attemptRound(entry, round, active) {
    let attempts = 0;
    while (await stillNeeded(round, active)) {
      if (sequenceOf(round.request) <= exhaustedThrough(entry, round.request) || attempts === MAX_ATTEMPTS) {
        exhaustGoal(entry, round.request);
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
      onChange();
    }
  }

  function start(entry, round) {
    entry.current = round;
    onChange();
    void run(entry, round);
    return round.promise;
  }

  function entryFor(key, request) {
    // Keep the original lifetime even after a round idles or an identical tip
    // arrives. A retired owner must not lend its exhausted budget to a replica
    // recreated at the same generation and sequence.
    let entry = entries.get(key);
    if (!entry || entry.generation !== request.generation || !requestActive(entry.owner)
      || entry.current && !requestActive(entry.current.request)) {
      if (entry) abandon(entry);
      entry = { generation: request.generation, owner: request, valid: true, current: null, pending: null, exhaustedThrough: -1, exhaustedFloor: 0 };
      entries.set(key, entry);
    }
    return entry;
  }

  return {
    enqueue(key, request) {
      const entry = entryFor(key, request);
      if (!entry.current) return start(entry, roundFor(request));
      if (!newerGoal(request, entry.current.request)) return entry.current.promise;
      entry.pending ||= roundFor(combinedGoal(entry.current.request, request));
      if (newerGoal(request, entry.pending.request)) entry.pending.request = combinedGoal(entry.pending.request, request);
      return entry.pending.promise;
    },
    snapshot: () => ({
      running: [...entries.values()].filter((entry) => entry.current).length,
      pending: [...entries.values()].filter((entry) => entry.pending).length,
    }),
    clear() {
      for (const entry of entries.values()) abandon(entry);
      entries.clear();
      onChange();
    },
  };
}
