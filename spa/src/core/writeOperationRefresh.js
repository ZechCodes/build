import { readCanonicalCached, recordWriteOf } from "./localCache.js";
import { threadWindow } from "./threadSync.js";

const threadParams = (record) => {
  const params = record.request.params;
  const names = ["entity_id", "agent_id", "conversation_id", "thread_id", "thread_generation_revision"];
  return { ...Object.fromEntries(names.filter((name) => params[name] !== undefined).map((name) => [name, params[name]])), newest: true, limit: 100 };
};

const READERS = {
  "tracker-task": async (address, record, authority) => {
    const result = await authority.call("tasks.get", { task_id: address.sub });
    if (!result?.task || !Array.isArray(result.timeline)) throw new Error("Task refresh is incomplete");
    return { task: result.task, timeline: result.timeline };
  },
  thread: async (_address, record, authority, previous) => {
    const page = await authority.call("thread.page", threadParams(record));
    if (!Array.isArray(page?.items)) throw new Error("Conversation refresh is incomplete");
    const window = threadWindow(previous?.value, page, { newest: true });
    if (!window) throw new Error("Conversation generation changed during refresh");
    return window;
  },
};

/** A confirmation is an invalidation until its precise record is read anew.
 * Capture the canonical write first: a concurrent push or reset wins over this
 * read, and an old opening cannot publish into its replacement. */
export async function refreshWriteOperation({ cache, frame, record, authority }) {
  if (!authority?.active()) return;
  const addresses = new Map((record.overlay || []).map(({ address }) => [JSON.stringify(address), address]));
  for (const address of addresses.values()) {
    const read = READERS[address.kind];
    if (!read) continue;
    const previous = await readCanonicalCached(address);
    if (!authority.active()) return;
    const value = await read(address, record, authority, previous);
    if (!authority.active()) return;
    await cache.observePublication({
      accept: authority.active,
      canonical: [{ address, value, publication: frame.published || [], guard: { written: recordWriteOf(previous) } }],
    });
  }
  if (frame.state === "succeeded" && (await cache.get(record.operationId))?.overlayActive) {
    throw new Error("The canonical publication has not reached the cache yet");
  }
}
