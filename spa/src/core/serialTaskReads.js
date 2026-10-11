// The pass can pause a project's task walk between its first page and the
// inbox. Push rereads share this queue, so a phase cannot overwrite their
// debt in trailingRead. A new session starts without waiting for old RPCs.

const invoke = (operation) => {
  try {
    return Promise.resolve(operation());
  } catch (error) {
    return Promise.reject(error);
  }
};

/** Serialize distinct operations by project and generation. Push callers
 * coalesce separately with trailingRead; this queue retains every operation.
 * Replaced generations discard operations that have not started. Started
 * reads still require their caller's normal authority fence before writing. */
export function createTaskReadQueue() {
  const queues = new Map();
  return (key, generation, operation) => {
    let queue = queues.get(key);
    if (!queue || queue.generation !== generation) {
      queue = { generation, done: null };
      queues.set(key, queue);
    }
    const start = () => queues.get(key) === queue ? invoke(operation) : undefined;
    const done = queue.done ? queue.done.then(start, start) : start();
    queue.done = done;
    const retire = () => {
      if (queues.get(key) === queue && queue.done === done) queues.delete(key);
    };
    void done.then(retire, retire);
    return done;
  };
}

export const serializeTaskRead = createTaskReadQueue();
