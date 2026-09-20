import { describe, expect, it } from "vitest";
import { agentDisplayName, agentInitials, agentName } from "../src/core/agentName.js";

describe("what to call an agent", () => {
  it("prefers the name an agent carries", () => {
    expect(agentDisplayName({ name: "Rail scroll", ordinal: 2 })).toBe("Rail scroll");
    expect(agentName({ name: "  Tracker  " })).toBe("Tracker");
  });

  // The ordinal is what every agent had before names, and what an agent nobody
  // has named still has. A client reading an older bridge sees no `name` at
  // all, and must read exactly as it did.
  it("falls back to the ordinal, and to a caller's place in a list", () => {
    expect(agentDisplayName({ ordinal: 2 })).toBe("Agent 2");
    expect(agentDisplayName({ name: "" }, 3)).toBe("Agent 3");
    expect(agentDisplayName({})).toBe("Agent");
    expect(agentDisplayName(null)).toBe("Agent");
  });

  it("cuts a bubble's letters from the name and nothing else", () => {
    expect(agentInitials({ name: "Rail scroll" })).toBe("RS");
    expect(agentInitials({ name: "Tracker" })).toBe("T");
    expect(agentInitials({ name: "one two three" })).toBe("OT");
    // An unnamed agent keeps its painted pattern; a letter cut from an ordinal
    // would say nothing the pattern does not say better.
    expect(agentInitials({ ordinal: 4 })).toBe("");
  });
});

describe("the renderers that used to print an ordinal", () => {
  it("names an agent in the rail, and keeps the topic after it", async () => {
    const { agentWho, bubbleTip } = await import("../src/core/agentRailModel.js");
    const named = { name: "Rail scroll", topic: "Fix the jump", ordinal: 2, provider: "claude" };
    expect(agentWho(named)).toBe("Rail scroll");
    expect(bubbleTip(named)).toContain("Rail scroll");
    expect(bubbleTip(named)).toContain("Fix the jump");

    // Unnamed reads exactly as it did: the topic, or the harness before there
    // is one.
    expect(agentWho({ topic: "Fix the jump", ordinal: 2 })).toBe("Fix the jump");
    expect(bubbleTip({ topic: "Fix the jump", ordinal: 2 })).toBe("Fix the jump");
  });

  it("names an agent on the project's board, and falls back to its place", async () => {
    const { workspaceAgents } = await import("../src/core/trackerAssignee.js");
    const projectKey = "dev-1|proj-1";
    const feed = {
      workspaces: [
        {
          id: "ws-1",
          workspace_id: "ws-1",
          name: "wire-facade",
          project_id: "proj-1",
          projectKey,
          entity_id: "run-ws-1",
        },
      ],
      items: [
        {
          kind: "branch",
          project_id: "proj-1",
          projectKey,
          run_id: "run-ws-1",
          agents: [
            { id: "agent-1", ordinal: 1, name: "Rail scroll" },
            { id: "agent-2", ordinal: 2 },
          ],
        },
      ],
    };
    const labels = workspaceAgents(feed, projectKey)[0].agents.map((agent) => agent.label);
    expect(labels).toEqual(["wire-facade · Rail scroll", "wire-facade · Agent 2"]);
  });

});
