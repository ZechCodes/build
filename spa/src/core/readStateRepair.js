// Read marks change badges, never ownership. Repair their cached projections
// separately from ordered sync so they cannot repeat selection or hold its ring.
import { entityIdOf } from "./entityId.js";
import { cachedWriteOf, isCachedWrite, captureCachedRecord, mergeCachedReadState, readCachedMany, updateCachedFeed } from "./localCache.js";
import { pushFence, pushedSince } from "./pushFence.js";
import { syncProjectAgents } from "./projectAgentsCache.js";

const ROW_FIELDS = ["needs_attention", "unread", "unread_count", "unread_reason", "attention"];
const AGENT_FIELDS = ["unread_count", "unread_reason", "read_through_sequence"];
const BOARD_FIELDS = ["items", "runs", "plans", "external_worktrees", "externalWorktrees"];
const rosterIdentity = (row) => JSON.stringify((row.agents || []).map((agent) =>
  [agent.id, agent.conversation_id, agent.thread_id, agent.thread_generation_revision]));

function mergeFields(held, incoming, fields) {
  const next = { ...held };
  for (const field of fields) if (Object.hasOwn(incoming, field)) next[field] = incoming[field];
  return next;
}

/** List digests may repair badges but cannot replace Detail or a generation. */
const cursorComparable = (held, incoming) => held.agents?.length > 0 && held.agents.every((agent, index) =>
  Number.isFinite(agent.read_through_sequence) && Number.isFinite(incoming.agents[index].read_through_sequence));
const cursorRegresses = (held, incoming) => (held.agents || []).some((agent, index) =>
  Number.isFinite(agent.read_through_sequence) && Number.isFinite(incoming.agents[index].read_through_sequence)
    && incoming.agents[index].read_through_sequence < agent.read_through_sequence);

export function mergeReadRoster(held, incoming, requireComparable = false) {
  if (!held || !incoming || rosterIdentity(held) !== rosterIdentity(incoming)) return null;
  if (requireComparable && !cursorComparable(held, incoming) || cursorRegresses(held, incoming)) return null;
  const next = mergeFields(held, incoming, ROW_FIELDS);
  if (Array.isArray(held.agents)) {
    next.agents = held.agents.map((agent, index) => mergeFields(agent, incoming.agents[index], AGENT_FIELDS));
  }
  return JSON.stringify(next) === JSON.stringify(held) ? null : next;
}

const addressOf = (deviceId, entityId, kind) => ({ deviceId, entityId, kind, sub: "" });
export const readStateRows = (board) => BOARD_FIELDS.flatMap((field) => board?.[field] || []);

/** Capture before the board request; a later feed write always wins. */
export async function captureReadFeed(deviceId) {
  const address = addressOf(deviceId, "", "feed");
  return { address, record: await captureCachedRecord(address), fence: pushFence() };
}

function mergeFeed(held, rows, entityId, requireComparable) {
  if (!held) return null;
  const incoming = new Map(rows.map((row) => [entityIdOf(row), row]));
  let changed = false;
  const next = { ...held };
  for (const field of BOARD_FIELDS) {
    if (!Array.isArray(held[field])) continue;
    next[field] = held[field].map((row) => {
      const id = entityIdOf(row);
      const replacement = (!entityId || id === entityId) && mergeReadRoster(row, incoming.get(id), requireComparable);
      changed ||= Boolean(replacement);
      return replacement || row;
    });
  }
  return changed ? next : null;
}

/** Reuse the immediate read mark's board reply without another wire read. */
export function repairReadFeed(context, captured, board, entityId = null) {
  if (!captured?.record?.value) return Promise.resolve(false);
  const { address, record, fence } = captured;
  return updateCachedFeed(address, (held, current) => context.active() && !pushedSince(address, fence)
    ? mergeFeed(held, readStateRows(board), entityId, !isCachedWrite(current, cachedWriteOf(record))) : null,
  { readStateFrom: record });
}

/** Modern project-owner badges use the complete owner roster, not board rows.
 * The existing background lane coalesces only this compatibility read. */
const capabilitiesOf = (context) => typeof context.capabilities === "function" ? context.capabilities() : context.capabilities;
const knownOwner = (roster, projects, entityId) => roster?.value?.entity_id === entityId
  || (projects?.value || []).some((project) => (project.entity_id || project.run_id) === entityId);

export async function repairReadOwner(context, entityId) {
  if (!capabilitiesOf(context)?.agents?.projectWorkers || !context.active()) return;
  const [roster, projects] = await readCachedMany([
    addressOf(context.deviceId, entityId, "project-agents"), addressOf(context.deviceId, "", "projects"),
  ]);
  if (knownOwner(roster, projects, entityId)) return syncProjectAgents({ ...context, entityId, background: true });
}

const repairs = new Map();
const sameOwner = (left, right) => left.session === right.session && left.requestScope === right.requestScope;

/** One board repair plus one trailing request per device/session/path. */
export function refreshReadState(context) {
  if (!context?.active()) return Promise.resolve(false);
  const held = repairs.get(context.deviceId);
  if (held && sameOwner(held.context, context) && held.context.active()) {
    held.nextContext = context;
    held.again = true;
    return held.done.then(() => held.next ?? false);
  }
  return startRepair(context);
}

function startRepair(context) {
  const state = { context, again: false };
  repairs.set(context.deviceId, state);
  state.done = readStateOnce(context).catch(() => false).finally(() => {
    if (repairs.get(context.deviceId) !== state) return;
    repairs.delete(context.deviceId);
    if (state.again && state.nextContext.active()) state.next = startRepair(state.nextContext);
  });
  return state.done;
}

async function readStateOnce(context) {
  const { deviceId } = context;
  const captured = await captureReadFeed(deviceId);
  const projectsAddress = addressOf(deviceId, "", "projects");
  const ids = [...new Set(readStateRows(captured.record?.value).map(entityIdOf).filter(Boolean))];
  const addresses = ids.map((id) => addressOf(deviceId, id, "row"));
  const [projects, ...before] = await readCachedMany([projectsAddress, ...addresses]);
  const fence = pushFence();
  if (!context.active()) return false;
  const board = await context.call("board.list", {});
  if (!context.active()) return false;
  const incoming = new Map(readStateRows(board).map((row) => [entityIdOf(row), row]));
  await Promise.all(addresses.map((address, index) => before[index]?.value
    ? mergeCachedReadState(address, before[index], (held, original) => context.active() && !pushedSince(address, fence)
      ? mergeReadRoster(held, incoming.get(address.entityId), !original) : null) : null));
  await repairReadFeed(context, captured, board);
  // A same-session pass that has not dispatched rosters will observe these
  // marks itself. Once any roster is dispatched, retain the targeted repair.
  if (context.repairOwners !== false) await repairProjectOwners(context, projects);
  return context.active();
}

function repairProjectOwners(context, projects) {
  const owners = (projects?.value || []).map((project) => project.entity_id || project.run_id).filter(Boolean);
  return Promise.all(owners.map((entityId) => repairReadOwner(context, entityId)));
}
