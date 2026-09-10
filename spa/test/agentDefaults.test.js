import { describe, it, expect } from "vitest";
import {
  AGENT_DEFAULTS_KEY,
  loadAgentDefaults,
  saveAgentDefaults,
  reconcileAgentDefaults,
} from "../src/core/agentDefaults.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

describe("loadAgentDefaults", () => {
  it("is all-empty until something is saved — empty means 'ask the catalog'", () => {
    expect(loadAgentDefaults(memoryStorage())).toEqual({ provider: "", model: "", effort: "" });
  });

  it("round-trips a saved preference", () => {
    const storage = memoryStorage();
    saveAgentDefaults({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" }, storage);
    expect(loadAgentDefaults(storage)).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" });
  });

  it("reads a stored harness as itself — every harness is an agent of its own", () => {
    const storage = memoryStorage({ [AGENT_DEFAULTS_KEY]: '{"provider":"claude_adk","model":"","effort":""}' });
    expect(loadAgentDefaults(storage).provider).toBe("claude_adk");
  });

  it("keeps only the three known fields", () => {
    const storage = memoryStorage();
    saveAgentDefaults({ provider: "claude", model: "", effort: "", danger: "rm -rf" }, storage);
    expect(JSON.parse(storage.getItem(AGENT_DEFAULTS_KEY))).toEqual({ provider: "claude", model: "", effort: "" });
  });

  it("reads corrupt or non-object storage as no preference", () => {
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: "{oh no" }))).toEqual({ provider: "", model: "", effort: "" });
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: "42" }))).toEqual({ provider: "", model: "", effort: "" });
    expect(loadAgentDefaults(memoryStorage({ [AGENT_DEFAULTS_KEY]: '{"provider":7}' }))).toEqual({
      provider: "",
      model: "",
      effort: "",
    });
  });

  it("survives a storage that throws (private mode)", () => {
    const hostile = refusingStorage();
    expect(loadAgentDefaults(hostile)).toEqual({ provider: "", model: "", effort: "" });
    expect(() => saveAgentDefaults({ provider: "claude" }, hostile)).not.toThrow();
  });
});

describe("reconcileAgentDefaults", () => {
  const full = { provider: "claude", model: "opus", effort: "high" };

  it("drops the model and effort when the provider changes — they belonged to the old one", () => {
    expect(reconcileAgentDefaults(full, { providerChanged: true })).toEqual({
      provider: "claude",
      model: "",
      effort: "",
    });
  });

  it("drops the effort when the model changes", () => {
    expect(reconcileAgentDefaults(full, { modelChanged: true })).toEqual({
      provider: "claude",
      model: "opus",
      effort: "",
    });
  });

  it("leaves a settled selection alone", () => {
    expect(reconcileAgentDefaults(full)).toEqual(full);
  });
});
