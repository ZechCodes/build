// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { agentModelBadge, agentModelBadgeHtml, agentModelsFromFeed } from "../src/core/agentModelBadge.js";
import { actorIdentityHtml } from "../src/core/trackerIdentity.js";
import { threadHtml } from "../src/core/thread.js";
import { withCacheFreshness } from "../src/core/cacheFreshness.js";
import { overviewHtml, overviewRows } from "../src/core/agentOverview.js";

describe("current cached model badges", () => {
  it("prefers the running model over a pending choice and uses its short tooltip", () => {
    expect(agentModelBadge({ active_model: "gpt-6-astra", model: "gpt-6-sol", effort: "high" }))
      .toEqual({ family: "Astra", title: "6 Astra · high" });
    expect(agentModelBadge({ provider: "claude", model: "alias" }, {
      providers: [{ id: "claude", models: [{ id: "alias", label: "Claude Fable 5.1" }] }],
    })).toEqual({ family: "Fable", title: "Fable 5.1" });
  });
  it("omits a missing or unknown model, including an agent on a known harness", () => {
    expect(agentModelBadgeHtml({ provider: "claude_adk" })).toBe("");
    expect(agentModelBadgeHtml({ model: "llama-3-70b", effort: "high" })).toBe("");
  });
  it("keeps cached project and unwatched rosters scoped to their device and project", () => {
    const agent = (id) => ({ id, model: "gpt-6-astra" });
    expect(Object.keys(agentModelsFromFeed({
      projects: [{ projectKey: "dev|proj", agents: [agent("project-agent")] }],
      runs: [{ projectKey: "dev|proj", agents: [agent("unwatched")] }],
      items: [{ projectKey: "dev|proj", agents: [agent("workspace-agent")] },
        { projectKey: "other|proj", agents: [agent("other-device")] }],
    }, "dev|proj"))).toEqual(["project-agent", "unwatched", "workspace-agent"]);
  });
  it("escapes a tooltip's cached effort", () => {
    document.body.innerHTML = agentModelBadgeHtml({ model: "gpt-6-astra", effort: '\"><img src=x onerror=alert(1)>' });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("span").title).toBe('6 Astra · "><img src=x onerror=alert(1)>');
  });
  it("uses the newest roster and does not revive agents removed from it", () => {
    const older = withCacheFreshness({ projectKey: "dev|proj", entity_id: "run", agents: [
      { id: "a", model: "gpt-6-sol" }, { id: "removed", model: "gpt-6-sol" },
    ] }, { at: 1 });
    const newer = withCacheFreshness({ projectKey: "dev|proj", entity_id: "run", agents: [
      { id: "a", model: "gpt-6-astra" },
    ] }, { at: 2 });
    const models = agentModelsFromFeed({ runs: [newer], items: [older] }, "dev|proj");
    expect(models.a.model).toBe("gpt-6-astra");
    expect(models.removed).toBeUndefined();
  });
  it("pairs the overview's running model with its active effort", () => {
    const rows = overviewRows([{ agent: { id: "a", active_model: "gpt-6-astra", active_effort: "high", model: "gpt-6-sol", effort: "low" } }], [null]);
    document.body.innerHTML = overviewHtml(rows);
    expect(document.querySelector(".agent-model-badge").title).toBe("6 Astra · high");
  });
  it("uses the cached roster for a durable task identity and updates with that roster", () => {
    const context = { identities: { a: { agent_id: "a", name: "Planner", provider: "codex" } },
      agentModels: { a: { model: "gpt-6-astra" } } };
    document.body.innerHTML = actorIdentityHtml({ kind: "agent", agent_id: "a" }, context);
    expect(document.querySelector(".agent-model-badge").title).toBe("6 Astra");
    context.agentModels.a = { model: "gpt-6-sol" };
    document.body.innerHTML = actorIdentityHtml({ kind: "agent", agent_id: "a" }, context);
    expect(document.querySelector(".agent-model-badge").textContent).toBe("Sol");
  });
  it("adds the cached model to incoming and outgoing agent names, without trusting message snapshots", () => {
    const reference = { id: "a", name: "Planner", owner: { id: "workspace", kind: "workspace", name: "Navigation" }, model: "gpt-6-sol" };
    const items = [{ type: "message", data: { id: "incoming", sequence: 1, role: "user", body: "Ready", from_agent: reference } },
      { type: "message", data: { id: "outgoing", sequence: 2, role: "agent", body: "Thanks", sent_to: reference } }];
    document.body.innerHTML = threadHtml({ items }, { place: { deviceId: "dev", projectId: "proj" },
      agentModels: { a: { active_model: "claude-opus-5-5" } } });
    expect([...document.querySelectorAll(".agent-model-badge")].map((badge) => badge.title))
      .toEqual(["Opus 5.5", "Opus 5.5"]);
    document.body.innerHTML = threadHtml({ items }, { place: { deviceId: "dev", projectId: "proj" } });
    expect(document.querySelector(".agent-model-badge")).toBeNull();
  });
});
