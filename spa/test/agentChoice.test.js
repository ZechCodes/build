// @vitest-environment jsdom
// The one provider/model/effort panel both the compose box's advanced flow and
// the toolbar's create menu ask their question with.

import { describe, it, expect } from "vitest";
import {
  NO_AGENT_CHOICE,
  agentChoiceParams,
  agentChoicePanelHtml,
  chosenProviderId,
  readAgentChoice,
  reconcileAgentChoice,
} from "../src/core/agentChoice.js";

const catalog = {
  default_provider: "claude",
  providers: [
    {
      id: "claude",
      label: "Claude Code",
      models: [
        { id: "opus", label: "Opus", supports_effort: true, efforts: ["low", "high"] },
        { id: "haiku", label: "Haiku", supports_effort: false },
      ],
      efforts: ["low", "high"],
    },
    { id: "codex", label: "Codex", models: [{ id: "gpt", label: "GPT", supports_effort: true }], efforts: ["medium"] },
  ],
};

const mount = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};

describe("which provider a choice is on", () => {
  it("takes the one it named, then the catalog's default, then whatever there is", () => {
    expect(chosenProviderId(catalog, { provider: "codex" })).toBe("codex");
    expect(chosenProviderId(catalog, NO_AGENT_CHOICE)).toBe("claude");
    expect(chosenProviderId({ providers: [{ id: "codex" }] }, { provider: "gone" })).toBe("codex");
    expect(chosenProviderId({ providers: [] }, NO_AGENT_CHOICE)).toBe("");
  });
});

describe("the panel", () => {
  it("starts shut — picking a harness is the rare act", () => {
    const host = mount(agentChoicePanelHtml(catalog, NO_AGENT_CHOICE));
    expect(host.querySelector("[data-agent-choice-toggle]").getAttribute("aria-expanded")).toBe("false");
    expect(host.querySelector(".agent-choice-fields").hidden).toBe(true);
  });

  it("offers the catalog's agents, the chosen agent's models, and its efforts", () => {
    const host = mount(agentChoicePanelHtml(catalog, { provider: "codex", model: "gpt", effort: "medium" }, { open: true }));
    expect([...host.querySelectorAll("#agent-choice-provider option")].map((o) => o.value)).toEqual(["claude", "codex"]);
    expect(host.querySelector("#agent-choice-provider").value).toBe("codex");
    expect([...host.querySelectorAll("#agent-choice-model option")].map((o) => o.value)).toEqual(["", "gpt"]);
    expect(host.querySelector("#agent-choice-effort").value).toBe("medium");
  });

  it("disables effort for a model that has none", () => {
    const host = mount(agentChoicePanelHtml(catalog, { provider: "claude", model: "haiku", effort: "" }, { open: true }));
    expect(host.querySelector("#agent-choice-effort").disabled).toBe(true);
  });

  it("keeps two panels apart by their own prefix", () => {
    const host = mount(agentChoicePanelHtml(catalog, NO_AGENT_CHOICE, { prefix: "tb-choice", open: true }));
    expect(host.querySelector("#tb-choice-provider")).toBeTruthy();
    expect(readAgentChoice(host, "tb-choice").provider).toBe("claude");
  });
});

describe("the choice as params", () => {
  it("omits what was left at the harness's own default", () => {
    expect(agentChoiceParams(catalog, NO_AGENT_CHOICE)).toEqual({});
    expect(agentChoiceParams(catalog, { provider: "claude", model: "opus", effort: "high" })).toEqual({
      provider: "claude",
      model: "opus",
      effort: "high",
    });
  });

  it("drops an effort the chosen model does not support", () => {
    expect(agentChoiceParams(catalog, { provider: "claude", model: "haiku", effort: "high" })).toEqual({
      provider: "claude",
      model: "haiku",
    });
  });
});

describe("reconciling a changed choice", () => {
  it("drops the model with its provider, and the effort with its model", () => {
    expect(reconcileAgentChoice({ provider: "codex", model: "opus", effort: "high" }, { providerChanged: true })).toEqual({
      provider: "codex",
      model: "",
      effort: "",
    });
    expect(reconcileAgentChoice({ provider: "claude", model: "haiku", effort: "high" }, { modelChanged: true })).toEqual({
      provider: "claude",
      model: "haiku",
      effort: "",
    });
  });
});
