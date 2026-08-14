// The bubble that is open decides which conversation every surface under the
// rail is talking about — the detail poll it reads and the review comments it
// sends. These are the pure pieces of that threading, plus the two call shapes
// the surfaces build from them.

import { describe, it, expect, vi } from "vitest";
import { agentScope, createAgentSelection } from "../src/core/agentSelection.js";
import { diffThreadMessages } from "../src/core/notes.js";

describe("naming the conversation a call is about", () => {
  it("names the chosen agent, and says nothing when none is chosen", () => {
    expect(agentScope("agent-2")).toEqual({ agent_id: "agent-2" });
    // No bubble open yet: the daemon answers with the entity's first agent,
    // which is what every surface before the rail asked for.
    expect(agentScope(null)).toEqual({});
    expect(agentScope("")).toEqual({});
    expect(agentScope(undefined)).toEqual({});
  });
});

describe("the shared selection handle", () => {
  it("starts on nothing and carries whatever the rail last chose", () => {
    const selection = createAgentSelection();
    expect(selection.get()).toBe(null);
    expect(selection.scope()).toEqual({});

    selection.set("agent-1");
    expect(selection.get()).toBe("agent-1");
    expect(selection.scope()).toEqual({ agent_id: "agent-1" });
  });

  it("tells its readers only when the choice actually moved", () => {
    const selection = createAgentSelection("agent-1");
    expect(selection.set("agent-1")).toBe(false);
    expect(selection.set("agent-2")).toBe(true);
    expect(selection.set("agent-2")).toBe(false);
    // Losing the agent (the branch resolved to a checkout with none) is a move
    // too: what was held was that agent's.
    expect(selection.set(null)).toBe(true);
    expect(selection.get()).toBe(null);
  });
});

describe("what the surfaces build with it", () => {
  it("threads the open bubble into a branch's detail poll", async () => {
    const callRpc = vi.fn(async () => ({ run: null }));
    const selection = createAgentSelection();
    const poll = () => callRpc("branch.get", { project_id: "proj-1", branch: "feature", ...selection.scope() });

    await poll();
    expect(callRpc).toHaveBeenLastCalledWith("branch.get", { project_id: "proj-1", branch: "feature" });

    selection.set("agent-2");
    await poll();
    expect(callRpc).toHaveBeenLastCalledWith("branch.get", {
      project_id: "proj-1",
      branch: "feature",
      agent_id: "agent-2",
    });
  });

  it("sends review comments to the agent whose conversation was open", async () => {
    const callRpc = vi.fn(async () => ({ ok: true }));
    const selection = createAgentSelection("agent-2");
    const messages = diffThreadMessages(
      [{ file: "src/app.rs", lnA: 10, lnB: 12, snippet: "let x = 1;", comment: "name this" }],
      "",
      null,
    );

    await callRpc("run.request_changes", { run_id: "run-1", ...selection.scope(), messages });

    const [method, params] = callRpc.mock.calls[0];
    expect(method).toBe("run.request_changes");
    expect(params.agent_id).toBe("agent-2");
    expect(params.messages.map((message) => message.body)).toEqual(["name this"]);
  });
});
