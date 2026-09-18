import { describe, it, expect } from "vitest";
import {
  AGENT_DEFAULTS_KEY,
  agentDefaultsFor,
  agentDefaultsIn,
  harnessDefaultsFor,
  loadAgentDefaults,
  loadHarnessDefaults,
  saveDefaultHarness,
  saveHarnessDefault,
} from "../src/core/agentDefaults.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

const NONE = { provider: "", model: "", effort: "" };

describe("loadHarnessDefaults", () => {
  it("names no harness and holds no preference until something is saved", () => {
    expect(loadHarnessDefaults(memoryStorage())).toEqual({ provider: "", harnesses: {} });
    expect(loadAgentDefaults(memoryStorage())).toEqual(NONE);
  });

  it("keeps one model and effort per harness, and the harness a start leads with", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    saveHarnessDefault("codex", { model: "gpt-5.6-sol", effort: "ultra" }, storage);
    saveDefaultHarness("codex", storage);

    expect(loadHarnessDefaults(storage)).toEqual({
      provider: "codex",
      harnesses: {
        claude: { model: "claude-opus-5", effort: "high" },
        codex: { model: "gpt-5.6-sol", effort: "ultra" },
      },
    });
    expect(loadAgentDefaults(storage)).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" });
    expect(agentDefaultsFor("claude_adk", storage)).toEqual({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
  });

  // Both carriers of a family run the same CLI over the same catalog, so a
  // model chosen for Claude Code is the model for Claude Code whichever
  // carrier the device's agent modes pick today.
  it("shares a preference across the carriers of one family", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude", { model: "claude-opus-5", effort: "" }, storage);
    expect(agentDefaultsFor("claude_adk", storage)).toEqual({ provider: "claude_adk", model: "claude-opus-5", effort: "" });
    saveHarnessDefault("codex_app_server", { model: "gpt-5.6-sol", effort: "ultra" }, storage);
    expect(harnessDefaultsFor(loadHarnessDefaults(storage), "codex")).toEqual({ model: "gpt-5.6-sol", effort: "ultra" });
  });

  it("keeps a preference for a harness outside every family under its own name", () => {
    const storage = memoryStorage();
    saveHarnessDefault("pi", { model: "", effort: "low" }, storage);
    expect(agentDefaultsFor("pi", storage)).toEqual({ provider: "pi", model: "", effort: "low" });
    expect(agentDefaultsFor("codex", storage)).toEqual({ provider: "codex", model: "", effort: "" });
  });

  it("changing the default harness keeps every harness's preference", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    saveDefaultHarness("codex", storage);
    saveDefaultHarness("claude_adk", storage);
    expect(loadAgentDefaults(storage)).toEqual({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
  });

  it("reads the older one-trio shape as that harness's preference", () => {
    const storage = memoryStorage({
      [AGENT_DEFAULTS_KEY]: '{"provider":"codex","model":"gpt-5.6-sol","effort":"ultra"}',
    });
    expect(loadHarnessDefaults(storage)).toEqual({
      provider: "codex",
      harnesses: { codex: { model: "gpt-5.6-sol", effort: "ultra" } },
    });
    expect(loadAgentDefaults(storage)).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" });
  });

  it("keeps only the known fields", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude", { model: "", effort: "", danger: "rm -rf" }, storage);
    saveDefaultHarness("claude", storage);
    expect(JSON.parse(storage.getItem(AGENT_DEFAULTS_KEY))).toEqual({
      provider: "claude",
      harnesses: { claude: { model: "", effort: "" } },
    });
  });

  it("reads corrupt or non-object storage as no preference", () => {
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: "{oh no" }))).toEqual(NONE);
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: "42" }))).toEqual(NONE);
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: '{"provider":7}' }))).toEqual(NONE);
    expect(loadHarnessDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: '{"provider":"codex","harnesses":[]}' }))).toEqual({
      provider: "codex",
      harnesses: {},
    });
    expect(loadHarnessDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: '{"harnesses":{"codex":{"model":3}}}' }))).toEqual({
      provider: "",
      harnesses: { codex: { model: "", effort: "" } },
    });
  });

  it("survives a storage that throws (private mode)", () => {
    const hostile = refusingStorage();
    expect(loadAgentDefaults(hostile)).toEqual(NONE);
    expect(() => saveDefaultHarness("claude", hostile)).not.toThrow();
    expect(() => saveHarnessDefault("claude", { model: "x", effort: "" }, hostile)).not.toThrow();
  });
});

describe("agentDefaultsIn", () => {
  const catalog = {
    default_provider: "claude_adk",
    providers: [
      { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
      { id: "codex", label: "Codex", models: [], efforts: [] },
    ],
  };

  it("starts on the stored harness with that harness's model and effort", () => {
    const storage = memoryStorage();
    saveHarnessDefault("codex", { model: "gpt-5.6-sol", effort: "ultra" }, storage);
    saveDefaultHarness("codex", storage);
    expect(agentDefaultsIn(catalog, storage)).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" });
  });

  // A preference for the catalog's default harness applies even while no
  // harness has been chosen as the default: the default IS that harness.
  it("applies the catalog default's preference when no harness was chosen", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" }, storage);
    expect(agentDefaultsIn(catalog, storage)).toEqual({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
  });

  it("clamps a stored carrier the offer does not hold onto its family's, keeping the family's preference", () => {
    const storage = memoryStorage();
    saveHarnessDefault("claude", { model: "claude-opus-5", effort: "high" }, storage);
    saveDefaultHarness("claude", storage);
    expect(agentDefaultsIn(catalog, storage)).toEqual({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
  });
});
