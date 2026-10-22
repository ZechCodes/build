import { WRITE_RETRY_DELAYS } from "./writeOperationIntent.js";

// A lease suppresses duplicate tab traffic. Idempotency remains the server's
// responsibility because browser suspension can race lease expiration.
export function createWriteOperationWorker({ cache, tabId, run, hasWork, random, setTimeout: schedule, clearTimeout: cancel }) {
  let timer = null;
  let running = null;
  let holder = null;
  let generation = 0;
  let stopped = false;
  let attempt = 0;
  let queued = [];

  function arm() {
    if (stopped || timer !== null || (!hasWork() && !queued.length)) return;
    const wait = WRITE_RETRY_DELAYS[Math.min(attempt++, WRITE_RETRY_DELAYS.length - 1)];
    timer = schedule(() => { timer = null; void reconcile(); }, Math.round(wait * (.8 + .4 * random())));
  }
  function renewLease(lease) {
    if (!lease.active) return;
    cache.renewLease(lease.id, 15000).then((owned) => {
      if (!owned) loseLease(lease);
      if (owned && lease.active) lease.heartbeat = schedule(() => renewLease(lease), 5000);
    }).catch(() => loseLease(lease));
  }
  function loseLease(lease) {
    lease.active = false;
    cancel(lease.heartbeat);
    if (holder !== lease) return;
    holder = null;
    running = null;
    generation++;
    arm();
  }
  async function leasedRun(lease) {
    if (!await cache.acquireLease(lease.id, 15000)) return;
    if (!lease.active) { await cache.releaseLease(lease.id); return; }
    lease.acquired = true;
    lease.heartbeat = schedule(() => renewLease(lease), 5000);
    const tasks = queued;
    queued = [];
    for (const task of tasks) dispatchTask(lease, task);
    try {
      await runPass(lease);
      while (lease.pending.size || lease.pass) {
        const work = [...lease.pending];
        if (lease.pass) work.push(lease.pass);
        await Promise.all(work);
      }
    }
    finally {
      lease.active = false;
      cancel(lease.heartbeat);
      await cache.releaseLease(lease.id);
    }
  }
  function runPass(lease) {
    if (lease.pass) return lease.pass;
    const pass = Promise.resolve().then(() => run(() => lease.active)).catch(() => {}).finally(() => {
      if (lease.pass === pass) lease.pass = null;
      arm();
    });
    lease.pass = pass;
    return pass;
  }
  function dispatchTask(lease, task) {
    let result;
    try { result = task(() => lease.active); } catch (error) { result = Promise.reject(error); }
    const work = Promise.resolve(result).catch(() => {}).finally(() => lease.pending.delete(work));
    lease.pending.add(work);
  }
  function reconcile() {
    if (stopped) return Promise.resolve();
    if (running) return holder?.active && holder.acquired ? runPass(holder) : running;
    const lease = { id: `${tabId}:${generation}`, active: true, heartbeat: null, pending: new Set() };
    holder = lease;
    const pass = leasedRun(lease).catch(() => {}).finally(() => {
      if (running === pass) { running = null; holder = null; arm(); }
    });
    running = pass;
    return pass;
  }
  function reset() { attempt = 0; cancel(timer); timer = null; }
  async function restart() {
    reset();
    generation++;
    const previous = holder;
    holder = null;
    running = null;
    if (!previous) return;
    previous.active = false;
    cancel(previous.heartbeat);
    await cache.releaseLease(previous.id).catch(() => {});
  }
  return {
    reconcile, restart, reset,
    dispatch(task) {
      if (holder?.active && holder.acquired) {
        const lease = holder;
        dispatchTask(lease, task);
        return;
      }
      queued.push(task);
      void reconcile();
    },
    wake() { arm(); },
    async close() { stopped = true; queued = []; await restart(); },
  };
}
