// What fills the reference index (#229): the feed as core/taskFeed.js delivers
// it, and every project's task list as the tracker has written it to the
// cache. Each is a cache read and nothing else — the sync layer is the one
// reader of the wire — and each write the cache announces is re-read, so a list
// that lands after a paint still reaches every surface that subscribed to the
// index (core/referenceIndex.js).
//
// The feed's subscription is handed in, so this runs without the app around it
// (views/gate.js starts it beside the feed).

import { deviceKey } from "./deviceKey.js";
import { subscribeCache } from "./localCache.js";
import { readTasksRecord, tasksAddress } from "./trackerCache.js";
import { holdReferenceSources } from "./referenceIndex.js";

/**
 * Keep the index filled until the returned stop is called.
 *
 * `subscribeFeed` is core/taskFeed.js's: it hands every listener the merge of
 * every machine's records, now and on each change.
 */
export function feedReferenceIndex({ subscribeFeed }) {
  let feed = null;
  let stopped = false;
  const tasks = {};
  const watched = new Map(); // project key → its task list's cache subscription

  const publish = () => {
    if (!stopped) holdReferenceSources({ feed, tasks: { ...tasks } });
  };

  const reread = async (deviceId, projectId) => {
    const record = await readTasksRecord(deviceId, projectId);
    if (stopped || !Array.isArray(record?.tasks)) return;
    tasks[deviceKey(deviceId, projectId)] = record.tasks;
    publish();
  };

  const watch = (project) => {
    const key = project.projectKey;
    if (!key || watched.has(key)) return;
    const { deviceId, id: projectId } = project;
    watched.set(key, subscribeCache(tasksAddress(deviceId, projectId), () => void reread(deviceId, projectId)));
    void reread(deviceId, projectId);
  };

  const unsubscribeFeed = subscribeFeed((next) => {
    if (stopped) return;
    feed = next;
    (next?.projects || []).forEach(watch);
    publish();
  });

  return () => {
    stopped = true;
    unsubscribeFeed();
    watched.forEach((unsubscribe) => unsubscribe());
    watched.clear();
  };
}
