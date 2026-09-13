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
  constructor({ projectId, branch }) {
    this.kind = "branch";
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
    return { name: "branch", projectId: this.projectId, branch: this.branch };
  }
}

class IssueRailContext {
  constructor({ projectId, issueId }) {
    this.kind = "issue";
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
    return { name: "issue", projectId: this.projectId, id: this.issueId };
  }
}

class WorkspaceRailContext {
  constructor({ workspaceId, projectId }) {
    this.kind = "workspace";
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
    return { name: "workspace", projectId: this.projectId, workspaceId: this.workspaceId };
  }
}

const CONTEXTS = { branch: BranchRailContext, issue: IssueRailContext, workspace: WorkspaceRailContext };

export function createAgentRailContext(context) {
  const Context = CONTEXTS[context && context.kind];
  if (!Context) throw new Error(`Unknown agent rail context: ${(context && context.kind) || ""}`);
  return new Context(context);
}
