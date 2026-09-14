import { describe, expect, it, vi } from "vitest";
import { createAgentRailContext } from "../src/core/agentRailContext.js";

describe("agent rail context adapters", () => {
  it("addresses branch detail and history without exposing dispatch conditionals", async () => {
    const call = vi.fn(async () => ({}));
    const context = createAgentRailContext({ kind: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/chat" });

    await context.detail(call, { agent_id: "agent-2", after_sequence: 5 });
    await context.olderPage(call, { entityId: "run-1", agentId: "agent-2", beforeSequence: 4 });

    expect(context.key).toBe("branch:p1:build/chat");
    // The row this rail is about is on one machine: the route it looks for
    // carries the device, the way every other route does.
    expect(context.feedRoute()).toEqual({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/chat" });
    expect(call).toHaveBeenNthCalledWith(1, "branch.get", {
      project_id: "p1", branch: "build/chat", agent_id: "agent-2", after_sequence: 5,
    });
    expect(call).toHaveBeenNthCalledWith(2, "thread.page", {
      entity_id: "run-1", agent_id: "agent-2", before_sequence: 4,
    });
  });

  it("keeps an execution agent out of the issue roster read", async () => {
    const call = vi.fn(async () => ({}));
    const context = createAgentRailContext({ kind: "issue", deviceId: "dev-2", projectId: "p1", issueId: "issue-1" });

    await context.detail(call, { agent_id: "agent-1" });

    expect(context.key).toBe("issue:issue-1");
    expect(context.feedRoute()).toEqual({ name: "issue", deviceId: "dev-2", projectId: "p1", id: "issue-1" });
    expect(call).toHaveBeenCalledWith("issue.get", { issue_id: "issue-1" });
  });
});
