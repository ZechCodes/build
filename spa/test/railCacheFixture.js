// The rail reads the cache and nothing else, so a test that wants a rail with
// agents on it puts the row on the disk the way the sync layer would.
//
// One helper per record the rail reads: the work item's row (its agents), the
// conversation window under it, and the machine's project list. Everything is
// written under a device, because every record is one machine's.

import { mergeCached, writeCached } from "../src/core/localCache.js";
import { stampProject, stampRow, stampWorkspace } from "../src/core/feedMerge.js";
import { entityIdOf } from "../src/core/entityId.js";
import { mergeThreadItems, windowFromThreadPayload } from "../src/core/thread.js";

const DEVICE = "dev-1";

/** The entity a row is addressed by — a run behind a branch, a task, a
 *  workspace's conversation owner. */
export const rowEntityId = (row) => entityIdOf(row);

/** Put a work item's row on disk, stamped with its machine the way a feed read
 *  stamps it. Answers the entity it was written under. */
export async function writeRailRow(row, { deviceId = DEVICE } = {}) {
  const entityId = rowEntityId(row);
  if (!entityId) return null;
  await writeCached({ deviceId, entityId, kind: "row", sub: "" }, stampRow(row, deviceId));
  return entityId;
}

/** The whole board this device holds: its rows, and the two lists a workspace
 *  route is named by. */
export async function writeRailBoard(
  { items = [], projects = [], workspaces = [] },
  { deviceId = DEVICE } = {},
) {
  await writeCached(
    { deviceId, entityId: "", kind: "projects", sub: "" },
    projects.map((project) => stampProject(project, deviceId)),
  );
  await writeCached(
    { deviceId, entityId: "", kind: "workspaces", sub: "" },
    workspaces.map((workspace) => stampWorkspace(workspace, deviceId)),
  );
  for (const row of items) await writeRailRow(row, { deviceId });
}

/** One conversation's window, as a `thread.page` answer would shape it. */
export function writeRailThread(entityId, conversationKey, threadPayload, { deviceId = DEVICE } = {}) {
  return writeCached(
    { deviceId, entityId, kind: "thread", sub: conversationKey },
    windowFromThreadPayload(threadPayload) || { items: [], deliveredSequence: 0, activityDigests: [] },
  );
}

/** A whole work item on disk: its row, and the conversation its fixture names
 *  — under the execution context where the row has one (a task's agent runs
 *  in its implementation run), else under every agent on it, because the bridge
 *  answers the same conversation whichever of them is asked about. */
export async function writeRailWorkItem(row, { deviceId = DEVICE } = {}) {
  const entityId = await writeRailRow(row, { deviceId });
  if (!entityId) return null;
  const thread = (row.run && row.run.thread) || row.thread || null;
  if (!thread || !(thread.items || []).length) return entityId;
  const execution = row.execution_context;
  if (execution?.entity_id) {
    await writeRailThread(
      execution.entity_id,
      execution.conversation_id || execution.agent_id,
      thread,
      { deviceId },
    );
    return entityId;
  }
  for (const agent of row.agents || []) {
    await writeRailThread(entityId, agent.conversation_id || agent.id, thread, { deviceId });
  }
  return entityId;
}

/** More conversation, the way a thread push lands: appended to the window the
 *  record already holds, with the cursor moved past it. */
export function pushRailThreadItems(entityId, conversationKey, items, { deviceId = DEVICE } = {}) {
  const newest = items.reduce((highest, item) => Math.max(highest, item.data?.sequence || 0), 0);
  return mergeCached({ deviceId, entityId, kind: "thread", sub: conversationKey }, (held) => ({
    activityDigests: [],
    ...held,
    items: mergeThreadItems(held?.items || [], items),
    deliveredSequence: Math.max(Number(held?.deliveredSequence || 0), newest),
  }));
}
