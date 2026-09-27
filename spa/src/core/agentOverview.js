// The expanded rail reads only durable row and conversation records. A fetch or
// push first writes those records; their announcements re-read and redraw here.
//
// What it draws (#186) is an overview of the work on the project: every
// workspace once, its agents nested under it, ordered so that what needs the
// reader comes first, then what is working, then the quiet, then the empty.
// Watching is a mark on the agent, not a section of its own.
import { readCachedMany, subscribeCache } from "./localCache.js";
import { threadCacheAddress } from "./conversationCache.js";
import { AGENT_STARTING, providerLabel, railEntity } from "./agentRailModel.js";
import { agentDisplayName } from "./agentName.js";
import { firstLine } from "./activityDigest.js";
import { EVENT_META } from "./threadEvents.js";
import { unreadReasonText } from "./inbox.js";
import { esc } from "./text.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { issuesAddress } from "./trackerCache.js";
import { assignedTo, isFinished } from "./trackerAgentIssues.js";
import { plainPreview } from "./previewText.js";
import { ICON_CHEVRON_RIGHT, ICON_EYE_OFF } from "./icons.js";

const newest = (items, predicate) => [...items].reverse().find(predicate);
const isMessage = (item) => item?.type === "message";
const sequence = (item) => Number(item?.data?.sequence) || 0;
const messageText = (item) => plainPreview(item?.data?.body);
const agentMessageTime = (thread) => {
  const last = newest(thread?.items || [], (item) => isMessage(item) && item.data?.role === "agent");
  const time = Date.parse(last?.data?.created_at || "");
  return last ? (Number.isFinite(time) ? time : 1) : 0;
};

function workingSnippet(thread, items) {
  const activity = newest(items, (item) => EVENT_META[item?.data?.event]?.activity);
  if (activity) return plainPreview(firstLine(activity.data.summary)) || EVENT_META[activity.data.event].label;
  const digest = newest(thread?.activityDigests || [], (entry) => entry?.last_tool_call?.summary);
  return digest ? plainPreview(firstLine(digest.last_tool_call.summary)) : "Working";
}

export function overviewSnippet(agent, thread) {
  const items = thread?.items || [];
  if (agent.working) return workingSnippet(thread, items);
  const unread = agent.unread_count && newest(items, (item) => isMessage(item) && item.data.role === "agent"
    && sequence(item) > (Number(agent.read_through_sequence) || 0));
  const latest = unread || newest(items, isMessage);
  return messageText(latest) || "No messages yet";
}

// ---- what a row says about its agent ---------------------------------------

/** The attention kinds that mean something went wrong rather than something
 *  is waiting: the row wears the error tone for these. */
const FAILED_REASONS = new Set(["run_failed", "stage_failed", "recovery_failed"]);

/** The one word a waiting row wears, by what made it unread; the full sentence
 *  (core/inbox.js) is its tooltip. */
const WAITING_WORD = {
  agent_message: "Unread",
  done: "Finished",
  blocked: "Blocked",
  review_blocked: "Blocked",
  idle_unreported: "Went idle",
  interrupted: "Interrupted",
  merged: "Merged",
  abandoned: "Abandoned",
};

export const OVERVIEW_STATES = Object.freeze({
  working: "working", waiting: "waiting", error: "error", starting: "starting", idle: "idle",
});

const failedStart = (agent) => (agent.start_error
  ? { state: OVERVIEW_STATES.error, word: "Failed to start", detail: String(agent.start_error) } : null);
const failedRun = (agent) => (agent.unread_count && FAILED_REASONS.has(agent.unread_reason)
  ? { state: OVERVIEW_STATES.error, word: "Failed", detail: unreadReasonText(agent.unread_reason, "agent") } : null);
const waitingOnReader = (agent) => (agent.unread_count
  ? { state: OVERVIEW_STATES.waiting, word: WAITING_WORD[agent.unread_reason] || "Unread",
    detail: unreadReasonText(agent.unread_reason, "agent") || `${agent.unread_count} unread` } : null);
const workingNow = (agent) => (agent.working
  ? { state: OVERVIEW_STATES.working, word: "Working", detail: "Working now" } : null);
const startingUp = (agent) => (agent.state === AGENT_STARTING
  ? { state: OVERVIEW_STATES.starting, word: "Starting", detail: "Starting a session" } : null);
const IDLE = Object.freeze({ state: OVERVIEW_STATES.idle, word: "Idle", detail: "" });

/** The readings in the order they win: a failed start is the thing to know
 *  whatever else the agent has said, then a failed run, then what is waiting
 *  for the reader, then work in flight, then a session on its way. */
const STATE_READINGS = [failedStart, failedRun, waitingOnReader, workingNow, startingUp];

