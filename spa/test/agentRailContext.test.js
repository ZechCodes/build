import { describe, expect, it, vi } from "vitest";
import { createAgentRailContext } from "../src/core/agentRailContext.js";

describe("agent rail context adapters", () => {
  it("addresses branch detail and history without exposing dispatch conditionals", async () => {
    const call = vi.fn(async () => ({}));
    const context = createAgentRailContext({ kind: "branch", projectId: "p1", branch: "build/chat" });

    await context.detail(call, { agent_id: "agent-2", after_sequence: 5 });
    await context.olderPage(call, { entityId: "run-1", agentId: "agent-2", beforeSequence: 4 });

    expect(context.key).toBe("branch:p1:build/chat");
    expect(context.feedRoute()).toEqual({ name: "branch", projectId: "p1", branch: "build/chat" });
    expect(call).toHaveBeenNthCalledWith(1, "branch.get", {
      project_id: "p1", branch: "build/chat", agent_id: "agent-2", after_sequence: 5,
    });
    expect(call).toHaveBeenNthCalledWith(2, "thread.page", {
      entity_id: "run-1", agent_id: "agent-2", before_sequence: 4,
    });
  });

  it("keeps an execution agent out of the issue roster read", async () => {
    const call = vi.fn(async () => ({}));
    const context = createAgentRailContext({ kind: "issue", projectId: "p1", issueId: "issue-1" });

    await context.detail(call, { agent_id: "agent-1" });

    expect(context.key).toBe("issue:issue-1");
    expect(context.feedRoute()).toEqual({ name: "issue", projectId: "p1", id: "issue-1" });
    expect(call).toHaveBeenCalledWith("issue.get", { issue_id: "issue-1" });
  });

  it("reads workspace chat from workspace.get when the bridge supplies ownership", async () => {
    const owned = { entity_id: "run-1", agents: [{ id: "agent-1" }] };
    const call = vi.fn(async () => owned);
    const context = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "run-1" });

    expect(await context.detail(call, { agent_id: "agent-1", after_sequence: 4 })).toBe(owned);
    expect(context.key).toBe("workspace:run-1");
    expect(context.feedRoute()).toEqual({ name: "workspace", projectId: "p1", workspaceId: "run-1" });
    expect(call).toHaveBeenCalledWith("workspace.get", {
      workspace_id: "run-1", agent_id: "agent-1", after_sequence: 4,
    });
  });

  it("recovers an adopted run by its exact workspace id on a metadata-only bridge", async () => {
    const metadata = { id: "run-1", directories: [{ branch: "build/shared" }] };
    const run = { run_id: "run-1", agents: [{ id: "agent-1" }] };
    const call = vi.fn(async (method) => method === "workspace.get" ? metadata : run);
    const context = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "run-1" });

    expect(await context.detail(call, { after_sequence: 7 })).toBe(run);
    expect(call).toHaveBeenNthCalledWith(2, "run.get", { run_id: "run-1", after_sequence: 7 });
  });

  it("keeps an unowned metadata-only workspace out of another branch conversation", async () => {
    const metadata = { id: "folder-1", directories: [{ branch: "main" }] };
    const call = vi.fn(async (method) => {
      if (method === "run.get") throw new Error("unknown run_id");
      return metadata;
    });
    const context = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "folder-1" });

    expect(await context.detail(call, {})).toBe(metadata);
    expect(call.mock.calls.map(([method]) => method)).toEqual(["workspace.get", "run.get", "board.list"]);
  });

  it("recovers an adopted run whose id differs by matching its exact project and root", async () => {
    const metadata = { id: "external-1", project_id: "p1", root: "/work/exact" };
    const run = { run_id: "run-7", worktree_path: "/work/exact", agents: [{ id: "agent-1" }] };
    const call = vi.fn(async (method, params) => {
      if (method === "workspace.get") return metadata;
      if (method === "run.get" && params.run_id === "external-1") throw new Error("unknown run_id");
      if (method === "board.list") return { items: [
        { kind: "branch", project_id: "p2", run_id: "wrong-project", worktree_path: "/work/exact" },
        { kind: "branch", project_id: "p1", run_id: "run-7", worktree_path: "/work/exact" },
      ] };
      return run;
    });
    const context = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "external-1" });

    expect(await context.detail(call, { agent_id: "agent-1" })).toBe(run);
    expect(call).toHaveBeenLastCalledWith("run.get", { run_id: "run-7", agent_id: "agent-1" });
  });

  it("does not hide a failed compatibility read as an agentless workspace", async () => {
    const call = vi.fn(async (method) => {
      if (method === "run.get") throw new Error("connection lost");
      return { id: "run-1" };
    });
    const context = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "run-1" });

    await expect(context.detail(call, {})).rejects.toThrow("connection lost");
  });
});
