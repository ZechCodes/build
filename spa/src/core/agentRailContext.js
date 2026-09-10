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

const CONTEXTS = { branch: BranchRailContext, issue: IssueRailContext };

export function createAgentRailContext(context) {
  const Context = CONTEXTS[context && context.kind];
  if (!Context) throw new Error(`Unknown agent rail context: ${(context && context.kind) || ""}`);
  return new Context(context);
}