/** An agent's state as the overview says it: `state` is one of
 *  OVERVIEW_STATES, `word` the short label the row wears, `detail` the longer
 *  sentence behind it. */
export function overviewState(agent) {
  const standing = agent || {};
  for (const read of STATE_READINGS) {
    const reading = read(standing);
    if (reading) return reading;
  }
  return IDLE;
}

const DATE_TOKEN = /^\d{8}$/;
const NUMERIC = /^\d+$/;
const UPPER = new Set(["gpt", "o1", "o3", "o4"]);

/** The tokens of a model id that say nothing on a row: the vendor, and a date. */
const silentToken = (token) => token === "claude" || DATE_TOKEN.test(token);
const capitalized = (token) => (UPPER.has(token) ? token.toUpperCase() : token.charAt(0).toUpperCase() + token.slice(1));
/** Whether a number continues the version the last word ended on ("5", "1" → "5.1"). */
const continuesVersion = (words, token) => NUMERIC.test(token) && words.length > 0
  && NUMERIC.test(words[words.length - 1].split(".").pop());
const addModelToken = (words, token) => {
  if (silentToken(token)) return words;
  if (continuesVersion(words, token)) return [...words.slice(0, -1), `${words[words.length - 1]}.${token}`];
  return [...words, capitalized(token)];
};

/** A model id as a row can wear it: "claude-opus-5" → "Opus 5",
 *  "gpt-6-astra" → "GPT 6 Astra", "claude-haiku-4-5-20251001" → "Haiku 4.5".
 *  The harness's name when the agent runs on its default. */
export function modelWord(agent) {
  const id = String(agent?.active_model || agent?.model || "").trim();
  if (!id) return providerLabel(agent?.provider);
  return id.split(/[-/]/).filter(Boolean).reduce(addModelToken, []).join(" ") || id;
}

export function overviewRows(entries, threads) {
  return entries.map(({ agent, state = agent, source }, index) => {
    const standing = overviewState(state);
    return {
      id: agent.id,
      source,
      workspaceId: entries[index].workspaceId || "",
      section: entries[index].section || "other",
      sectionName: entries[index].sectionName || "Agents",
      name: agentDisplayName(agent),
      model: modelWord(state),
      effort: String(state.effort || ""),
      snippet: overviewSnippet(state, threads[index]),
      lastAgentMessageAt: agentMessageTime(threads[index]),
      working: !!state.working,
      unread: !!state.unread_count,
      unreadCount: Number(state.unread_count) || 0,
      state: standing.state,
      stateWord: standing.word,
      stateDetail: standing.detail,
      watching: agent.watched,
    };
  });
}

/** How many of a workspace's agents the project overview shows before it
 *  offers the rest on that workspace's own overview. */
export const WORKSPACE_PREVIEW_AGENTS = 3;

/** Where a row sorts within its workspace: what needs the reader, then what is
 *  working, then the rest — and among equals, the one heard from last. */
const ROW_RANK = { error: 3, waiting: 3, working: 2, starting: 2, idle: 1 };
const rowRank = (row) => ROW_RANK[row.state] || 1;
const byAttention = (one, other) => rowRank(other) - rowRank(one) || other.lastAgentMessageAt - one.lastAgentMessageAt;

/** Whether the reader watches the agent on this row. A row from a bridge that
 *  says nothing about watching counts as watched. */
const isUnwatched = (row) => row.watching === false;

const UNWATCHED_MARK = "Not watching";

const watchHtml = (row) => (isUnwatched(row)
  ? `<span class="rail-overview-watch" role="img" aria-label="${UNWATCHED_MARK}" title="${UNWATCHED_MARK}">${ICON_EYE_OFF}</span>`
  : "");

const rowHtml = (row) => {
  const classes = ["rail-overview-row", isUnwatched(row) ? "rail-overview-row-unwatched" : ""].filter(Boolean).join(" ");
  const modelTitle = row.effort ? `${row.model} · ${row.effort}` : row.model;
  return `<button type="button" class="${classes}" data-state="${esc(row.state)}" data-overview-agent="${esc(row.id)}" data-overview-source="${esc(row.source)}" data-overview-workspace="${esc(row.workspaceId)}">
    <span class="rail-overview-dot" aria-hidden="true"></span>
    <span class="rail-overview-who"><span class="rail-overview-name">${esc(row.name)}</span><span class="rail-overview-model" title="${esc(modelTitle)}">${esc(row.model)}</span></span>
    <span class="rail-overview-state" title="${esc(row.stateDetail)}">${esc(row.stateWord)}</span>${watchHtml(row)}
    <span class="rail-overview-snippet">${esc(row.snippet)}</span>
  </button>`;
};

