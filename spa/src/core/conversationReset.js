// Reset is a new transcript generation under the conversation's stable address.
// Write an authoritative empty window before publishing its replacement agent.
import { cachedRecords, deleteCached, mergeCachedAtomically, mergeCachedTogether, readCached } from "./localCache.js";
import { purgeConversationUiRecords } from "./localUiState.js";
import { forgetConversationRevisionBodies } from "./revisionBodies.js";
import { closeConversationAttachmentLightboxes } from "./threadAttachmentLightbox.js";
import { retireConversationFeed } from "./conversationFeedReset.js";
import { notePush } from "./pushFence.js";

export const conversationResetSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "conversation-reset-support", sub: "" });
export const rememberConversationResetSupport = (deviceId, capabilities) => {
  if (!deviceId) return Promise.resolve(false);
  const supported = capabilities?.conversations?.reset === true;
  return mergeCachedAtomically(conversationResetSupportAddress(deviceId), (held) =>
    held?.supported === supported ? null : { supported });
};

export const emptyConversationWindow = (threadId, revision = 0) => ({
  thread_id: threadId, thread_generation_revision: revision, items: [], deliveredSequence: 0, olderItemsRemain: false,
  knownTotalItems: 0, activityDigests: [],
});

/** Digest and push generations are authoritative. Ordinary pages never call
 * this: an old page may finish after reset, and must be refused at its merge. */
export async function reconcileConversationGeneration({ deviceId, entityId, agent, active = () => true, forceReset = false, previousGenerationId = "" }) {
  if (!agent?.thread_id || !active()) return false;
  const conversationId = agent.conversation_id || agent.id;
  const address = { deviceId, entityId, kind: "thread", sub: conversationId };
  let previousThreadId;
  await mergeCachedAtomically(address, (held) => {
    if (!active() || unchangedGeneration(held, agent)) return null;
    if (canAdoptLegacyWindow(held, agent, conversationId, forceReset)) return adoptLegacyWindow(held, agent);
    previousThreadId = heldThreadId(held) || previousGenerationId || `thread:${conversationId}`;
    return replacementGenerationWindow(held, agent, previousThreadId);
  });
  if (!previousThreadId || !active()) return false;
  notePush(address);
  await retireConversationAliases({ deviceId, entityId, agentId: agent.id, conversationId, threadId: previousThreadId }, agent);
  return true;
}

export async function reconcileConversationGenerations(context, entityId, agents) {
  for (const agent of agents || []) {
    if (!context.active()) return false;
    await reconcileConversationGeneration({ deviceId: context.deviceId, entityId, agent, active: context.active });
    if (!agent.thread_id) continue;
    const threadId = await cachedConversationThreadId({ deviceId: context.deviceId, entityId, kind: "thread", sub: agent.conversation_id || agent.id });
    if (threadId && threadId !== agent.thread_id) return false;
  }
  return true;
}

export async function applyConversationReset(context, answer) {
  if (!answer?.agent || !context.active()) return;
  await reconcileConversationGeneration({
    deviceId: context.deviceId, entityId: answer.entity_id, agent: answer.agent, active: context.active, forceReset: true, previousGenerationId: answer.previous_thread_id,
  });
  const rowAddress = { deviceId: context.deviceId, entityId: answer.entity_id, kind: "row", sub: "" };
  const threadAddress = { ...rowAddress, kind: "thread", sub: answer.conversation_id };
  notePush(rowAddress);
  await mergeCachedTogether([threadAddress, rowAddress], ([thread, row]) =>
    context.active() && thread?.thread_id === answer.agent.thread_id && row ? [null, {
      ...resetRowSummary(row, answer), agents: row.agents.map((agent) => agent.id === answer.agent_id ? answer.agent : agent),
    }] : null);
}

export async function cachedConversationThreadId(address) {
  return (await readCached(address))?.value?.thread_id || "";
}

const obsoleteGeneration = (held, agent) => held?.retired_thread_ids?.includes(agent.thread_id)
  || Number(held?.thread_generation_revision || 0) > Number(agent.thread_generation_revision || 0);

async function retireConversationAliases(ownership, agent) {
  const records = await cachedRecords({ deviceId: ownership.deviceId });
  const aliases = conversationAliases(records, ownership);
  for (const entityId of new Set(aliases.map((alias) => alias.entityId))) {
    const owner = { ...ownership, entityId };
    await publishAliasGeneration(owner, agent);
    await retireConversationFeed(owner, agent, resetAliasesInRow);
  }
  for (const alias of aliases) {
    forgetConversationRevisionBodies(alias);
    closeConversationAttachmentLightboxes(alias);
    await purgeConversationUiRecords(alias);
  }
  const activities = records.filter((record) => oldActivityRecord(record, ownership.threadId, aliases));
  await deleteCached([...aliases.map(({ deviceId, entityId, agentId }) => ({ deviceId, entityId, kind: "surfaces", sub: agentId })), ...activities.map(({ address }) => address)]);
}

