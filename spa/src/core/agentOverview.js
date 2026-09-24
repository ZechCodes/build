// The expanded rail reads only durable row and conversation records. A fetch or
// push first writes those records; their announcements re-read and redraw here.
import { readCachedMany, subscribeCache } from "./localCache.js";
import { threadCacheAddress } from "./conversationCache.js";
import { railEntity } from "./agentRailModel.js";
import { agentDisplayName } from "./agentName.js";
import { firstLine } from "./activityDigest.js";
import { EVENT_META } from "./threadEvents.js";
import { esc } from "./text.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { ICON_CHEVRON_RIGHT } from "./icons.js";

const newest = (items, predicate) => [...items].reverse().find(predicate);
const isMessage = (item) => item?.type === "message";
const sequence = (item) => Number(item?.data?.sequence) || 0;
const messageText = (item) => firstLine(item?.data?.body);
const agentMessageTime = (thread) => {
  const last = newest(thread?.items || [], (item) => isMessage(item) && item.data?.role === "agent");
  const time = Date.parse(last?.data?.created_at || "");
  return last ? (Number.isFinite(time) ? time : 1) : 0;
};

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
  return entries.map(({ agent, state = agent, source }, index) => ({
    id: agent.id,
    source,
    workspaceId: entries[index].workspaceId || "",
    section: entries[index].section || "other",
    sectionName: entries[index].sectionName || "Agents",
    name: agentDisplayName(agent),
    snippet: overviewSnippet(state, threads[index]),
    lastAgentMessageAt: agentMessageTime(threads[index]),
    working: !!state.working,
    unread: !!state.unread_count,
    watching: agent.watched,
  }));
}

/** How many of a workspace's agents the project overview shows before it
 *  offers the rest on that workspace's own overview. */
export const WORKSPACE_PREVIEW_AGENTS = 3;

const rowHtml = (row) => {
  const state = [row.working ? "Working" : row.unread ? "Unread" : "",
    row.watching === undefined ? "" : row.watching ? "Watching" : "Not watching"].filter(Boolean).join(" · ");
  return `<button type="button" class="rail-overview-row" data-overview-agent="${esc(row.id)}" data-overview-source="${esc(row.source)}" data-overview-workspace="${esc(row.workspaceId)}">
    <span class="rail-overview-name">${esc(row.name)}</span>
    <span class="rail-overview-snippet">${esc(row.snippet)}</span>
    ${state ? `<span class="rail-overview-state">${state}</span>` : ""}
  </button>`;
};

/** A workspace's heading. On the project overview it is the way into that
 *  workspace's own overview; everywhere else it only names the section. */
const sectionTitleHtml = (section, opensWorkspace) => opensWorkspace
  ? `<h2><button type="button" class="rail-overview-open" data-overview-scope="${esc(section.workspaceId)}"><span>${esc(section.name)}</span>${ICON_CHEVRON_RIGHT}</button></h2>`
  : `<h2>${esc(section.name)}</h2>`;

const byLatestAgentMessage = (one, other) => other.lastAgentMessageAt - one.lastAgentMessageAt;
const latestAgentMessage = (section) => Math.max(0, ...section.rows.map((row) => row.lastAgentMessageAt));

/** Whether a workspace is in the scope: one workspace's scope drops every
 *  other workspace, and keeps the project's agents beside it. */
const inScope = (scope, workspaceId) => scope?.kind !== "workspace" || workspaceId === scope.workspaceId;
const rowsInScope = (rows, scope) => rows.filter((row) => row.section !== "workspace" || inScope(scope, row.workspaceId));

/** Sections in reading order: the project's agents first, then the one heard
 *  from most recently. Every workspace named gets its section, agents or none,
 *  so an empty one still has its way in and its +; the quiet ones go last. */
