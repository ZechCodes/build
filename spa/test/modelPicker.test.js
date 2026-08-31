import { describe, it, expect } from "vitest";
import * as modelPicker from "../src/core/modelPicker.js";
import {
  catalogForProvider,
  modelOptionsHtml,
  effortOptionsHtml,
  effortSupported,
  modelParams,
  normalizeModelCatalog,
  providerLabel,
  providerOptionsHtml,
  creatableCatalog,
  STARTABLE_PROVIDERS,
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

describe("the harnesses an agent can be created on", () => {
  it("offers one card per harness — an agent is locked to the one it was made on", () => {
    expect(STARTABLE_PROVIDERS.map((provider) => provider.id)).toEqual(["claude_adk", "claude", "codex"]);
    expect(STARTABLE_PROVIDERS.map((provider) => provider.label)).toEqual([
      "Claude Code",
      "Claude Code TUI",
      "Codex",
    ]);
  });

  it("gives every harness its own name, so no two agents read alike", () => {
    expect(providerLabel("claude_adk")).toBe("Claude Code");
    expect(providerLabel("claude")).toBe("Claude Code TUI");
    expect(providerLabel("codex")).toBe("Codex");
  });

  it("keeps no alias between the two claude harnesses — each one is an agent", () => {
    expect(modelPicker.genericProviderId).toBeUndefined();
    expect(modelPicker.DEFAULT_START_PROVIDER).toBeUndefined();
  });

  it("says what an unnamed or unknown provider is, rather than nothing", () => {
    expect(providerLabel("")).toBe("Agent");
    expect(providerLabel("gemini")).toBe("gemini");
  });

  it("never says how a harness runs", () => {
    const shown = [
      ...STARTABLE_PROVIDERS.map((provider) => provider.label),
      providerLabel("claude"),
      providerLabel("claude_adk"),
      providerLabel("codex"),
    ].join(" ");
    expect(shown).not.toMatch(/headless/i);
  });
});

// What a create surface offers is two agents, never three. Whether Claude Code
// opens as the TUI is the account's question, answered once in Settings, and
// asking it again in front of every new agent is the thing this narrowing ends.
describe("the catalog a create surface offers", () => {
  const threeProviders = (defaultProvider) => ({
    default_provider: defaultProvider,
    providers: [
      { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
      { id: "claude", label: "Claude Code", models: [], efforts: [] },
      { id: "codex", label: "Codex CLI", models: [{ id: "gpt-5.6-sol", label: "GPT-5.6-Sol" }], efforts: ["low"] },
    ],
  });

  it("offers two agents out of the three the bridge serves", () => {
    const offered = creatableCatalog(threeProviders("claude_adk"));

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
    // The labels are the client's one naming table, so an older bridge that
    // called both claude carriers the same thing cannot show the name twice.
    expect(offered.providers.map((provider) => provider.label)).toEqual(["Claude Code", "Codex"]);
  });

  it("gives the Claude Code card the carrier the account chose, and only then", () => {
    const claudeIds = (defaultProvider) =>
      creatableCatalog(threeProviders(defaultProvider)).providers.map((provider) => provider.id);

    expect(claudeIds("claude")).toEqual(["claude", "codex"]);
    expect(claudeIds("claude_adk")).toEqual(["claude_adk", "codex"]);
    // Under a Codex default, "Claude Code" means the plain name's own carrier.
    expect(claudeIds("codex")).toEqual(["claude_adk", "codex"]);
  });

  it("carries each agent the models the bridge listed for the carrier behind it", () => {
    const offered = creatableCatalog(threeProviders("claude_adk"));

    expect(offered.providers[0].models).toEqual(MODELS);
    expect(offered.providers[0].efforts).toEqual(EFFORTS);
    expect(offered.providers[1].models.map((model) => model.id)).toEqual(["gpt-5.6-sol"]);
  });

  // Creating an agent needs only a harness, and the cards paint before the
  // models.list round trip answers — so the offer is two either way.
  it("offers both agents before the bridge has listed a single model", () => {
    const offered = creatableCatalog({});

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
    expect(offered.providers.every((provider) => provider.models.length === 0)).toBe(true);
  });

  it("never puts the carrier question in front of a person", () => {
    const shown = providerOptionsHtml(creatableCatalog(threeProviders("claude_adk")).providers, "claude_adk");

    expect(shown).not.toContain("Claude Code TUI");
    expect(shown).not.toMatch(/headless/i);
  });
});

describe("provider catalog", () => {
  it("renders providers and resolves a provider-specific model catalog", () => {
    expect(providerOptionsHtml(CATALOG.providers, "codex")).toContain('value="codex" selected');
    expect(catalogForProvider(CATALOG, "codex").models[0].id).toBe("gpt-5.6-sol");
  });

  it("keeps every harness the bridge lists, each as itself", () => {
    // Every harness is an agent a person can create, so nothing is folded away:
    // an agent locked to one of them needs its own models under its own id.
    const normalized = normalizeModelCatalog({
      default_provider: "claude_adk",
      providers: [
        { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
        { id: "claude", label: "Claude Code TUI", models: MODELS, efforts: EFFORTS },
        { id: "codex", label: "Codex", models: [], efforts: [] },
      ],
    });
    expect(normalized.providers.map((provider) => provider.id)).toEqual(["claude_adk", "claude", "codex"]);
    expect(normalized.default_provider).toBe("claude_adk");
  });

  it("stands one provider up out of a bridge too old to list any", () => {
    const normalized = normalizeModelCatalog({ models: MODELS, efforts: EFFORTS });
    expect(normalized.providers.map((provider) => provider.id)).toEqual(["claude_adk"]);
    expect(normalized.providers[0].label).toBe("Claude Code");
    expect(normalized.providers[0].models).toEqual(MODELS);
    expect(normalized.default_provider).toBe("claude_adk");
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
