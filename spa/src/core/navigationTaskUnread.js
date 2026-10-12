// Destination badges follow watched task lists and timelines from cache.
// Unlike the rail's attention follower, navigation never refreshes a detail
// or reads device sessions: cache announcements supply every update.
import { subscribeCache } from "./localCache.js";
import { tasksAddress, readTasksRecord } from "./trackerCache.js";
import { createTrackerTaskDetailsFeed } from "./trackerTaskDetailsFeed.js";
import { countsInTotals, taskUnreadTally } from "./taskUnread.js";

function followProject(project, onChange) {
  const { deviceId, id: projectId } = project;
  let tasks = [];
  let reads = 0;
  let disposed = false;
  const changed = () => { if (!disposed) onChange(); };
  const details = createTrackerTaskDetailsFeed({ deviceId, projectId, onChange: changed });

  async function reread() {
    const read = ++reads;
    const record = await readTasksRecord(deviceId, projectId);
    if (disposed || read !== reads) return;
    tasks = record?.tasks || [];
    changed();
    await details.updateTasks(tasks.filter((task) => task.watched === true && countsInTotals(task)));
  }

  const unsubscribe = subscribeCache(tasksAddress(deviceId, projectId), () => void reread());
  void reread();
  return {
    source: () => ({ project, tasks, details: details.read() }),
    dispose() {
      disposed = true;
      unsubscribe();
      details.dispose();
    },
  };
}

/** Follow every catalog project, independent of device filtering or sessions.
 * taskUnread() synchronously returns the latest cached ownership tally. */
export function followNavigationTaskUnread({ onChange = () => {} } = {}) {
  const followed = new Map();
  let disposed = false;

  function drop(key) {
    followed.get(key)?.dispose();
    followed.delete(key);
  }

  function follow(projects = []) {
    if (disposed) return;
    const wanted = new Map(projects.map((project) => [project.projectKey, project]));
    let removed = false;
    for (const key of followed.keys()) {
      if (wanted.has(key)) continue;
      drop(key);
      removed = true;
    }
    for (const [key, project] of wanted) {
      if (!followed.has(key)) followed.set(key, followProject(project, onChange));
    }
    if (removed) onChange();
  }

  return {
    follow,
    taskUnread: () => taskUnreadTally([...followed.values()].map((one) => one.source())),
    dispose() {
      disposed = true;
      for (const key of followed.keys()) drop(key);
    },
  };
}
