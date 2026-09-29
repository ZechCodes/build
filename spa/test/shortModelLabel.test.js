// #257: the model a compact row wears — family and version for Claude, version
// and codename for GPT — so it fits whole beside the agent's name. Anything the
// rules do not know keeps the catalog's full label, and nothing is ever blank.
import { describe, expect, it } from "vitest";
import { shortModelLabel, shortModelName } from "../src/core/agentChoice.js";

describe("shortModelName", () => {
  it.each([
    ["claude-opus-5-5", "Opus 5.5"],
    ["claude-sonnet-5-5", "Sonnet 5.5"],
    ["claude-haiku-4-5-20251001", "Haiku 4.5"],
    ["claude-fable-5-1", "Fable 5.1"],
    ["claude-opus-5", "Opus 5"],
    ["claude-opus-5-20260915", "Opus 5"],
    ["claude-opus-5-5-20260915-experimental-preview", "Opus 5.5"],
    ["gpt-6-astra", "6 Astra"],
    ["gpt-6.1-sol", "6.1 Sol"],
    ["gpt-5.6-sol", "5.6 Sol"],
    ["gpt-6-luna", "6 Luna"],
  ])("names the model id %s as %s", (id, expected) => {
    expect(shortModelName(id)).toBe(expected);
  });

  it.each([
    ["Claude Opus 5.5", "Opus 5.5"],
    ["Claude Haiku 4.5", "Haiku 4.5"],
    ["GPT-6-Astra", "6 Astra"],
    ["GPT-6.1 Sol", "6.1 Sol"],
  ])("names the catalog label %s as %s", (label, expected) => {
    expect(shortModelName(label)).toBe(expected);
  });

  it.each([
    ["an alias with no version", "opus"],
    ["a model of another vendor", "llama-3-70b"],
    ["a GPT with no codename", "gpt-4o"],
    ["the old Claude order", "claude-3-5-sonnet-20241022"],
    ["an empty id", ""],
    ["no id at all", undefined],
  ])("knows nothing of %s", (_, id) => {
    expect(shortModelName(id)).toBe("");
  });
});

describe("shortModelLabel", () => {
  const catalog = {
    providers: [
      { id: "claude", models: [
        { id: "claude-opus-5-5", label: "Claude Opus 5.5" },
        { id: "opus", label: "Opus (latest)" },
        { id: "house-model", label: "Claude Sonnet 5.5" },
      ] },
      { id: "codex", models: [{ id: "gpt-6-astra", label: "GPT-6-Astra" }] },
    ],
  };

  it("shortens a model the catalog knows", () => {
    expect(shortModelLabel(catalog, "claude", "claude-opus-5-5")).toBe("Opus 5.5");
    expect(shortModelLabel(catalog, "codex", "gpt-6-astra")).toBe("6 Astra");
  });

  it("shortens a model the catalog does not know, from its id", () => {
    expect(shortModelLabel(catalog, "claude", "claude-sonnet-5-5")).toBe("Sonnet 5.5");
    expect(shortModelLabel(null, "", "gpt-6.1-sol")).toBe("6.1 Sol");
  });

  it("shortens from the catalog's label when the id says nothing", () => {
    expect(shortModelLabel(catalog, "claude", "house-model")).toBe("Sonnet 5.5");
  });

  it("keeps the full label for what no rule knows", () => {
    expect(shortModelLabel(catalog, "claude", "opus")).toBe("Opus (latest)");
    expect(shortModelLabel(null, "", "llama-3-70b")).toBe("llama-3-70b");
  });

  it("is blank only when there is no model at all", () => {
    expect(shortModelLabel(catalog, "claude", "")).toBe("");
    expect(shortModelLabel(catalog, "claude", undefined)).toBe("");
  });
});