async function publishAliasGeneration(ownership, agent) {
  const { deviceId, entityId, conversationId, threadId } = ownership;
  const threadAddress = { deviceId, entityId, kind: "thread", sub: conversationId };
  const rowAddress = { deviceId, entityId, kind: "row", sub: "" };
  notePush(threadAddress); notePush(rowAddress);
  const answer = { agent, entity_id: entityId, conversation_id: conversationId, previous_thread_id: threadId,
    thread: { thread_id: agent.thread_id, thread_generation_revision: agent.thread_generation_revision || 0, items: [], sessions: [] } };
  await mergeCachedTogether([threadAddress, rowAddress], ([thread, row]) => {
    if (obsoleteGeneration(thread, agent)) return null;
    const replacement = heldThreadId(thread) === agent.thread_id ? null : replacementGenerationWindow(thread, agent, threadId);
    return [replacement, row ? resetAliasesInRow(row, answer) : null];
  });
}

export function resetAliasesInRow(row, answer) {
  const cleared = resetRowSummary(row, answer);
  if (row.agents) cleared.agents = row.agents.map((agent) => resetAliasAgent(agent, answer));
  if (row.run?.agents) cleared.run.agents = row.run.agents.map((agent) => resetAliasAgent(agent, answer));
  return cleared;
}
function resetAliasAgent(agent, answer) {
  if ((agent.conversation_id || agent.id) !== answer.conversation_id) return agent;
  if (agent.id === answer.agent.id) return answer.agent;
  return { ...agent, thread_id: answer.agent.thread_id, thread_generation_revision: answer.agent.thread_generation_revision || 0,
    choice_revision: Number(agent.choice_revision || 0) + Number(agent.thread_id !== answer.agent.thread_id),
    active_model: agent.model || "", active_effort: agent.effort || "", resume_session_id: null, surface_session_generation: null,
    unread_count: 0, unread_reason: null, read_through_sequence: 0, working: false, working_time: null, state: "idle",
    can_interrupt: false, start_error: null, topic: null, title: null, last_context_tokens: null, last_context_at: null,
    session_cache_read_tokens: null, surfaces: null };
}

function conversationAliases(records, ownership) {
  const aliases = new Map([[`${ownership.entityId}:${ownership.agentId}`, ownership]]);
  for (const { address, value } of records) {
    if (address.kind !== "row") continue;
    for (const agent of value?.agents || []) {
      if ((agent.conversation_id || agent.id) !== ownership.conversationId) continue;
      aliases.set(`${address.entityId}:${agent.id}`, { ...ownership, entityId: address.entityId, agentId: agent.id });
    }
  }
  return [...aliases.values()];
}
const oldActivityRecord = ({ address }, threadId, aliases) => address.kind === "activity"
  && (address.sub.includes(`:${threadId}:`) || aliases.some(({ entityId, agentId }) =>
    address.entityId === entityId && address.sub.startsWith(`${agentId}:`) && address.sub.split(":").length === 2));

const heldThreadId = (held) => held?.thread_id;
const unchangedGeneration = (held, agent) => heldThreadId(held) === agent.thread_id || obsoleteGeneration(held, agent);
const canAdoptLegacyWindow = (held, agent, conversationId, forceReset) =>
  !forceReset && !heldThreadId(held) && agent.thread_id === `thread:${conversationId}`;
const adoptLegacyWindow = (held, agent) => held ? { ...held, thread_id: agent.thread_id, thread_generation_revision: agent.thread_generation_revision || 0 } : emptyConversationWindow(agent.thread_id, agent.thread_generation_revision);
const replacementGenerationWindow = (held, agent, previousThreadId) => ({
  ...emptyConversationWindow(agent.thread_id, agent.thread_generation_revision),
  retired_thread_ids: [...(held?.retired_thread_ids || []), previousThreadId],
});

function resetRowSummary(row, answer) {
  const cleared = resetOwnerSummary(row, row.agents, answer);
  if (row.run) cleared.run = resetOwnerSummary(row.run, row.run.agents || row.agents, answer);
  return cleared;
}
const heldThreadMatches = (thread, answer) => thread?.id === answer.previous_thread_id || thread?.thread_id === answer.previous_thread_id;
const primaryConversationMatches = (agents, answer) => agents?.[0]
  && (agents[0].conversation_id || agents[0].id) === answer.conversation_id;
function resetOwnerSummary(owner, agents, answer) {
  if (!heldThreadMatches(owner.thread, answer) && !primaryConversationMatches(agents, answer)) return { ...owner };
  return { ...owner, summary: "", error: null, last_error: null, ...(owner.thread ? { thread: answer.thread } : {}) };
}
