// @vitest-environment jsdom
// The project agent's own slot: the harness, model and reasoning effort a new
// project agent starts on, per device, laid over the account's own defaults.

import { describe, it, expect, beforeEach } from "vitest";
import {
  PROJECT_AGENT_DEFAULTS_KEY,
  layeredProjectAgentDefaults,
  projectAgentChoice,
  projectAgentDefaultsFor,
  projectAgentDefaultsIn,
  projectAgentDefaultsStorage,
} from "../src/core/projectAgentDefaults.js";
import { harnessDefaultsPanelHtml, mountHarnessDefaults } from "../src/core/harnessDefaults.js";
import { saveDefaultHarness, saveHarnessDefault } from "../src/core/agentDefaults.js";
import { memoryStorage, writeRefusingStorage } from "./memoryStorage.js";

const CATALOG = {
  default_provider: "claude_adk",
  agent_modes: { claude: "headless", codex: "headless" },
  providers: [
    {
      id: "claude_adk",
      label: "Claude Code",
      models: [
        { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
        { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
      ],
      efforts: ["low", "high"],
    },
    {
      id: "codex_app_server",
      label: "Codex",
      models: [{ id: "gpt-5.6-sol", label: "GPT-5.6-Sol", supports_effort: true, efforts: ["medium", "ultra"] }],
      efforts: ["medium", "ultra"],
    },
  ],
};

let storage;
beforeEach(() => {
  storage = memoryStorage();
});

describe("the project agent's slot", () => {
  it("is the account's defaults until the slot says otherwise", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    saveDefaultHarness("claude_adk", storage);

    expect(projectAgentChoice(storage)).toEqual({
      provider: "claude_adk",
      model: "claude-opus-5",
      effort: "high",
    });
  });

  it("says nothing at all while nothing anywhere was chosen", () => {
    expect(projectAgentChoice(storage)).toEqual({ provider: "", model: "", effort: "" });
  });

  // The slot is shaped exactly like the account's value, which is what lets the
  // account's savers write into it untouched.
  it("takes the account's savers through its own storage view", () => {
    const slot = projectAgentDefaultsStorage(storage);
    saveHarnessDefault("codex_app_server", { model: "gpt-5.6-sol", effort: "ultra" }, slot);
    saveDefaultHarness("codex_app_server", slot);

    expect(JSON.parse(storage.getItem(PROJECT_AGENT_DEFAULTS_KEY)).harnesses.codex).toEqual({
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
    expect(projectAgentChoice(storage)).toEqual({
      provider: "codex_app_server",
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
  });

  it("layers field by field, so the slot can name a model and leave the effort alone", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    saveHarnessDefault("claude_adk", { model: "claude-haiku-4-5", effort: "" }, projectAgentDefaultsStorage(storage));

    expect(layeredProjectAgentDefaults(storage).harnesses.claude).toEqual({
      model: "claude-haiku-4-5",
      effort: "high",
    });
    expect(projectAgentDefaultsFor("claude_adk", storage)).toEqual({
      provider: "claude_adk",
      model: "claude-haiku-4-5",
      effort: "high",
    });
  });

  it("reads as no preference when the stored value will not parse", () => {
    storage.setItem(PROJECT_AGENT_DEFAULTS_KEY, "{oops");
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "" }, storage);
    expect(projectAgentDefaultsFor("claude_adk", storage)).toEqual({
      provider: "claude_adk",
      model: "claude-opus-5",
      effort: "",
    });
  });

  it("keeps working where nothing can be stored", () => {
    const refusing = writeRefusingStorage();
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "" }, projectAgentDefaultsStorage(refusing));
    expect(projectAgentChoice(refusing)).toEqual({ provider: "", model: "", effort: "" });
  });
});

describe("what a new project agent starts on", () => {
  it("is the slot's harness with that harness's own model and effort", () => {
    const slot = projectAgentDefaultsStorage(storage);
    saveDefaultHarness("codex_app_server", slot);
    saveHarnessDefault("codex_app_server", { model: "gpt-5.6-sol", effort: "ultra" }, slot);

    expect(projectAgentDefaultsIn(CATALOG, storage)).toEqual({
      provider: "codex_app_server",
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
  });

  it("falls back to the account's harness, then to the catalog's own", () => {
    saveDefaultHarness("codex_app_server", storage);
    saveHarnessDefault("codex_app_server", { model: "gpt-5.6-sol", effort: "" }, storage);
    expect(projectAgentDefaultsIn(CATALOG, storage)).toEqual({
      provider: "codex_app_server",
      model: "gpt-5.6-sol",
      effort: "",
    });

    expect(projectAgentDefaultsIn(CATALOG, memoryStorage())).toEqual({
      provider: "claude_adk",
      model: "",
      effort: "",
    });
  });
});

describe("the panel over that slot", () => {
  it("saves what is picked and reads it back", () => {
    document.body.innerHTML = harnessDefaultsPanelHtml({ prefix: "proj", title: "🧭 Project agent" });
    mountHarnessDefaults(document.body, {
      catalog: CATALOG,
      prefix: "proj",
      storage: projectAgentDefaultsStorage(storage),
    });

    const pick = (selector, value) => {
      const select = document.querySelector(selector);
      select.value = value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    };
    pick("#projmodel-codex_app_server", "gpt-5.6-sol");
    pick("#projeffort-codex_app_server", "ultra");
    pick("#projprovider", "codex_app_server");

    expect(document.querySelector("#projmodel-codex_app_server").value).toBe("gpt-5.6-sol");
    expect(document.querySelector("#projprovider").value).toBe("codex_app_server");
    expect(projectAgentChoice(storage)).toEqual({
      provider: "codex_app_server",
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
    // A choice made for the project agent never moves the account's own.
    expect(storage.getItem("build.agentDefaults")).toBe(null);
  });
});
