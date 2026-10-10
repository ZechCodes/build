/** Counts may lag behind the transcript; a known cursor also proves progress. */
export function hasReadProgress(agent, read) {
  if (!agent || read <= 0) return false;
  return !!agent.unread_count || (Number.isFinite(agent.read_through_sequence) && read > agent.read_through_sequence);
}

// One conversation's read reports. Failed attempts are not confirmations;
// scroll/paint/return events may retry them after a capped refusal backoff.
export function createChatReadReporter(now = () => Date.now()) {
  const confirmed = new Set();
  const accepted = new Set();
  let retryAt = 0;
  let retryDelay = 1000;
  const pending = new Set();
  const floorOf = (floor) => typeof floor === "number" ? floor : Infinity;
  const covers = (report, read, floor) => report.read >= read && floorOf(report.floor) <= floorOf(floor);
  const isNews = (read, floor) => ![...confirmed].some((report) => covers(report, read, floor));

  const confirm = (read, floor) => {
    // A tail and a separate history report do not prove the gap between them
    // was read. Retain actual pairs, dropping only pairs this one covers.
    for (const report of confirmed) {
      if (covers({ read, floor }, report.read, report.floor)) confirmed.delete(report);
    }
    if (isNews(read, floor)) confirmed.add({ read, floor });
    retryAt = 0;
    retryDelay = 1000;
  };
  const refused = (read, floor) => {
    // A newer confirmation can cover an older failure that arrived late.
    if (!isNews(read, floor)) return;
    retryAt = now() + retryDelay;
    retryDelay = Math.min(retryDelay * 2, 30_000);
  };

  // Acceptance alone may leave the cursor behind unread history below the
  // floor. Suppress duplicates at that cursor, but reconsider them when the
  // roster reports progress; only actual cursor coverage confirms a pair.
  const reconcile = (cursor) => {
    for (const report of accepted) {
      if (Number.isFinite(cursor) && cursor >= report.read) {
        confirm(report.read, report.floor);
        accepted.delete(report);
      } else if (cursor !== report.cursor) {
        accepted.delete(report);
      }
    }
  };

  return {
    async report(read, floor, send, observedCursor = () => undefined) {
      const cursor = observedCursor();
      reconcile(cursor);
      if (!isNews(read, floor) || now() < retryAt) return false;
      if ([...accepted].some((report) => covers(report, read, floor))) return false;
      if ([...pending].some((report) => covers(report, read, floor))) return false;
      const report = { read, floor };
      pending.add(report);
      try {
        if (await send()) {
          const current = observedCursor();
          if (Number.isFinite(current) && current >= read) confirm(read, floor);
          else accepted.add({ read, floor, cursor });
          retryAt = 0;
          retryDelay = 1000;
          return true;
        }
        refused(read, floor);
      } catch {
        refused(read, floor);
      } finally {
        pending.delete(report);
      }
      return false;
    },
  };
}