function overviewSections(rows, showProjectAgents, workspaces) {
  const sections = new Map();
  if (showProjectAgents) sections.set("project", { section: "project", name: "Project agents", workspaceId: "", rows: [] });
  for (const { workspaceId, name } of workspaces) {
    sections.set(`workspace:${workspaceId}`, { section: "workspace", name, workspaceId, rows: [] });
  }
  for (const row of rows) {
    const key = row.section === "workspace" ? `workspace:${row.workspaceId}` : row.section;
    if (!sections.has(key)) sections.set(key, { section: row.section, name: row.sectionName, workspaceId: row.workspaceId, rows: [] });
    sections.get(key).rows.push(row);
  }
  return [...sections.values()].sort((one, other) => {
    if (one.section === "project") return -1;
    if (other.section === "project") return 1;
    return latestAgentMessage(other) - latestAgentMessage(one);
  });
}

const addButtonHtml = (section) => `<button type="button" class="iconbtn rail-overview-add" data-overview-add="${esc(section.workspaceId)}" aria-label="Add an agent to ${esc(section.name)}" title="Add an agent to ${esc(section.name)}">+</button>`;

const seeAllHtml = (section, count) => `<button type="button" class="rail-overview-see-all" data-overview-scope="${esc(section.workspaceId)}" aria-label="See all ${count} agents in ${esc(section.name)}">See all</button>`;

/** One section. On the project's scope a workspace shows its first few agents
 *  by the overview's own order, and its heading and See all open the rest. */
function sectionHtml(section, projectScope) {
  const workspace = section.section === "workspace";
  const capped = projectScope && workspace;
  const sorted = section.rows.sort(byLatestAgentMessage);
  const listed = capped ? sorted.slice(0, WORKSPACE_PREVIEW_AGENTS) : sorted;
  const more = listed.length < sorted.length ? seeAllHtml(section, sorted.length) : "";
  return `<section class="rail-overview-section" aria-label="${esc(section.name)}">
    <div class="rail-overview-section-head">${sectionTitleHtml(section, capped)}${workspace ? addButtonHtml(section) : ""}</div>
    ${listed.map(rowHtml).join("")}${more}</section>`;
}

/** `scope` is the rail's overview scope: `{ kind: "project" }` shows every
 *  workspace, each capped and opening its own overview; `{ kind: "workspace",
 *  workspaceId }` shows that one workspace beside the project's agents. Any
 *  other scope draws the rows as they are. */
export function overviewHtml(rows, { showProjectAgents = false, scope = null, workspaces = [] } = {}) {
  const shown = rowsInScope(rows, scope);
  const named = workspaces.filter((workspace) => inScope(scope, workspace.workspaceId));
  if (!shown.length && !showProjectAgents && !named.length) return '<p class="rail-overview-empty">No agents here yet.</p>';
  const projectScope = scope?.kind === "project";
  return overviewSections(shown, showProjectAgents, named).map((section) => sectionHtml(section, projectScope)).join("");
}

const cachedConversationId = (agent, execution) => execution
  ? execution.conversation_id || execution.agent_id || agent.id
  : agent.conversation_id || agent.id;

function rosterEntry(agent, execution, source) {
  return {
    agent,
    state: execution?.agent ? { ...agent, ...execution.agent } : agent,
    source: source.slot,
    workspaceId: source.workspaceId,
    section: source.section,
    sectionName: source.sectionName,
    entityId: execution?.entity_id || source.entityId,
    agentId: execution?.agent_id || agent.id,
    conversationId: cachedConversationId(agent, execution),
  };
}

function projectWorkspaceSources(workspaces, existing, projectId, scope) {
  const sources = [...existing];
  for (const workspace of workspaces) {
    if (workspace.project_id !== projectId) continue;
    const workspaceId = workspace.workspace_id || workspace.id;
    const entityId = workspace.entity_id || workspace.run_id;
    if (!workspaceId || !entityId || sources.some((source) => source.workspaceId === workspaceId)) continue;
    sources.push({ slot: "workspace", kind: "workspace", workspaceId, section: "workspace",
      sectionName: workspaceDisplayName(workspace), entityId,
      address: scope.address({ entityId, kind: "row", sub: "" }) });
  }
  return sources;
}

