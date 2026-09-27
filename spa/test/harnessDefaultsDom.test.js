// @vitest-environment jsdom
// Settings → Browser agent defaults: one model and reasoning effort per
// harness, and which harness a new task or agent starts on. Saved on every
// change — a preference with a Save button is one people forget to press.

import { describe, it, expect, beforeEach } from "vitest";
import { harnessDefaultsPanelHtml, mountHarnessDefaults } from "../src/core/harnessDefaults.js";
import { loadHarnessDefaults, saveDefaultHarness, saveHarnessDefault } from "../src/core/agentDefaults.js";
import { memoryStorage } from "./memoryStorage.js";

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
    { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
    {
      id: "codex_app_server",
      label: "Codex",
      models: [{ id: "gpt-5.6-sol", label: "GPT-5.6-Sol", supports_effort: true, efforts: ["medium", "ultra"] }],
      efforts: ["medium", "ultra"],
    },
    { id: "codex", label: "Codex TUI", models: [], efforts: [] },
  ],
};

const $ = (selector) => document.querySelector(selector);
const values = (selector) => [...document.querySelectorAll(selector)].map((option) => option.value);
const change = (selector, value) => {
  $(selector).value = value;
  $(selector).dispatchEvent(new Event("change", { bubbles: true }));
};

let storage;
const mount = (catalog = CATALOG) => {
  document.body.innerHTML = harnessDefaultsPanelHtml();
  mountHarnessDefaults(document.body, { catalog, storage });
};

beforeEach(() => {
  storage = memoryStorage();
});

describe("what the panel offers", () => {
  it("has a model and effort for each creatable harness, then the default harness", () => {
    mount();
    expect([...document.querySelectorAll("[data-harness]")].map((row) => row.dataset.harness)).toEqual(["claude_adk", "codex_app_server"]);
    expect(values("#defmodel-claude_adk option")).toEqual(["", "claude-opus-5", "claude-haiku-4-5"]);
    expect(values("#defeffort-claude_adk option")).toEqual(["", "low", "high"]);
    expect(values("#defmodel-codex_app_server option")).toEqual(["", "gpt-5.6-sol"]);
    expect(values("#defprovider option")).toEqual(["claude_adk", "codex_app_server"]);
    expect([...document.querySelectorAll("#defprovider option")].map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
    expect(document.body.textContent).toContain("Claude Code");
    expect(document.body.textContent).toContain("Codex");
  });

  it("shows what is stored: each harness's own preference and the chosen default", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    saveHarnessDefault("codex", { model: "gpt-5.6-sol", effort: "ultra" }, storage);
    saveDefaultHarness("codex", storage);
    mount();
    expect($("#defmodel-claude_adk").value).toBe("claude-opus-5");
    expect($("#defeffort-claude_adk").value).toBe("high");
    expect($("#defmodel-codex_app_server").value).toBe("gpt-5.6-sol");
    expect($("#defeffort-codex_app_server").value).toBe("ultra");
    expect($("#defprovider").value).toBe("codex_app_server");
  });

  it("leads with the catalog's default harness while none is chosen", () => {
    mount();
    expect($("#defprovider").value).toBe("claude_adk");
  });

  it("shuts the effort for a model that takes none", () => {
    saveHarnessDefault("claude_adk", { model: "claude-haiku-4-5", effort: "" }, storage);
    mount();
    expect($("#defeffort-claude_adk").disabled).toBe(true);
    expect($("#defeffort-codex_app_server").disabled).toBe(false);
  });
});

describe("saving", () => {
  it("stores a model under its own harness, and only there", () => {
    mount();
    change("#defmodel-codex_app_server", "gpt-5.6-sol");
    expect(loadHarnessDefaults(storage).harnesses).toEqual({ codex: { model: "gpt-5.6-sol", effort: "" } });
    expect($("#defmodel-claude_adk").value).toBe("");
    expect($("#defsaved").textContent).toBe("Saved.");
  });

  it("drops the effort that hung off the model just changed", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    mount();
    change("#defmodel-claude_adk", "claude-haiku-4-5");
    expect(loadHarnessDefaults(storage).harnesses.claude).toEqual({ model: "claude-haiku-4-5", effort: "" });
    expect($("#defeffort-claude_adk").disabled).toBe(true);
  });

  it("stores an effort beside the model it goes with", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "" }, storage);
    mount();
    change("#defeffort-claude_adk", "low");
    expect(loadHarnessDefaults(storage).harnesses.claude).toEqual({ model: "claude-opus-5", effort: "low" });
  });

  it("stores the default harness without touching any harness's preference", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    mount();
    change("#defprovider", "codex_app_server");
    expect(loadHarnessDefaults(storage)).toEqual({
      provider: "codex_app_server",
      harnesses: { claude: { model: "claude-opus-5", effort: "high" } },
    });
    expect($("#defmodel-claude_adk").value).toBe("claude-opus-5");
  });

  it("offers a stored model the catalog no longer carries rather than dropping it silently", () => {
    saveHarnessDefault("claude_adk", { model: "company-custom", effort: "" }, storage);
    mount();
    expect($("#defmodel-claude_adk").value).toBe("company-custom");
  });
});
