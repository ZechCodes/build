import { deviceKey } from "./deviceKey.js";
import { workspaceRun } from "./workspaceModel.js";

const unknownRun = (error) => /unknown run_id/i.test(error?.message || String(error));
const runMatchesWorkspace = (run, workspace) => {
  if (run.worktree_path && run.worktree_path !== workspace.root) return false;
  if (run.project_id && run.project_id !== workspace.project_id) return false;
  return true;
};

async function legacyWorkspaceDetail(call, workspace, workspaceId, scope) {
  try {
    return await call("run.get", { run_id: workspaceId, ...scope });
  } catch (error) {
    if (!unknownRun(error)) throw error;
  }
  const board = await call("board.list");
  const owner = workspaceRun(workspace, board.items || []);
  if (!owner) return workspace;
  const run = await call("run.get", { run_id: owner.run_id, ...scope });
  return runMatchesWorkspace(run, workspace) ? run : workspace;
}

class BranchRailContext {
  constructor({ deviceId = null, projectId, branch }) {
    this.kind = "branch";
    this.deviceId = deviceId;
    this.projectId = projectId;
    this.branch = branch;
    this.key = `branch:${projectId}:${branch}`;
  }

  detail(call, scope) {
    return call("branch.get", { project_id: this.projectId, branch: this.branch, ...scope });
  }

  ensureConversation() {
    return null;
  }

  olderPage(call, { entityId, agentId, beforeSequence }) {
    return call("thread.page", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      before_sequence: beforeSequence,
    });
  }

  feedRoute() {
    return { name: "branch", deviceId: this.deviceId, projectId: this.projectId, branch: this.branch };
  }
}

class IssueRailContext {
  constructor({ deviceId = null, projectId, issueId }) {
    this.kind = "issue";
    this.deviceId = deviceId;
    this.projectId = projectId;
    this.issueId = issueId;
    this.key = `issue:${issueId}`;
  }

  detail(call, scope) {
    // An issue's selected execution agent belongs to its implementation run,
    // not to the issue roster accepted by issue.get. The issue read still
    // owns metadata and the canonical transcript; execution_context on its
    // answer supplies the addressed run/agent triple for mutations.
    const issueScope = { ...scope };
    delete issueScope.agent_id;
    return call("issue.get", { issue_id: this.issueId, ...issueScope });
  }

  ensureConversation() {
    return null;
  }

  olderPage(call, { entityId, agentId, beforeSequence }) {
    return call("thread.page", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      before_sequence: beforeSequence,
    });
  }

  feedRoute() {
    return { name: "issue", deviceId: this.deviceId, projectId: this.projectId, id: this.issueId };
  }
}

/** A project is a conversation owner the way a workspace is, with one
 *  difference that is the whole point: its agents work in a scratch directory
 *  Build owns, never in the project's checkout. The page mints the owner before
 *  it mounts the rail (views/projectView.js), so the rail is handed the run to
 *  read and reads it as a run. */
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

  detail(call, scope) {
    return call("run.get", { run_id: this.entityId, ...scope });
  }

  ensureConversation(call) {
    return call("project.ensure_conversation", { project_id: this.projectId });
  }

  olderPage(call, { entityId, agentId, beforeSequence }) {
    return call("thread.page", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      before_sequence: beforeSequence,
    });
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

  async detail(call, scope) {
    const workspace = await call("workspace.get", { workspace_id: this.workspaceId, ...scope });
    const payload = workspace.workspace || workspace;
    if ("entity_id" in payload || "agents" in payload) return payload;
    // Workspace-only bridges initially returned metadata here. Recover the
    // exact adopted run without guessing from a branch shared by checkouts.
    return legacyWorkspaceDetail(call, payload, this.workspaceId, scope);
  }
  ensureConversation(call) {
    return call("workspace.ensure_conversation", { workspace_id: this.workspaceId });
  }

  olderPage(call, { entityId, agentId, beforeSequence }) {
    return call("thread.page", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      before_sequence: beforeSequence,
    });
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
  issue: IssueRailContext,
  project: ProjectRailContext,
  workspace: WorkspaceRailContext,
};

export function createAgentRailContext(context) {
  const Context = CONTEXTS[context && context.kind];
  if (!Context) throw new Error(`Unknown agent rail context: ${(context && context.kind) || ""}`);
  return new Context(context);
}
