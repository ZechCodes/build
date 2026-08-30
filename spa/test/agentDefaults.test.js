import { describe, it, expect } from "vitest";
import {
  AGENT_DEFAULTS_KEY,
  loadAgentDefaults,
  saveAgentDefaults,
  reconcileAgentDefaults,
} from "../src/core/agentDefaults.js";

const memory = (seed = {}) => {
  const map = new Map(Object.entries(seed));
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, v),
    removeItem: (k) => map.delete(k),
  };
};

describe("loadAgentDefaults", () => {
  it("is all-empty until something is saved — empty means 'ask the catalog'", () => {
    expect(loadAgentDefaults(memory())).toEqual({ provider: "", model: "", effort: "" });
  });

  it("round-trips a saved preference", () => {
    const storage = memory();
    saveAgentDefaults({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" }, storage);
    expect(loadAgentDefaults(storage)).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "ultra" });
  });

  it("reads a preference saved on a carrier as the agent that carries it", () => {
    // Back when the picker offered a card per carrier, a browser could save the
    // concrete one. It means "Claude Code", and which program that opens is now
    // the account's answer — so it loads as the agent, not the carrier.
    const storage = memory({ [AGENT_DEFAULTS_KEY]: '{"provider":"claude_adk","model":"","effort":""}' });
    expect(loadAgentDefaults(storage).provider).toBe("claude");
  });

  it("keeps only the three known fields", () => {
    const storage = memory();
    saveAgentDefaults({ provider: "claude", model: "", effort: "", danger: "rm -rf" }, storage);
    expect(JSON.parse(storage.getItem(AGENT_DEFAULTS_KEY))).toEqual({ provider: "claude", model: "", effort: "" });
  });

  it("reads corrupt or non-object storage as no preference", () => {
    expect(loadAgentDefaults(memory({ [AGENT_DEFAULTS_KEY]: "{oh no" }))).toEqual({ provider: "", model: "", effort: "" });
    expect(loadAgentDefaults(memory({ [AGENT_DEFAULTS_KEY]: "42" }))).toEqual({ provider: "", model: "", effort: "" });
    expect(loadAgentDefaults(memory({ [AGENT_DEFAULTS_KEY]: '{"provider":7}' }))).toEqual({
      provider: "",
      model: "",
      effort: "",
    });
  });

  it("survives a storage that throws (private mode)", () => {
    const hostile = {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
      removeItem() {},
    };
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
