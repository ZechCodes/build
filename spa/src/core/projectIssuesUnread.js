// The project Issues tab's count (#104): "For the project it is all unreads" —
// the unread of every watched issue in the project, read off its cached
// `issues.list` record (core/issueUnread.js).
//
// It listens to that record, so an `issues` push moves the count with nothing
// asked of the bridge: core/cacheSync.js rewrites the record and
// core/localCache.js announces it. One project at a time — the one the bar is
// standing in.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { watchedIssuesUnread } from "./issueUnread.js";

/**
 * Follow one project's watched unread. `follow(deviceId, projectId)` moves to
 * a project, or to none with no ids; `count()` answers what the cache held at
 * the last read; `onChange` is called when that count moves.
 */
export function followProjectIssuesUnread(onChange = () => {}) {
  let standing = null;

  function stop() {
    standing?.unsubscribe();
    standing = null;
  }

  function follow(deviceId, projectId) {
    const key = deviceId && projectId ? JSON.stringify([deviceId, projectId]) : null;
    if ((standing?.key ?? null) === key) return;
    stop();
    if (!key) return;
    const current = { key, count: 0, reads: 0, unsubscribe: () => {} };
    standing = current;
    const reread = async () => {
      const read = ++current.reads;
      const record = await readIssuesRecord(deviceId, projectId);
      if (standing !== current || read !== current.reads) return;
      const count = watchedIssuesUnread(record?.issues || []);
      if (count === current.count) return;
      current.count = count;
      onChange();
    };
    current.unsubscribe = subscribeCache(issuesAddress(deviceId, projectId), () => void reread());
    void reread();
  }

  return { follow, count: () => standing?.count || 0, dispose: stop };
}
