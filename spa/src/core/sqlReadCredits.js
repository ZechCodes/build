// Sender admission belongs to the opening, across priorities and carriers.
// Caller settlement is separate: a timed-out or cancelled sent read still
// occupies the bridge until its final reply, carrier close, or restore reset.
export const SQL_READ_METHODS = Object.freeze([
  "thread.page", "thread.activity", "thread.operation",
  "tasks.list", "tasks.get", "tasks.waiting_on.blocks",
  "writes.status", "writes.subscribe",
]);
export const SQL_READ_LIMIT = 8;
const methods = new Set(SQL_READ_METHODS);
export const isSqlRead = (method) => methods.has(method);

export function createSqlReadCredits(limit = SQL_READ_LIMIT) {
  const queued = new Map();
  const admitted = new Map();

  const drain = () => {
    while (admitted.size < limit && queued.size) {
      const [id, entry] = queued.entries().next().value;
      queued.delete(id);
      admitted.set(id, entry);
      entry.start();
    }
  };
  const find = (id) => admitted.get(id) || queued.get(id);
  const remove = (id) => {
    admitted.delete(id);
    queued.delete(id);
  };
  const discard = (matches, error) => {
    const removed = [...admitted, ...queued].filter(([, entry]) => matches(entry));
    // Remove the whole group before releasing any credit; a queued request
    // on the closing carrier must never start between two removals.
    for (const [id] of removed) remove(id);
    for (const [, entry] of removed) entry.reject(error);
    drain();
  };

  return {
    reserve(id, start, reject) {
      queued.set(id, { start, reject, carrier: null, sent: false });
      drain();
    },
    bind(id, carrier) {
      const entry = find(id);
      if (entry) entry.carrier = carrier;
    },
    sent(id) {
      const entry = admitted.get(id);
      if (entry) entry.sent = true;
    },
    holds: (id) => Boolean(find(id)),
    accepts(id, carrier) {
      const entry = admitted.get(id);
      return Boolean(entry?.sent && entry.carrier === carrier);
    },
    finish(id) {
      remove(id);
      drain();
    },
    cancel(id) {
      if (find(id)?.sent) return;
      remove(id);
      drain();
    },
    releaseCarrier: (carrier, error) => discard((entry) => entry.carrier === carrier, error),
    reset: (error) => discard(() => true, error),
  };
}
