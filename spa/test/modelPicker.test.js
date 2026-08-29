import { describe, it, expect } from "vitest";
import {
  catalogForProvider,
  modelOptionsHtml,
  effortOptionsHtml,
  effortSupported,
  modelParams,
  providerOptionsHtml,
} from "../src/core/modelPicker.js";

// The selectors are fed by the bridge's models.list RPC (the catalog ships with
// the bridge, never the UI). "Harness default" — empty value — must always be
// offered: it means the user's own Claude Code config decides.

const MODELS = [
  { id: "claude-opus-4-8", label: "Claude Opus 4.8", supports_effort: true },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
];
const EFFORTS = ["low", "medium", "high", "xhigh", "max"];
const CATALOG = {
  default_provider: "claude",
  providers: [
    { id: "claude", label: "Claude Code", models: MODELS, efforts: EFFORTS },
    {
      id: "codex",
      label: "Codex CLI",
      efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
      models: [
        { id: "gpt-5.6-sol", label: "GPT-5.6-Sol", supports_effort: true, efforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
        { id: "gpt-5.6-luna", label: "GPT-5.6-Luna", supports_effort: true, efforts: ["low", "medium", "high", "xhigh", "max"] },
      ],
    },
  ],
};

describe("the harnesses a start can name", () => {
  it("offers the headless claude carrier beside the two CLIs, under the bridge's own label", async () => {
    const { STARTABLE_PROVIDERS, DEFAULT_START_PROVIDER } = await import("../src/core/modelPicker.js");
    expect(STARTABLE_PROVIDERS.map((provider) => provider.id)).toEqual(["claude", "claude_adk", "codex"]);
    expect(STARTABLE_PROVIDERS.find((provider) => provider.id === "claude_adk").label).toBe("Claude Code (headless)");
    // The full PTY harness stays the obvious start; headless is an offer, not a default.
    expect(DEFAULT_START_PROVIDER).toBe("claude");
  });
});

describe("provider catalog", () => {
  it("renders providers and resolves a provider-specific model catalog", () => {
    expect(providerOptionsHtml(CATALOG.providers, "codex")).toContain('value="codex" selected');
    expect(catalogForProvider(CATALOG, "codex").models[0].id).toBe("gpt-5.6-sol");
  });

  it("uses the selected model's reasoning levels", () => {
    const codex = catalogForProvider(CATALOG, "codex");
    const html = effortOptionsHtml(codex.efforts, "", codex.models[1]);
    expect(html).toContain("max");
    expect(html).not.toContain("ultra");
  });

  it("includes provider in dispatch params", () => {
    const codex = catalogForProvider(CATALOG, "codex");
    expect(modelParams(codex.models, "gpt-5.6-sol", "ultra", "codex")).toEqual({
      provider: "codex",
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
  });
});

describe("modelOptionsHtml", () => {
  it("offers harness default first, then the catalog, marking the selection", () => {
    const html = modelOptionsHtml(MODELS, "claude-opus-4-8");
    expect(html.indexOf('value=""')).toBeGreaterThan(-1);
    expect(html.indexOf('value=""')).toBeLessThan(html.indexOf("claude-opus-4-8"));
    expect(html).toContain('value="claude-opus-4-8" selected');
    expect(html).toContain("Claude Haiku 4.5");
  });

  it("keeps a selection that is not in the catalog selectable (newer model)", () => {
    const html = modelOptionsHtml(MODELS, "claude-opus-5");
    expect(html).toContain('value="claude-opus-5" selected');
  });

  it("escapes labels", () => {
    const html = modelOptionsHtml([{ id: "m", label: "<b>x</b>", supports_effort: true }], "");
    expect(html).not.toContain("<b>");
  });
});

describe("effortSupported", () => {
  it("is true for harness default and unknown models, false for no-effort catalog models", () => {
    expect(effortSupported(MODELS, "")).toBe(true);
    expect(effortSupported(MODELS, "claude-opus-4-8")).toBe(true);
    expect(effortSupported(MODELS, "claude-opus-5")).toBe(true);
    expect(effortSupported(MODELS, "claude-haiku-4-5")).toBe(false);
  });
});

describe("effortOptionsHtml", () => {
  it("offers default first and marks the selection", () => {
    const html = effortOptionsHtml(EFFORTS, "xhigh");
    expect(html.indexOf('value=""')).toBeLessThan(html.indexOf("xhigh"));
    expect(html).toContain('value="xhigh" selected');
  });
});

describe("modelParams", () => {
  it("omits empties so the harness default stays the default", () => {
    expect(modelParams(MODELS, "", "")).toEqual({});
    expect(modelParams(MODELS, "claude-opus-4-8", "")).toEqual({
      model: "claude-opus-4-8",
    });
    expect(modelParams(MODELS, "", "high")).toEqual({ effort: "high" });
  });

  it("passes model and effort together", () => {
    expect(modelParams(MODELS, "claude-opus-4-8", "xhigh")).toEqual({
      model: "claude-opus-4-8",
      effort: "xhigh",
    });
  });

  it("drops effort for catalog models that do not support it", () => {
    expect(modelParams(MODELS, "claude-haiku-4-5", "high")).toEqual({
      model: "claude-haiku-4-5",
    });
  });
});
