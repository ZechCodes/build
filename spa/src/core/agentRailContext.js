// What the rail is the rail OF, one adapter per kind of work item.
//
// The rail reads nothing off the wire any more: its agents and its
// conversation come from the cache, and what is left here is how each kind of
// work item is NAMED — the key it is remembered under, the route its row is
// found by, the verb that mints it a conversation — plus the two on-demand
// reads there are: the page above the window a reader has scrolled to the top
// of, and a task's own work item, which the board writes no row for.

import { mergeCached } from "./localCache.js";
import { mergeActivityDigests } from "./activityDigest.js";
import { deviceKey } from "./deviceKey.js";
import { mergeThreadItems } from "./thread.js";

/**
 * Widen the conversation's record with the page above it.
 *
 * The page is folded into the record rather than into whatever the panel is
 * holding, because the record is the conversation: the panel is re-drawn from
 * it, and a widening kept only in the view would be lost to the next write.
 *
 * `beforeSequence` is the seek the page was asked for, and it is what makes
 * the answer safe to fold in: a page carries the items immediately before its
 * seek, so it abuts this window only while the window's floor is still that
 * seek. A round trip is long enough for it to stop being — the reader switched
 * agents, or a fresh window was opened on the newest items. Folding it in then
 * would seat it under a floor it was never below, with everything in between
 * missing and nothing that would ever ask for it again.
 */
export function widenCachedThread(address, page, beforeSequence) {
  if (!address || !page) return Promise.resolve();
  return mergeCached(address, (held) => {
    const items = held?.items || [];
    if (!items.length || items[0].data?.sequence !== beforeSequence) return null;
    return {
      ...held,
      items: mergeThreadItems(items, page.items || []),
      // What the page says about the far end replaces what the last one said:
      // it is the answer about the floor this window now has.
      olderItemsRemain: page.has_more === true,
      activityDigests: mergeActivityDigests(held.activityDigests || [], page),
    };
  });
}

/** The one read a rail makes: the page above the window, written where the
 *  panel reads it from. Shared by every kind of work item — a conversation is
 *  paged the same way whatever holds it. */
async function olderThreadPage(call, { entityId, agentId, beforeSequence, address }) {
  const page = await call("thread.page", {
    entity_id: entityId,
    ...(agentId ? { agent_id: agentId } : {}),
    before_sequence: beforeSequence,
  });
  await widenCachedThread(address, page, beforeSequence);
  return page;
}

class BranchRailContext {
  constructor({ deviceId = null, projectId, branch }) {
    this.kind = "branch";
    this.deviceId = deviceId;
    this.projectId = projectId;
    this.branch = branch;
    this.key = `branch:${projectId}:${branch}`;
  }

  ensureConversation() {
    return null;
  }

  olderPage(call, asked) {
    return olderThreadPage(call, asked);
  }

  feedRoute() {
    return { name: "branch", deviceId: this.deviceId, projectId: this.projectId, branch: this.branch };
  }
}

/**
 * A task, which is the one work item the cache holds nothing for.
 *
 * Tasks left the active-work board (bridge `app/board/views.rs`), so no row is
 * ever built for one, no `state` push carries one, and the sync layer's ordered
 * pass never walks one. `task.get` is the only thing on either side that says
 * who a task's agents are, so this context keeps it — the single exception to
 * "the rail reads the cache", asked once per mount and never on a clock. Stage
 * 9 takes the task surface cache-only and this goes with it.
 */
class TaskRailContext {
  constructor({ deviceId = null, projectId, taskId }) {
    this.kind = "task";
    this.deviceId = deviceId;
    this.projectId = projectId;
    this.taskId = taskId;
    this.key = `task:${taskId}`;
  }

  ensureConversation() {
    return null;
  }

  workItem(call) {
    return call("task.get", { task_id: this.taskId });
  }

  olderPage(call, asked) {
    return olderThreadPage(call, asked);
  }

  feedRoute() {
    return { name: "task", deviceId: this.deviceId, projectId: this.projectId, id: this.taskId };
  }
}

/** A project is a conversation owner the way a workspace is, with one
 *  difference that is the whole point: its agents work in a scratch directory
 *  Build owns, never in the project's checkout. The page mints the owner before
 *  it mounts the rail (views/projectView.js), so the rail is handed the entity
 *  whose row it reads. */
class ProjectRailContext {
  constructor({ deviceId = null, projectId, entityId = null }) {
    this.kind = "project";
    this.deviceId = deviceId;
    this.projectId = projectId;
    this.entityId = entityId;
    // Every machine mints a `proj-1`, so the rail's own name carries the
    // machine the project is on (core/deviceKey.js).
    this.key = `project:${deviceKey(deviceId, projectId)}`;
  }

  ensureConversation(call) {
    return call("project.ensure_conversation", { project_id: this.projectId });
  }

  olderPage(call, asked) {
    return olderThreadPage(call, asked);
  }

  feedRoute() {
    return { name: "project", deviceId: this.deviceId, projectId: this.projectId };
  }
}

class WorkspaceRailContext {
  constructor({ deviceId = null, workspaceId, projectId }) {
    this.kind = "workspace";
    this.deviceId = deviceId;
    this.workspaceId = workspaceId;
    this.projectId = projectId;
    this.key = `workspace:${workspaceId}`;
  }

  ensureConversation(call) {
    return call("workspace.ensure_conversation", { workspace_id: this.workspaceId });
  }

  olderPage(call, asked) {
    return olderThreadPage(call, asked);
  }

  feedRoute() {
    return {
      name: "workspace",
      deviceId: this.deviceId,
      projectId: this.projectId,
      workspaceId: this.workspaceId,
    };
  }
}

const CONTEXTS = {
  branch: BranchRailContext,
  task: TaskRailContext,
  project: ProjectRailContext,
  workspace: WorkspaceRailContext,
};

export function createAgentRailContext(context) {
  const Context = CONTEXTS[context && context.kind];
  if (!Context) throw new Error(`Unknown agent rail context: ${(context && context.kind) || ""}`);
  return new Context(context);
}
