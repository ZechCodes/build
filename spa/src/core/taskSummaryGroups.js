// Exact #900 static stream specifications and cached continuation state. This
// module admits no reads: each group advances only on an explicit caller read.
import { columnsOf, PRIORITIES } from "./trackerModel.js";

export const compareTaskSummaryNumber = (left, right) => right.number - left.number;
const priorityRank = (task) => Math.max(0, PRIORITIES.findIndex(({ id }) => id === task.priority));
export const compareTaskSummaryPriority = (left, right) =>
  priorityRank(right) - priorityRank(left) || compareTaskSummaryNumber(left, right);
const updatedMs = (task) => task.updated_at ? Date.parse(task.updated_at) : 0;
export const compareTaskSummaryUpdated = (left, right) =>
  updatedMs(right) - updatedMs(left) || compareTaskSummaryNumber(left, right);

/** Cross-project anchor comparisons intentionally have no number or ID tie
 * break: the merge retains project source order, whose own rows are ordered. */
export function compareTaskSummaryAnchors(left, right) {
  if (left.order_anchor_ms === right.order_anchor_ms) return 0;
  if (left.order_anchor_ms == null) return 1;
  if (right.order_anchor_ms == null) return -1;
  return left.order_anchor_ms - right.order_anchor_ms;
}

/** Within one project the backend breaks equal anchors by task number. The
 * query's explicit null remains unknown; never reconstruct it from task facts. */
export const compareTaskSummaryAnchor = (left, right) =>
  compareTaskSummaryAnchors(left, right) || compareTaskSummaryNumber(left, right);

function activeGroups(base, assigned, activeStatuses, working) {
  if (!activeStatuses.length) return [];
  const active = { ...assigned, state: "open", statuses: activeStatuses };
  const groups = [];
  if (working) groups.push({ id: "working", params: { ...base, ...active, blocked: false, undelivered: false } });
  groups.push({ id: "assigned", params: { ...base, ...active, blocked: false, ...(working ? { undelivered: true } : {}) } });
  groups.push({ id: "blocked", params: { ...base, ...active, blocked: true } });
  return groups;
}

/** Static membership comes from the complete cached column vocabulary, never
 * from whichever task rows happen to be loaded. Working is a cached runtime
 * observation, not a server task status. Assignment takes precedence globally. */
export function agentTaskSummaryGroups({ projectId, agentId, columns, working = false, limit = 100 }) {
  const base = { project_id: projectId, summary: true, order: "priority", limit };
  const assignee = { kind: "agent", agent_id: agentId };
  const assigned = { assignee };
  const activeStatuses = [...new Set(columnsOf(columns).map(({ id }) => id))]
    .filter((id) => id !== "in_review" && id !== "done");
  return [
    ...activeGroups(base, assigned, activeStatuses, working === true),
    { id: "tracked", params: { ...base, tracker_agent_id: agentId, exclude_assignee: assignee } },
    { id: "in_review", params: { ...base, ...assigned, state: "open", status: "in_review" } },
    { id: "done", params: { ...base, ...assigned, state: "open", status: "done" } },
    { id: "closed", params: { ...base, ...assigned, state: "closed" } },
  ];
}

// Page size is not membership identity; column order is not a selection fact.
function groupKey({ id, params }) {
  const { limit: _limit, ...membership } = params;
  if (membership.statuses) membership.statuses = [...membership.statuses].sort();
  return JSON.stringify([id, membership]);
}

function target(options) {
  const specs = agentTaskSummaryGroups(options);
  const context = [options.deviceId ?? null, options.coordinatorId ?? null, options.rosterKey ?? null,
    options.working === true, specs.map(groupKey)];
  return { specs, key: JSON.stringify(context) };
}

function emptyGroup(spec, previous) {
  return { ...spec, key: groupKey(spec), items: previous?.items || [], nextCursor: null,
    basis: null, complete: false, stale: true };
}

function stateForTarget(destination, generation, previous = {}) {
  return { key: destination.key, generation,
    order: destination.specs.map(({ id }) => id),
    groups: Object.fromEntries(destination.specs.map((spec) => [spec.id, emptyGroup(spec, previous[spec.id])])) };
}

export const createTaskSummaryGroupState = (options) => stateForTarget(target(options), 0);

/** A cached working/coordinator/roster change advances the whole generation.
 * Retained rows are explicitly stale and have no reusable continuation. */
export function retargetTaskSummaryGroups(state, options) {
  const destination = target(options);
  return state.key === destination.key ? state : stateForTarget(destination, state.generation + 1, state.groups);
}

/** A token captures the exact buffered group, so a duplicate or superseded
 * reply cannot append twice. Null means this explicit continuation is absent. */
export function captureTaskSummaryGroupRead(state, id, { continuation = false } = {}) {
  const group = state.groups[id];
  if (!group) return null;
  if (continuation && (group.stale || !group.nextCursor)) return null;
  return { generation: state.generation, id, key: group.key, group,
    cursor: continuation ? group.nextCursor : null, basis: continuation ? group.basis : null };
}

const uniqueItems = (items) => [...new Map(items.map((item) => [item.id, item])).values()];

function acceptsPage(state, token, page) {
  if (!token || token.generation !== state.generation) return false;
  const group = state.groups[token.id];
  if (group !== token.group || group.key !== token.key) return false;
  if (token.cursor && (group.nextCursor !== token.cursor || page.basis !== token.basis)) return false;
  return true;
}

/** The caller supplies one authoritative page. Sparse empty pages remain
 * incomplete when they carry a cursor; no other group fills their missing head.
 * Continuation basis changes leave the cache standing for an explicit restart. */
export function applyTaskSummaryGroupPage(state, token, page) {
  if (!acceptsPage(state, token, page)) return state;
  const group = state.groups[token.id];
  const items = uniqueItems(token.cursor ? [...group.items, ...page.tasks] : page.tasks);
  const nextCursor = page.next_cursor ?? null;
  return { ...state, groups: { ...state.groups, [token.id]: { ...group, items, nextCursor,
    basis: page.basis, complete: nextCursor === null, stale: false } } };
}
