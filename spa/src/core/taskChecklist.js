// Checklist writes change one marker in the cached task body. The cache's
// subscription owns the repaint; a later task read confirms the stored body.
import { mergeCachedAtomically } from "./localCache.js";
import { readTaskRecord, taskAddress } from "./trackerCache.js";
import { setMarkdownTaskChecked } from "./markdownTasks.js";

// Pending saves outlive a mounted page. The same task in another mount uses
// their authority too; a Web Lock orders the brief read snapshot and whole
// save across tabs. Without Locks, this queue still orders mounts in this tab.
const writes = new Map();
function sharedWrite(key) {
  if (!writes.has(key)) writes.set(key, { saving: false, revision: 0, tail: Promise.resolve(), listeners: new Set() });
  return writes.get(key);
}

function withWriteLock(key, state, work) {
  const run = () => globalThis.navigator?.locks
    ? navigator.locks.request(key, work) : work();
  const result = state.tail.then(run);
  state.tail = result.catch(() => {});
  return result;
}

export function createTaskChecklist({ deviceId, projectId, taskId, call, refresh, onPending, onFailure }) {
  const address = taskAddress(deviceId, projectId, taskId);
  const key = `build.taskChecklist:${JSON.stringify([deviceId, projectId, taskId])}`;
  const shared = sharedWrite(key);
  shared.listeners.add(onPending);
  const move = (saving) => {
    shared.saving = saving;
    shared.revision += 1;
    shared.listeners.forEach((listener) => listener(saving));
  };
  const release = () => {
    if (!shared.listeners.size && !shared.saving) writes.delete(key);
  };

  async function putBody(before, after) {
    return mergeCachedAtomically(address, (held) => {
      if (!held?.task || held.task.body !== before) return null;
      return { ...held, task: { ...held.task, body: after } };
    });
  }

  return {
    busy: () => shared.saving,
    readBasis: () => withWriteLock(key, shared, async () => ({
      revision: shared.revision,
      body: (await readTaskRecord(deviceId, projectId, taskId))?.task?.body ?? null,
    })),
    // A read begun before or during a write cannot undo its optimistic body.
    // The refresh after the write takes a new version and answers any push
    // that arrived while saving too.
    acceptsRead: (basis, held) => !shared.saving && basis.revision === shared.revision &&
      basis.body === (held?.task?.body ?? null),

    async press(index, checked, source) {
      if (shared.saving) return;
      const body = setMarkdownTaskChecked(source, index, checked);
      if (body === null || body === source) return;
      move(true);
      await withWriteLock(key, shared, async () => {
        let optimistic = false;
        try {
          optimistic = await putBody(source, body);
          if (!optimistic) throw new Error("The checklist changed before it could be saved. Try again.");
          await call("tasks.update", { task_id: taskId, body });
        } catch (error) {
          // Other cache writers may already have replaced this body. Revert
          // only our own value, retaining newer fields and timeline entries.
          if (optimistic) await putBody(body, source).catch(() => {});
          onFailure(error);
        } finally {
          move(false);
          release();
          void refresh();
        }
      }).catch((error) => {
        // A browser can refuse lock admission before the save callback runs.
        // No body changed, but the pending controls still need settling.
        move(false);
        release();
        onFailure(error);
        void refresh();
      });
    },
    dispose() {
      shared.listeners.delete(onPending);
      release();
    },
  };
}