/** A workspace's heading. On the project overview it is the way into that
 *  workspace's own overview; everywhere else it only names the section. */
const sectionTitleHtml = (section, opensWorkspace) => opensWorkspace
  ? `<h2><button type="button" class="rail-overview-open" data-overview-scope="${esc(section.workspaceId)}" title="Open ${esc(section.name)}"><span>${esc(section.name)}</span>${ICON_CHEVRON_RIGHT}</button></h2>`
  : `<h2><span>${esc(section.name)}</span></h2>`;

const latestAgentMessage = (section) => Math.max(0, ...section.rows.map((row) => row.lastAgentMessageAt));

/** Where a workspace sorts on the overview: one with something that needs the
 *  reader first, then one with work in flight, then the quiet ones with
 *  agents, then the empty; among equals, the one heard from last. */
export function sectionRank(section) {
  if (section.rows.some((row) => row.state === OVERVIEW_STATES.error || row.state === OVERVIEW_STATES.waiting)) return 3;
  if (section.rows.some((row) => row.state === OVERVIEW_STATES.working || row.state === OVERVIEW_STATES.starting)) return 2;
  return section.rows.length ? 1 : 0;
}

/** Whether a workspace is in the scope: one workspace's scope drops every
 *  other workspace, and keeps the project's agents beside it. */
const inScope = (scope, workspaceId) => scope?.kind !== "workspace" || workspaceId === scope.workspaceId;
const rowsInScope = (rows, scope) => rows.filter((row) => row.section !== "workspace" || inScope(scope, row.workspaceId));

/** The open issues that stand on a workspace: linked to it, or held by one of
 *  its agents; the one touched last first. Finished work is not an overview's
 *  business. */
export function workspaceIssues(issues, workspaceId, agentIds = []) {
  return (issues || []).filter((issue) => !isFinished(issue)
    && ((issue?.links?.workspace_ids || []).includes(workspaceId) || agentIds.some((agentId) => assignedTo(issue, agentId))))
    .sort((one, other) => Date.parse(other.updated_at || "") - Date.parse(one.updated_at || "") || (other.number || 0) - (one.number || 0));
}

/** Sections in reading order: the project's agents first, then by rank. Every
 *  workspace named gets its section, agents or none, so an empty one still has
 *  its way in and its +; the quiet ones go last. */
function overviewSections(rows, showProjectAgents, workspaces, issues) {
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
  for (const section of sections.values()) {
    section.issues = section.section === "workspace"
      ? workspaceIssues(issues, section.workspaceId, section.rows.map((row) => row.id)) : [];
  }
  return [...sections.values()].sort((one, other) => {
    if (one.section === "project") return -1;
    if (other.section === "project") return 1;
    return sectionRank(other) - sectionRank(one) || latestAgentMessage(other) - latestAgentMessage(one);
  });
}

const addButtonHtml = (section) => `<button type="button" class="iconbtn rail-overview-add" data-overview-add="${esc(section.workspaceId)}" aria-label="Add an agent to ${esc(section.name)}" title="Add an agent to ${esc(section.name)}">+</button>`;

/** The +'s slot, held empty on the project's heading (#192): the project has
 *  no + of its own, and its dot stands in the dots' column all the same. */
const addSlotHtml = (section) => (section.section === "workspace" ? addButtonHtml(section) : '<span class="rail-overview-add-slot"></span>');

const seeAllHtml = (section, count) => `<button type="button" class="rail-overview-see-all" data-overview-scope="${esc(section.workspaceId)}" aria-label="See all ${count} agents in ${esc(section.name)}">See all ${count}</button>`;

/** The issue a workspace is for, as its heading wears it: the number and the
 *  title, and how many more stand on it. */
const issueHtml = (issues) => {
  if (!issues.length) return "";
  const [first, ...rest] = issues;
  const title = issues.map((issue) => `#${issue.number} ${issue.title || ""}`.trim()).join("\n");
  return `<span class="rail-overview-issue" title="${esc(title)}"><span class="rail-overview-issue-number">#${esc(String(first.number))}</span><span class="rail-overview-issue-title">${esc(first.title || "")}</span>${rest.length ? `<span class="rail-overview-issue-more">+${rest.length}</span>` : ""}</span>`;
};

/** The working dot, always drawn (#192): the pulse in the accent while any
 *  agent works, and the same dot muted and still when nothing does, so the
 *  slot holds its column and a reader can tell "nothing" from "not shown". */
const workingDotHtml = (working) => {
  const [className, word] = working ? ["rail-overview-live", `${working} working`] : ["rail-overview-idle", "Nothing working"];
  return `<span class="${className}" title="${word}" role="img" aria-label="${word}"></span>`;
};

