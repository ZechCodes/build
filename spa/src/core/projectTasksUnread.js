// The project Tasks tab's count (#104): "For the project it is all unreads" —
// the unread of every watched task in the project that is not finished
// (#183), read off its cached list and task details (core/taskUnread.js).
//
// A task page's accepted read updates its detail before the list refreshes.
// Follow both records so this badge agrees with Inbox throughout that gap.
// Cache announcements supply every update; this follower asks for no reads.

import { subscribeCache } from "./localCache.js";
import { tasksAddress, readTasksRecord } from "./trackerCache.js";
import { countsInTotals, watchedTasksUnread } from "./taskUnread.js";
import { createTrackerTaskDetailsFeed } from "./trackerTaskDetailsFeed.js";

/**
 * Follow one project's watched unread. `follow(deviceId, projectId)` moves to
 * a project, or to none with no ids; `count()` answers what the cache held at
 * the last read; `onChange` is called when that count moves.
 */
export function followProjectTasksUnread(onChange = () => {}) {
  let standing = null;

  function stop() {
    standing?.unsubscribe();
    standing?.details.dispose();
    standing = null;
  }

  function recount(current) {
    if (standing !== current) return;
    const details = current.details.read();
    const count = watchedTasksUnread(current.tasks, { detailOf: (task) => details.get(task.id) });
    if (count === current.count) return;
    current.count = count;
    onChange();
  }

  function follow(deviceId, projectId) {
    const key = deviceId && projectId ? JSON.stringify([deviceId, projectId]) : null;
    if ((standing?.key ?? null) === key) return;
    stop();
    if (!key) return;
    const current = { key, count: 0, reads: 0, tasks: [], unsubscribe: () => {} };
    standing = current;
    current.details = createTrackerTaskDetailsFeed({
      deviceId, projectId, onChange: () => recount(current),
    });
    const reread = async () => {
      const read = ++current.reads;
      const record = await readTasksRecord(deviceId, projectId);
      if (standing !== current || read !== current.reads) return;
      current.tasks = record?.tasks || [];
      await current.details.updateTasks(current.tasks.filter((task) => task.watched === true && countsInTotals(task)));
      if (read === current.reads) recount(current);
    };
    current.unsubscribe = subscribeCache(tasksAddress(deviceId, projectId), () => void reread());
    void reread();
  }

  return { follow, count: () => standing?.count || 0, dispose: stop };
}
