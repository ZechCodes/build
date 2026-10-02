// Drafts are not replicas. Only successful ownership lists can retire them;
// an expired cache, an unwatched row or an older bridge says nothing about
// whether their owner still exists. Unknown owners remain until we know.

import { readCached, subscribeCache } from "./localCache.js";
import { deleteUiDraftsIfUnwritten, uiDraftRecords } from "./localUiStore.js";

const listId = {
  workspaces: (row) => row?.workspace_id || row?.id,
  projects: (row) => row?.project_id || row?.id,
};
const nonempty = (value) => typeof value === "string" && value.length > 0;
const entityId = (row) => row?.entity_id || row?.run_id;
const validList = (rows, kind) => Array.isArray(rows) && rows.every((row) => nonempty(listId[kind](row)));
const validConversations = (rows) => Array.isArray(rows)
  && rows.every((row) => nonempty(row?.conversation_id));
const conversations = (row) => Array.isArray(row?.conversations) ? row.conversations : [];

/** Positive evidence wins across lists: a canonical conversation can be
 * shared, and unwatched runs need not appear in the board's visible items. */
function liveIds(view) {
  const ids = new Set();
  for (const kind of ["workspaces", "projects", "items", "runs"]) {
    if (!Array.isArray(view[kind])) continue;
    for (const row of view[kind]) addLiveRow(ids, row);
  }
  return ids;
}

function addLiveRow(ids, row) {
  if (!row) return;
  ids.add(listId.workspaces(row));
  ids.add(listId.projects(row));
  ids.add(entityId(row));
  ids.add(row.worktree_id);
  ids.add(row.task_id);
  for (const conversation of conversations(row)) ids.add(conversation?.conversation_id);
  if (Array.isArray(row.agents)) for (const agent of row.agents) ids.add(agent?.conversation_id || agent?.id);
}

/** A disappeared owner retires its previously observed entity and
 * conversations. A present owner retires only conversations its COMPLETE
 * roster no longer names. Missing fields are compatibility, not deletion. */
function ownershipChanges(before, after) {
  const gone = new Set();
  const rosters = new Map();
  const complete = ["workspaces", "projects"].every((kind) => validList(after[kind], kind));
  if (complete) for (const kind of ["workspaces", "projects"]) collectOwnerChanges(kind, before, after, gone, rosters);
  gone.delete(undefined);
  return { gone, rosters };
}

function collectOwnerChanges(kind, before, after, gone, rosters) {
  const current = after[kind];
  const owners = new Set(current.map(listId[kind]));
  for (const row of current) {
    if (entityId(row) && validConversations(row.conversations)) {
      rosters.set(entityId(row), new Set(row.conversations.map((entry) => entry.conversation_id)));
    }
  }
  for (const row of validList(before[kind], kind) ? before[kind] : []) {
    if (owners.has(listId[kind](row))) continue;
    gone.add(entityId(row));
    for (const conversation of conversations(row)) gone.add(conversation?.conversation_id);
  }
}

/** Directory addresses encode the workspace and source separately. Never
 * treat a textual prefix (ws-1 versus ws-10) as ownership. */
function workspaceOf(address) {
  if (address.sub === "workspace-settings:") return address.entityId;
  if (!address.entityId.startsWith("workspace:")) return null;
  try {
    const parts = JSON.parse(address.entityId.slice("workspace:".length));
    return Array.isArray(parts) && typeof parts[0] === "string" ? parts[0] : null;
  } catch {
    return null;
  }
}

const isWorkspaceDraft = (address) => address.sub === address.entityId
  || address.sub === `run:${address.entityId}` || address.sub === `worktree:${address.entityId}`
  || address.sub.startsWith("changes:");

function draftIsGone(address, state) {
  if (!address.entityId || state.live.has(address.entityId)) return false;
  if (address.sub.startsWith("chat:draft:")) return false;
  const workspaceId = workspaceOf(address);
  if (workspaceId) return state.workspaces !== null && !state.workspaces.has(workspaceId);
  const chat = /^chat:agent:([^:]+):(.+)$/.exec(address.sub);
  if (chat) return chatIsGone(address.entityId, chat[1], state);
  return isWorkspaceDraft(address) && state.gone.has(address.entityId);
}

function chatIsGone(conversationId, parentId, state) {
  if (!state.completeConversations) return false;
  if (state.live.has(parentId) && !state.rosters.has(parentId)) return false;
  if (state.gone.has(conversationId) || state.gone.has(parentId)) return true;
  const roster = state.rosters.get(parentId);
  // Older drafts used the parent entity where a conversation id belongs.
  return conversationId !== parentId && Boolean(roster && !roster.has(conversationId));
}

function obsoleteDrafts(entries, before, after) {
  const state = {
    ...ownershipChanges(before, after),
    live: liveIds(after),
    workspaces: validList(after.workspaces, "workspaces")
      ? new Set(after.workspaces.map(listId.workspaces)) : null,
    completeConversations: ["workspaces", "projects"].every((kind) => validList(after[kind], kind)
      && after[kind].every((row) => validConversations(row.conversations))),
  };
  return entries.filter(({ address }) => draftIsGone(address, state));
}

const ownershipWrite = (address) => !address.kind
  || ["feed", "row", "projects", "workspaces"].includes(address.kind);

const recognizedDraft = ({ address }) => Boolean(address.entityId && (
  workspaceOf(address) || /^chat:agent:([^:]+):(.+)$/.test(address.sub) || isWorkspaceDraft(address)
));

/** Capture before the list requests; reconcile before those lists are
 * written. Any intervening ownership write, including a cross-tab cache
 * announcement, makes this pass inconclusive. The next pass can try again. */
export async function prepareDraftPrune(deviceId) {
  let changed = false;
  const dispose = subscribeCache({ deviceId }, (address) => {
    if (ownershipWrite(address)) changed = true;
  });
  try {
    const [entries, workspaces, projects] = await Promise.all([
      uiDraftRecords(deviceId),
      readCached({ deviceId, entityId: "", kind: "workspaces" }),
      readCached({ deviceId, entityId: "", kind: "projects" }),
    ]);
    const before = { workspaces: workspaces?.value, projects: projects?.value };
    const candidates = entries.filter(recognizedDraft);
    return {
      dispose,
      hasCandidates: candidates.length > 0,
      prune: (after, active) => deleteUiDraftsIfUnwritten(
        obsoleteDrafts(candidates, before, after), () => !changed && active(),
      ),
    };
  } catch (error) {
    dispose();
    throw error;
  }
}
