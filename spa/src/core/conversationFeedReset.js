// The board's feed keeps copies of conversation digests beside their rows.
// A reset retires those copies too, and a delayed list cannot restore them.
import { FEED_COLLECTIONS } from "./feedMerge.js";
import { entityIdOf } from "./entityId.js";
import { recordKey } from "./idbRecords.js";
import { cachedWriteOf, captureCachedRecord, updateCachedFeed } from "./localCache.js";

const rosterOf = (agents) => Array.isArray(agents) ? agents : [];
const threadAgent = (thread) => thread?.agent?.id ? [{ id: thread.agent.id, thread_id: thread.thread_id || thread.id,
  thread_generation_revision: thread.thread_generation_revision }] : [];
const agentsOf = (row) => [...rosterOf(row.agents), ...rosterOf(row.run?.agents), ...threadAgent(row.thread), ...threadAgent(row.run?.thread)];
const canonicalId = (agent) => agent.conversation_id || agent.id;
const feedAddress = (deviceId) => ({ deviceId, entityId: "", kind: "feed", sub: "" });
const threadAddress = (deviceId, entityId, conversationId) => ({ deviceId, entityId, kind: "thread", sub: conversationId });

function mapFeedRows(view, rewrite) {
  return { ...view, ...Object.fromEntries(FEED_COLLECTIONS.filter((field) => Array.isArray(view?.[field]))
    .map((field) => [field, view[field].map(rewrite)])) };
}

const ownsConversation = (row, ownership) => agentsOf(row).some((agent) => canonicalId(agent) === ownership.conversationId)
  || row.thread?.id === ownership.threadId || row.run?.thread?.id === ownership.threadId;

const emptyThread = (agent) => ({ id: agent.thread_id, thread_id: agent.thread_id,
  thread_generation_revision: agent.thread_generation_revision || 0, items: [], sessions: [], revisions: [] });

const resetAnswer = (ownership, agent) => ({ agent, conversation_id: ownership.conversationId,
  previous_thread_id: ownership.threadId, thread: emptyThread(agent) });

/** Retire only this canonical conversation's copies, guarded against another
 * reset advancing the shared history while this cleanup waits. */
export function retireConversationFeed(ownership, agent, rewriteRow) {
  const address = feedAddress(ownership.deviceId);
  const guard = threadAddress(ownership.deviceId, ownership.entityId, ownership.conversationId);
  return guardedFeedWrite(address, [guard], { active: () => true, makeFeed: (held, [thread]) => {
    if (thread?.value?.thread_id !== agent.thread_id || !held) return null;
    const answer = resetAnswer(ownership, agent);
    return mapFeedRows(held, (row) => ownsConversation(row, ownership) ? rewriteRow(row, answer) : row);
  } });
}

/** The existing feed transaction compares every guard before invoking its
 * update. Retry a changed snapshot; never split the check from the write. */
async function guardedFeedWrite(address, guards, { active, makeFeed, options = {} }) {
  while (active()) {
    const found = await Promise.all(guards.map(captureCachedRecord));
    if (found.some((record) => !cachedWriteOf(record))) return false;
    const unchanged = guards.map((at, index) => ({ address: at, written: cachedWriteOf(found[index]) }));
    let attempted = false;
    const written = await updateCachedFeed(address, (held) => {
      attempted = true;
      return active() ? makeFeed(held, found) : null;
    }, { ...options, unchanged });
    if (written || attempted) return written;
  }
  return false;
}

function feedGuards(deviceId, view) {
  const guards = new Map();
  for (const field of FEED_COLLECTIONS) {
    for (const row of view[field] || []) {
      const entityId = entityIdOf(row);
      if (!entityId) continue;
      const ownRow = { deviceId, entityId, kind: "row", sub: "" };
      guards.set(recordKey(ownRow), ownRow);
      for (const agent of agentsOf(row)) {
        const at = threadAddress(deviceId, entityId, canonicalId(agent));
        guards.set(recordKey(at), at);
      }
    }
  }
  return [...guards.values()];
}

const currentRow = (deviceId, entityId, records) => records.get(recordKey({ deviceId, entityId, kind: "row", sub: "" }))?.value;

function currentConversation(deviceId, entityId, agent, records) {
  const canonical = canonicalId(agent);
  const own = currentRow(deviceId, entityId, records);
  const latestAgent = agentsOf(own || {}).find((held) => canonicalId(held) === canonical);
  const thread = records.get(recordKey(threadAddress(deviceId, entityId, canonical)))?.value;
  return { thread: thread?.thread_id ? thread : latestAgent, own };
}

const retiredDigest = (agent, thread) => thread?.thread_id && (agent.thread_id
  ? agent.thread_id !== thread.thread_id : Number(thread.thread_generation_revision || 0) > 0);

function admittedFeedRow(row, deviceId, records, rewriteRow) {
  const entityId = entityIdOf(row);
  let next = row;
  const seen = new Set();
  for (const agent of agentsOf(row)) {
    const conversationId = canonicalId(agent);
    if (seen.has(conversationId)) continue;
    seen.add(conversationId);
    const { thread, own } = currentConversation(deviceId, entityId, agent, records);
    if (!retiredDigest(agent, thread)) continue;
    const current = agentsOf(own || {}).find((held) => held.id === agent.id && held.thread_id === thread.thread_id);
    const digest = current || { id: "", thread_id: thread.thread_id, thread_generation_revision: thread.thread_generation_revision };
    next = rewriteRow(next, resetAnswer({ conversationId, threadId: agent.thread_id || `thread:${conversationId}` }, digest));
  }
  return next;
}

/** Admit a board snapshot against all its thread generations inside the same
 * transaction as the feed write. Other rows remain this list's observation. */
export function writeConversationFeed(address, view, { active, supersededFeedRow, rewriteRow, pruneView = (held) => held }) {
  const guards = feedGuards(address.deviceId, view);
  return guardedFeedWrite(address, guards, {
    active, options: { observedFeedRows: true, supersededFeedRow },
    makeFeed: (_held, found) => {
      const records = new Map(guards.map((at, index) => [recordKey(at), found[index]]));
      return mapFeedRows(pruneView(view), (row) => admittedFeedRow(row, address.deviceId, records, rewriteRow));
    },
  });
}