/** Every workspace the overview can head: those with a row to read, then the
 *  project's workspaces that have no entity yet, and so nothing to read. */
function workspaceSections(rosterSources, workspaces, projectId) {
  const named = rosterSources.filter((source) => source.section === "workspace" && source.workspaceId)
    .map((source) => ({ workspaceId: source.workspaceId, name: source.sectionName }));
  for (const workspace of workspaces) {
    const workspaceId = workspace.workspace_id || workspace.id;
    if (workspace.project_id !== projectId || !workspaceId || named.some((one) => one.workspaceId === workspaceId)) continue;
    named.push({ workspaceId, name: workspaceDisplayName(workspace) });
  }
  return named;
}

function rosterEntries(sources, records, feed) {
  return sources.flatMap((source, index) => {
    const fallback = (feed?.runs || []).find((run) => (run.entity_id || run.run_id) === source.entityId);
    const owner = railEntity(records[index]?.value || fallback || {}, source.kind);
    return owner.agents.map((agent) => rosterEntry(agent, owner.executionContext, source));
  });
}

function threadEntries(entries, scope) {
  return entries.map(({ agent, state, entityId, agentId, source, conversationId, workspaceId, section, sectionName }) => ({
    agent, state, source, workspaceId, section, sectionName,
    address: scope?.address(threadCacheAddress({ deviceId: scope.deviceId, entityId, agentId, conversationId })),
  })).filter((entry) => entry.address);
}

/** Sources are cache row addresses for this work item and, where present, its
 * project agent. A subscription is installed before each read so a write
 * racing the read gets another pass. Generations discard stale passes. */
export function createAgentOverview({ sources, scope, onRows, projectId = null, includeProjectWorkspaces = false }) {
  let active = false;
  let generation = 0;
  const watches = new Map();
  // A function when the breadth can change under a mounted overview: the
  // rail's scope moves between one workspace and the whole project.
  const includesWorkspaces = () => (typeof includeProjectWorkspaces === "function"
    ? includeProjectWorkspaces() : includeProjectWorkspaces);
  /** The machine's workspace list and feed, read only when the overview is
   *  about more than its own sources. */
  const listAddressesFor = (included) => {
    const workspaceAddress = included && scope?.address({ entityId: "", kind: "workspaces" });
    const feedAddress = workspaceAddress && scope?.address({ entityId: "", kind: "feed" });
    return { workspaceAddress, listAddresses: [workspaceAddress, feedAddress].filter(Boolean) };
  };

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
    const stale = () => !active || current !== generation;
    const { workspaceAddress, listAddresses } = listAddressesFor(includesWorkspaces());
    watch([...sources().map((source) => source.address).filter(Boolean), ...listAddresses]);
    const [workspaceRecord, feedRecord] = await readCachedMany(listAddresses);
    if (stale()) return;
    const workspaces = workspaceAddress ? (workspaceRecord?.value || []) : [];
    const rosterSources = projectWorkspaceSources(workspaces, sources().filter((source) => source.address), projectId, scope);
    watch([...rosterSources.map((source) => source.address), ...listAddresses]);
    const rosterRecords = await readCachedMany(rosterSources.map((source) => source.address));
    if (stale()) return;
    const addressed = threadEntries(rosterEntries(rosterSources, rosterRecords, feedRecord?.value), scope);
    watch([...rosterSources.map((source) => source.address), ...addressed.map((entry) => entry.address), ...listAddresses]);
    const threadRecords = await readCachedMany(addressed.map((entry) => entry.address));
    if (stale()) return;
    onRows(overviewRows(addressed, threadRecords.map((record) => record?.value)),
      { workspaces: workspaceSections(rosterSources, workspaces, projectId) });
  };

  return {
    // The rail says open on every paint the overview is showing in; only the
    // first starts the read, and the cache watches keep it current after that.
    open() {
      if (active) return;
      active = true;
      void refresh();
    },
    refresh() { void refresh(); },
    close() { active = false; generation += 1; watch([]); },
  };
}
