/** Counts may lag behind the transcript; a known cursor also proves progress. */
export function hasReadProgress(agent, read) {
  if (!agent || read <= 0) return false;
  return !!agent.unread_count || (Number.isFinite(agent.read_through_sequence) && read > agent.read_through_sequence);
}

// One conversation's read reports. Failed attempts are not confirmations;
// scroll/paint/return events may retry them after a capped refusal backoff.
export function createChatReadReporter(now = () => Date.now()) {
  let confirmedRead = 0;
  let confirmedFloor = Infinity;
  let retryAt = 0;
  let retryDelay = 1000;
  const pending = new Set();
  const floorOf = (floor) => typeof floor === "number" ? floor : Infinity;
  const isNews = (read, floor) => read > confirmedRead || floorOf(floor) < confirmedFloor;
  const covers = (report, read, floor) => report.read >= read && floorOf(report.floor) <= floorOf(floor);

  const confirm = (read, floor) => {
    confirmedRead = Math.max(confirmedRead, read);
    confirmedFloor = Math.min(confirmedFloor, floorOf(floor));
    retryAt = 0;
    retryDelay = 1000;
  };
  const refused = (read, floor) => {
    // A newer confirmation can cover an older failure that arrived late.
    if (!isNews(read, floor)) return;
    retryAt = now() + retryDelay;
    retryDelay = Math.min(retryDelay * 2, 30_000);
  };

  return {
    async report(read, floor, send) {
      if (!isNews(read, floor) || now() < retryAt) return false;
      if ([...pending].some((report) => covers(report, read, floor))) return false;
      const report = { read, floor };
      pending.add(report);
      try {
        if (await send()) {
          confirm(read, floor);
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
