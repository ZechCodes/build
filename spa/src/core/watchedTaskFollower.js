// The inbox's watched tasks (#125), followed through the cache.
//
// Each project the rail lists has a cached `tasks.list` record, which the
// sync layer re-reads on every pass and on every `tasks` push
// (core/cacheSync.js). This follows those records, and keeps a cached
// `tasks.get` record for every watched task that is still open — the
// timeline is what says whether an agent has said something new — reading
// one only when the list says the task has moved since. A read's answer
// only writes the cache; the rail repaints on the cache's announcement.
//
// Every project the rail lists is followed, whether or not its machine has
// greeted this tab yet: what makes a row is the cached records, and a cold
// reload paints them before any machine answers. The watch is proof enough of
// the bridge — only one that carries watching ever marks a task `watched`,
// and only watched tasks are rows or have their details read.

import { subscribeCache } from "./localCache.js";
import { contextFor } from "./deviceContexts.js";
import { isFinished } from "./trackerAgentTasks.js";
import { createTrackerTaskDetailsFeed } from "./trackerTaskDetailsFeed.js";
import { tasksAddress, readTasksRecord } from "./trackerCache.js";
import { taskAgentIsRunning, watchedTaskEntries } from "./watchedTaskRows.js";
import { taskUnreadTally } from "./taskUnread.js";
import { createWatchedTaskDependencies } from "./watchedTaskDependencies.js";
import { trailingRead } from "./trailingRead.js";

const sessionOf = (deviceId) => contextFor(deviceId)?.session || null;
const followsDetail = (task) => task?.watched === true && !isFinished(task);

/** Asks the machine for whatever session it is on when the read is made. */
const callOn = (deviceId) => (method, params) => {
  const context = contextFor(deviceId);
  return context ? context.rpc(method, params) : Promise.reject(new Error("This machine is not connected."));
};

/** One project's list and the details of its open watched tasks, and the
 *  Needs you rule its machine's tasks are read by (core/needsYouRule.js). */
function followProject(project, onChange, dependencies) {
  const { deviceId, id: projectId } = project;
  let tasks = [];
  let askedOnly = false;
  let runningAgentIds = new Set();
  let projectAgentId = null;
  let workspaces = [];
  let taskSignature = null;
  let disposed = false;
  let invalidation = 0;
  const details = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc: callOn(deviceId), onChange });

  async function readTasks() {
    const session = sessionOf(deviceId);
    const before = invalidation;
    const record = await readTasksRecord(deviceId, projectId);
    if (disposed || before !== invalidation || session !== (sessionOf(deviceId))) return;
    const next = record?.tasks || [];
    const signature = JSON.stringify(next);
    if (signature === taskSignature) return;
    taskSignature = signature;
    tasks = next;
    onChange();
    await details.updateTasks(tasks.filter(followsDetail));
  }

  const reread = trailingRead(readTasks, { generationOf: () => sessionOf(deviceId) });
  const unsubscribe = subscribeCache(tasksAddress(deviceId, projectId), (address) => {
    if (!address.kind) invalidation += 1;
    void reread();
  });
  const unsubscribeDependencies = dependencies.subscribe(project, (next) => {
    if (disposed) return;
    ({ askedOnly, runningAgentIds, projectAgentId, workspaces } = next);
    onChange();
  });
  void reread();
  return {
    // A reconnect is a new session: reads the old one refused are asked again.
    session: sessionOf(deviceId),
    source: () => ({ project, tasks, details: details.read(), askedOnly, runningAgentIds, projectAgentId, workspaces }),
    dispose() {
      disposed = true;
      unsubscribe();
      unsubscribeDependencies();
      details.dispose();
    },
  };
}

const projectKeyOf = (project) => `${project.deviceId}|${project.id}`;

/**
 * Follow the watched tasks of `projects` as the rail lists them. `onChange`
 * is called whenever a followed record changes; `entries()` answers the rows
 * as the cache holds them now, and `taskUnread()` the watched tasks' unread
 * (core/taskUnread.js). Call `follow(projects)` whenever the rail's
 * projects or a machine's connection change.
 */
export function followWatchedTasks({ onChange = () => {} } = {}) {
  const followed = new Map();
  const devices = new Map();

  function drop(key) {
    followed.get(key)?.dispose();
    followed.delete(key);
  }

  function follow(projects = []) {
    const wanted = new Map(projects.map((project) => [projectKeyOf(project), project]));
    for (const key of followed.keys()) if (!wanted.has(key)) drop(key);
    for (const [key, project] of wanted) {
      const standing = followed.get(key);
      if (standing && standing.session === sessionOf(project.deviceId)) continue;
      drop(key);
      if (!devices.has(project.deviceId)) devices.set(project.deviceId, createWatchedTaskDependencies(project.deviceId));
      followed.set(key, followProject(project, onChange, devices.get(project.deviceId)));
    }
    retireUnusedDevices(projects);
  }

  function retireUnusedDevices(projects) {
    const deviceIds = new Set(projects.map((project) => project.deviceId));
    for (const [deviceId, dependency] of devices) {
      if (deviceIds.has(deviceId)) continue;
      dependency.dispose();
      devices.delete(deviceId);
    }
  }

  const sources = () => [...followed.values()].map((one) => one.source());

  return {
    follow,
    entries: () => watchedTaskEntries(sources()),
    /** Where the rail wears each watched task's unread (#104). */
    taskUnread: () => taskUnreadTally(sources()),
    /** Folded project heads also carry running watched tasks with no Needs-you
     *  row. A task's status alone never says its agent is running. */
    runningProjectKeys: () => new Set(sources()
      .filter(({ tasks, runningAgentIds, projectAgentId }) => tasks.some((task) => followsDetail(task) && taskAgentIsRunning(task, runningAgentIds, projectAgentId)))
      .map(({ project }) => project.projectKey)),
    dispose() {
      [...followed.keys()].forEach(drop);
      devices.forEach((dependency) => dependency.dispose());
      devices.clear();
    },
  };
}
