// Cache-only reference sources. Point records are read only for explicitly
// admitted mounted content; this feed never discovers or dispatches lookups.
import { deviceKey } from "./deviceKey.js";
import { readCached, readCachedMany, subscribeCache } from "./localCache.js";
import { tasksAddress } from "./trackerCache.js";
import { readsAddress } from "./taskReadOrder.js";
import { taskLookupAddress } from "./taskAggregateReads.js";
import { subscribeTaskReferenceDemand } from "./taskReferenceDemand.js";
import { holdReferenceSources } from "./referenceIndex.js";

export function feedReferenceIndex({ subscribeFeed }) {
  let feed = null;
  let stopped = false;
  const tasks = {}, tasksComplete = {}, taskLookups = {};
  const watched = new Map();
  const points = new Map();
  const publish = () => {
    if (!stopped) holdReferenceSources({ feed, tasks: { ...tasks }, tasksComplete: { ...tasksComplete }, taskLookups: { ...taskLookups } });
  };

  function watch(project) {
    const key = project.projectKey;
    if (!key || watched.has(key)) return;
    const { deviceId, id: projectId } = project;
    const address = tasksAddress(deviceId, projectId);
    let serial = 0;
    const reread = async () => {
      const read = ++serial;
      const [cached, note] = await readCachedMany([address, readsAddress(address)]);
      const record = cached?.value;
      if (stopped || !watched.has(key) || read !== serial) return;
      if (Array.isArray(record?.tasks)) tasks[key] = record.tasks;
      else delete tasks[key];
      // Legacy stretches carry no coherent basis/session proof.
      tasksComplete[key] = false;
      publish();
    };
    const unsubscribes = [subscribeCache(address, () => void reread()), subscribeCache(readsAddress(address), () => void reread())];
    watched.set(key, () => { serial += 1; unsubscribes.forEach((unsubscribe) => unsubscribe()); });
    void reread();
  }

  function watchPoint(scope, number) {
    const address = taskLookupAddress(scope, number);
    const key = JSON.stringify(address);
    if (points.has(key)) return;
    const owner = deviceKey(scope.deviceId, scope.projectId);
    let serial = 0;
    const reread = async () => {
      const read = ++serial;
      const record = await readCached(address);
      if (stopped || !points.has(key) || read !== serial) return;
      const result = record?.value?.result;
      if (!result) return; // Refusal, reservation and cache failure retain known answers.
      taskLookups[owner] = { ...taskLookups[owner], [number]: result };
      publish();
    };
    points.set(key, { owner, number, unsubscribe: () => { serial += 1; unsubscribe(); } });
    const unsubscribe = subscribeCache(address, () => void reread());
    void reread();
  }

  const unsubscribeDemand = subscribeTaskReferenceDemand((demands) => {
    const wanted = new Set(demands.flatMap(({ scope, numbers }) => numbers.map((number) => JSON.stringify(taskLookupAddress(scope, number)))));
    let removed = false;
    for (const [key, point] of points) {
      if (wanted.has(key)) continue;
      point.unsubscribe();
      points.delete(key);
      const retained = { ...taskLookups[point.owner] };
      delete retained[point.number];
      if (Object.keys(retained).length) taskLookups[point.owner] = retained;
      else delete taskLookups[point.owner];
      removed = true;
    }
    if (removed) publish();
    for (const { scope, numbers } of demands) for (const number of numbers) watchPoint(scope, number);
  });
  const unsubscribeFeed = subscribeFeed((next) => {
    if (stopped) return;
    feed = next;
    (next?.projects || []).forEach(watch);
    publish();
  });

  return () => {
    stopped = true;
    unsubscribeFeed();
    unsubscribeDemand();
    watched.forEach((unsubscribe) => unsubscribe());
    points.forEach((point) => point.unsubscribe());
    watched.clear();
    points.clear();
  };
}