/** What a workspace's heading says at a glance, in three fixed slots: the
 *  unread waiting for the reader (a mark before it for an agent that failed),
 *  the working dot, and — outside this span — the +. */
const summaryHtml = (section) => {
  // The pulse follows the agent's working flag, not its state word: an agent
  // working with an unread message reads "Unread", and is still at work.
  const working = section.rows.filter((row) => row.working).length;
  const unread = section.rows.reduce((total, row) => total + row.unreadCount, 0);
  const failed = section.rows.filter((row) => row.state === OVERVIEW_STATES.error).length;
  const parts = [];
  if (failed) parts.push(`<span class="rail-overview-need is-error" title="${failed} failed">!</span>`);
  if (unread) parts.push(`<span class="rail-overview-need" title="${unread} unread">${unread}</span>`);
  if (section.section === "workspace" && !section.rows.length) parts.push('<span class="rail-overview-none">No agents</span>');
  parts.push(workingDotHtml(working));
  return parts.join("");
};

/** One section. On the project's scope a workspace shows its first few agents
 *  by the overview's own order, and its heading and See all open the rest. */
function sectionHtml(section, projectScope) {
  const workspace = section.section === "workspace";
  const capped = projectScope && workspace;
  const sorted = section.rows.sort(byAttention);
  const listed = capped ? sorted.slice(0, WORKSPACE_PREVIEW_AGENTS) : sorted;
  const more = listed.length < sorted.length ? seeAllHtml(section, sorted.length) : "";
  const classes = ["rail-overview-section", section.rows.length ? "" : "rail-overview-section-empty"].filter(Boolean).join(" ");
  return `<section class="${classes}" aria-label="${esc(section.name)}" data-rank="${sectionRank(section)}">
    <div class="rail-overview-section-head">${sectionTitleHtml(section, capped)}${issueHtml(section.issues || [])}<span class="rail-overview-sum">${summaryHtml(section)}</span>${addSlotHtml(section)}</div>
    ${listed.map(rowHtml).join("")}${more}</section>`;
}

/** `scope` is the rail's overview scope: `{ kind: "project" }` shows every
 *  workspace, each capped and opening its own overview; `{ kind: "workspace",
 *  workspaceId }` shows that one workspace beside the project's agents. Any
 *  other scope draws the rows as they are. `issues` is the project's cached
 *  issue list, which names the issue each workspace is for. */
export function overviewHtml(rows, { showProjectAgents = false, scope = null, workspaces = [], issues = [] } = {}) {
  const shown = rowsInScope(rows, scope);
  const named = workspaces.filter((workspace) => inScope(scope, workspace.workspaceId));
  if (!shown.length && !showProjectAgents && !named.length) return '<p class="rail-overview-empty">No agents here yet.</p>';
  const projectScope = scope?.kind === "project";
  return overviewSections(shown, showProjectAgents, named, issues)
    .map((section) => sectionHtml(section, projectScope)).join("");
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

/** The issues a cached tracker list carries; none for a project never read. */
const cachedIssues = (record) => (record && record.value && record.value.issues) || [];

/** The records at `addresses`, in the same places; undefined where the address
 *  is null (a list this pass does not read) or the cache has nothing. */
async function readListed(addresses) {
  const records = await readCachedMany(addresses.filter(Boolean));
  let next = 0;
  return addresses.map((address) => (address ? records[next++] : undefined));
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
   *  about more than its own sources; and the project's issue list, as the
   *  tracker cached it, in every scope with a workspace heading to name an
   *  issue on. Read, never fetched — a project whose issues this client has
   *  not read heads its workspaces by name alone. `listed` keeps the three
   *  places whether or not each is read this pass. */
  const listAddressesFor = (included) => {
    const workspaceAddress = included && scope?.address({ entityId: "", kind: "workspaces" });
    const feedAddress = workspaceAddress && scope?.address({ entityId: "", kind: "feed" });
    const issuesRecordAddress = projectId && scope?.address(issuesAddress(scope.deviceId, projectId));
    const listed = [workspaceAddress, feedAddress, issuesRecordAddress].map((address) => address || null);
    return { workspaceAddress, listed, listAddresses: listed.filter(Boolean) };
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
    const { workspaceAddress, listed, listAddresses } = listAddressesFor(includesWorkspaces());
    watch([...sources().map((source) => source.address).filter(Boolean), ...listAddresses]);
    const [workspaceRecord, feedRecord, issuesRecord] = await readListed(listed);
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
      { workspaces: workspaceSections(rosterSources, workspaces, projectId), issues: cachedIssues(issuesRecord) });
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
