// The expanded rail reads only durable row and conversation records. A fetch or
// push first writes those records; their announcements re-read and redraw here.
import { readCachedMany, subscribeCache } from "./localCache.js";
import { threadCacheAddress } from "./conversationCache.js";
import { railEntity } from "./agentRailModel.js";
import { agentDisplayName } from "./agentName.js";
import { firstLine } from "./activityDigest.js";
import { EVENT_META } from "./threadEvents.js";
import { esc } from "./text.js";

const newest = (items, predicate) => [...items].reverse().find(predicate);
const isMessage = (item) => item?.type === "message";
const sequence = (item) => Number(item?.data?.sequence) || 0;
const messageText = (item) => firstLine(item?.data?.body);

function workingSnippet(thread, items) {
  const activity = newest(items, (item) => EVENT_META[item?.data?.event]?.activity);
  if (activity) return firstLine(activity.data.summary) || EVENT_META[activity.data.event].label;
  const digest = newest(thread?.activityDigests || [], (entry) => entry?.last_tool_call?.summary);
  return digest ? firstLine(digest.last_tool_call.summary) : "Working";
}

export function overviewSnippet(agent, thread) {
  const items = thread?.items || [];
  if (agent.working) return workingSnippet(thread, items);
  const unread = agent.unread_count && newest(items, (item) => isMessage(item) && item.data.role === "agent"
    && sequence(item) > (Number(agent.read_through_sequence) || 0));
  const latest = unread || newest(items, isMessage);
  return messageText(latest) || "No messages yet";
}

export function overviewRows(entries, threads) {
  return entries.map(({ agent, source }, index) => ({
    id: agent.id,
    source,
    name: agentDisplayName(agent),
    snippet: overviewSnippet(agent, threads[index]),
    working: !!agent.working,
    unread: !!agent.unread_count,
  }));
}

export function overviewHtml(rows) {
  if (!rows.length) return '<p class="rail-overview-empty">No agents here yet.</p>';
  return rows.map((row) => `<button type="button" class="rail-overview-row" data-overview-agent="${esc(row.id)}" data-overview-source="${esc(row.source)}">
    <span class="rail-overview-name">${esc(row.name)}</span>
    <span class="rail-overview-snippet">${esc(row.snippet)}</span>
    ${row.working ? '<span class="rail-overview-state">Working</span>' : row.unread ? '<span class="rail-overview-state">Unread</span>' : ""}
  </button>`).join("");
}

/** Sources are cache row addresses for this work item and, where present, its
 * project agent. A subscription is installed before each read so a write
 * racing the read gets another pass. Generations discard stale passes. */
export function createAgentOverview({ sources, scope, onRows }) {
  let active = false;
  let generation = 0;
  const watches = new Map();

  const watch = (addresses) => {
    const wanted = new Set(addresses.map((address) => JSON.stringify(address)));
    for (const [key, stop] of watches) if (!wanted.has(key)) { stop(); watches.delete(key); }
    for (const address of addresses) {
      const key = JSON.stringify(address);
      if (!watches.has(key)) watches.set(key, subscribeCache(address, () => void refresh()));
    }
  };

  const refresh = async () => {
    if (!active) return;
    const current = ++generation;
    const rosterSources = sources().filter((source) => source.address);
    watch(rosterSources.map((source) => source.address));
    const rosterRecords = await readCachedMany(rosterSources.map((source) => source.address));
    if (!active || current !== generation) return;
    const entries = rosterSources.flatMap((source, index) => {
      const owner = railEntity(rosterRecords[index]?.value || {}, source.kind);
      return owner.agents.map((agent) => ({ agent, source: source.slot,
        entityId: owner.executionContext?.entity_id || source.entityId,
        conversationId: owner.executionContext?.conversation_id || agent.conversation_id || agent.id }));
    });
    const addressed = entries.map(({ agent, entityId, source, conversationId }) => ({ agent, entityId, source,
      address: scope?.address(threadCacheAddress({
      deviceId: scope.deviceId,
      entityId,
      agentId: agent.id,
      conversationId,
    })) })).filter((entry) => entry.address);
    watch([...rosterSources.map((source) => source.address), ...addressed.map((entry) => entry.address)]);
    const threadRecords = await readCachedMany(addressed.map((entry) => entry.address));
    if (!active || current !== generation) return;
    onRows(overviewRows(addressed, threadRecords.map((record) => record?.value)));
  };

  return {
    open() { active = true; void refresh(); },
    refresh() { void refresh(); },
    close() { active = false; generation += 1; watch([]); },
  };
}
