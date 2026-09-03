// @vitest-environment jsdom
// The one provider/model/effort panel both the compose box's advanced flow and
// the toolbar's create menu ask their question with.

import { describe, it, expect } from "vitest";
import {
  NO_AGENT_CHOICE,
  agentChoiceParams,
  agentChoicePanelHtml,
  chosenProviderId,
  modelMenuLabel,
  modelMenuOptions,
  modelMenuSelection,
  modelMenuTitle,
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
    { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
    { id: "codex", label: "Codex TUI", models: [{ id: "gpt", label: "GPT", supports_effort: true }], efforts: ["medium"] },
  ],
};

/** What the bridge serves: four harnesses, under an account whose
 *  default is the plain Claude Code one. */
const fourHarnesses = {
  default_provider: "claude_adk",
  providers: [
    { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
    { id: "claude", label: "Claude Code", models: [], efforts: [] },
    { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
    { id: "codex", label: "Codex TUI", models: [], efforts: [] },
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

  it("offers the two agents, out of the four harnesses the bridge serves", () => {
    const host = mount(agentChoicePanelHtml(fourHarnesses, NO_AGENT_CHOICE, { open: true }));

    const options = [...host.querySelectorAll("#agent-choice-provider option")];
    expect(options.map((option) => option.value)).toEqual(["claude_adk", "codex"]);
    expect(options.map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
  });

  // A preference stored when the other carrier was on offer. It is a record,
  // and the offer is where it clamps: the panel paints the Claude Code entry
  // rather than painting nothing selected.
  it("paints a stored preference for the other claude carrier as Claude Code", () => {
    const host = mount(agentChoicePanelHtml(fourHarnesses, { provider: "claude", model: "", effort: "" }, { open: true }));

    expect(host.querySelector("#agent-choice-provider").value).toBe("claude_adk");
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

  // What the select painted is what the send carries. A stale token cannot ride
  // out on the wire under a control that showed the reader a different name.
  it("sends the agent the panel painted, not the stale token behind it", () => {
    expect(agentChoiceParams(fourHarnesses, { provider: "claude", model: "", effort: "" })).toEqual({
      provider: "claude_adk",
    });
    expect(agentChoiceParams(fourHarnesses, NO_AGENT_CHOICE)).toEqual({});
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

// The composer's own control: one menu, on the left of the row, saying what the
// next turn will run on. It asks nothing about the harness — an agent is locked
// to the one it was created on — so the only questions left are the model and
// how hard it should think.
describe("the composer's model menu", () => {
  const options = (choice, providerId = "claude") => modelMenuOptions(catalog, providerId, choice);
  const ids = (choice, providerId) => options(choice, providerId).map((option) => option.id);
  const chosen = (choice, providerId) =>
    options(choice, providerId).filter((option) => option.selected).map((option) => option.id);

  it("offers the harness's own models, the harness default first", () => {
    expect(ids(NO_AGENT_CHOICE)).toContain("model:");
    expect(ids(NO_AGENT_CHOICE).slice(0, 3)).toEqual(["model:", "model:opus", "model:haiku"]);
    expect(options(NO_AGENT_CHOICE)[1].label).toBe("Opus");
    // The agent's OWN catalog: a codex agent is never offered a claude model.
    expect(ids(NO_AGENT_CHOICE, "codex")).toContain("model:gpt");
    expect(ids(NO_AGENT_CHOICE, "codex")).not.toContain("model:opus");
  });

  it("never offers a harness — the agent is locked to the one it was made on", () => {
    expect(ids({ provider: "claude", model: "opus", effort: "" }).some((id) => id.startsWith("provider"))).toBe(false);
  });

  it("marks what is in use now, on both questions", () => {
    expect(chosen(NO_AGENT_CHOICE)).toEqual(["model:", "effort:"]);
    expect(chosen({ provider: "claude", model: "opus", effort: "high" })).toEqual(["model:opus", "effort:high"]);
  });

  it("offers the chosen model's own reasoning levels", () => {
    expect(ids({ provider: "claude", model: "opus", effort: "" })).toEqual([
      "model:", "model:opus", "model:haiku", "effort:", "effort:low", "effort:high",
    ]);
  });

  it("drops the effort question for a model that does not answer it", () => {
    expect(ids({ provider: "claude", model: "haiku", effort: "" })).toEqual(["model:", "model:opus", "model:haiku"]);
  });

  it("says on its button what the next turn will run on", () => {
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE)).toBe("Default model");
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "" })).toBe("Opus");
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "high" })).toBe("Opus · high");
    // A model the catalog does not carry — an entity dispatched on a newer
    // bridge — reads as the id the bridge holds rather than as nothing.
    expect(modelMenuLabel(catalog, "claude", { model: "claude-opus-5", effort: "" })).toBe("claude-opus-5");
  });

  it("says the model the agent is running on, not the choice behind it", () => {
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE, "opus")).toBe("Opus");
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "" }, "opus-20260214")).toBe("Opus");
  });

  it("says Default model only for an agent that has never run and chose nothing", () => {
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE, "")).toBe("Default model");
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE, "haiku")).toBe("Haiku");
    expect(modelMenuLabel(catalog, "claude", { model: "haiku", effort: "" }, "")).toBe("Haiku");
  });

  it("names both when the next start will spend a different model", () => {
    expect(modelMenuLabel(catalog, "claude", { model: "haiku", effort: "" }, "opus")).toBe("Opus → Haiku");
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "high" }, "haiku")).toBe("Haiku → Opus · high");
  });

  it("keeps the effort beside the model the menu is holding", () => {
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "high" }, "opus")).toBe("Opus · high");
    expect(modelMenuLabel(catalog, "claude", { model: "", effort: "high" }, "opus")).toBe("Opus · high");
  });

  it("reads an announced model the catalog does not carry as the id the bridge holds", () => {
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE, "claude-fable-5-1")).toBe("claude-fable-5-1");
    expect(modelMenuLabel(catalog, "claude", { model: "opus", effort: "" }, "claude-fable-5-1")).toBe(
      "claude-fable-5-1 → Opus",
    );
  });

  it("says in words that the pending model waits for the next start", () => {
    expect(modelMenuTitle(catalog, "claude", NO_AGENT_CHOICE, "")).toBe("Model and reasoning effort");
    expect(modelMenuTitle(catalog, "claude", { model: "opus", effort: "" }, "opus")).toBe("Model and reasoning effort");
    expect(modelMenuTitle(catalog, "claude", { model: "haiku", effort: "" }, "opus")).toBe(
      "Running Opus. Haiku at the next start.",
    );
  });

  it("says in words that the next start takes the harness default, once the pick is cleared", () => {
    expect(modelMenuTitle(catalog, "claude", NO_AGENT_CHOICE, "opus")).toBe(
      "Running Opus. The harness default at the next start.",
    );
    expect(modelMenuLabel(catalog, "claude", NO_AGENT_CHOICE, "opus")).toBe("Opus");
  });

  it("names one model, not two of the same, for ids the catalog does not carry", () => {
    expect(modelMenuLabel(catalog, "claude", { model: "claude-fable-5-1", effort: "" }, "claude-fable-5-1")).toBe(
      "claude-fable-5-1",
    );
    expect(modelMenuTitle(catalog, "claude", { model: "claude-fable-5-1", effort: "" }, "claude-fable-5-1")).toBe(
      "Model and reasoning effort",
    );
  });

  it("turns a press into the next choice, keeping the harness out of it", () => {
    const current = { provider: "claude", model: "opus", effort: "high" };
    expect(modelMenuSelection("effort:low", current)).toEqual({ provider: "claude", model: "opus", effort: "low" });
    expect(modelMenuSelection("effort:", current)).toEqual({ provider: "claude", model: "opus", effort: "" });
    // A model change drops the effort that hung off the model before it.
    expect(modelMenuSelection("model:haiku", current)).toEqual({ provider: "claude", model: "haiku", effort: "" });
    expect(modelMenuSelection("model:", current)).toEqual({ provider: "claude", model: "", effort: "" });
  });
});
