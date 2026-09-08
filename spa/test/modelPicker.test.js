import { describe, it, expect } from "vitest";
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
  matchCatalogModel,
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
    expect(STARTABLE_PROVIDERS.map((provider) => provider.id)).toEqual([
      "claude_adk",
      "claude",
      "codex_app_server",
      "codex",
    ]);
    expect(STARTABLE_PROVIDERS.map((provider) => provider.label)).toEqual([
      "Claude Code",
      "Claude Code TUI",
      "Codex",
      "Codex TUI",
    ]);
  });

  it("gives every harness its own name, so no two agents read alike", () => {
    expect(providerLabel("claude_adk")).toBe("Claude Code");
    expect(providerLabel("claude")).toBe("Claude Code TUI");
    expect(providerLabel("codex_app_server")).toBe("Codex");
    expect(providerLabel("codex")).toBe("Codex TUI");
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
      providerLabel("codex_app_server"),
      providerLabel("codex"),
    ].join(" ");
    expect(shown).not.toMatch(/headless/i);
  });
});

describe("the catalog a create surface offers", () => {
  const fourProviders = (defaultProvider) => ({
    default_provider: defaultProvider,
    providers: [
      { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
      { id: "claude", label: "Claude Code", models: [], efforts: [] },
      { id: "codex_app_server", label: "Codex", models: [{ id: "gpt-app", label: "GPT App" }], efforts: ["medium"] },
      { id: "codex", label: "Codex TUI", models: [{ id: "gpt-tui", label: "GPT TUI" }], efforts: ["low"] },
    ],
  });

  it("keeps Codex on the TUI for a bridge with a flat catalog", () => {
    const offered = creatableCatalog(normalizeModelCatalog({
      default_provider: "codex_app_server",
      models: MODELS,
      efforts: EFFORTS,
    }));

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
  });

  it("keeps Codex on the TUI for a bridge that lists only the original three providers", () => {
    const offered = creatableCatalog({
      default_provider: "codex_app_server",
      providers: [
        { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
        { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
        { id: "codex", label: "Codex", models: [], efforts: [] },
      ],
    });

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
  });

  it("offers two generic agents out of the four concrete harnesses the bridge serves", () => {
    const offered = creatableCatalog(fourProviders("claude_adk"));

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
    expect(offered.providers.map((provider) => provider.label)).toEqual(["Claude Code", "Codex"]);
  });

  it("gives the Claude Code card the carrier the account chose, and only then", () => {
    const claudeIds = (defaultProvider) =>
      creatableCatalog(fourProviders(defaultProvider)).providers.map((provider) => provider.id);

    expect(claudeIds("claude")).toEqual(["claude", "codex"]);
    expect(claudeIds("claude_adk")).toEqual(["claude_adk", "codex"]);
    expect(claudeIds("codex")).toEqual(["claude_adk", "codex"]);
    expect(claudeIds("codex_app_server")).toEqual(["claude_adk", "codex_app_server"]);
  });

  it("gives the Codex card the app-server carrier only when that is the account default", () => {
    const codexId = (defaultProvider) =>
      creatableCatalog(fourProviders(defaultProvider)).providers[1].id;

    expect(codexId("codex")).toBe("codex");
    expect(codexId("codex_app_server")).toBe("codex_app_server");
    expect(codexId("claude")).toBe("codex");
    expect(codexId("claude_adk")).toBe("codex");
  });

  it.each([
    [{ claude: "headless", codex: "headless" }, ["claude_adk", "codex_app_server"]],
    [{ claude: "headless", codex: "tui" }, ["claude_adk", "codex"]],
    [{ claude: "tui", codex: "headless" }, ["claude", "codex_app_server"]],
    [{ claude: "tui", codex: "tui" }, ["claude", "codex"]],
  ])("uses explicit agent modes independently of the fallback agent: %o", (agent_modes, expected) => {
    const offered = creatableCatalog({ ...fourProviders("claude_adk"), agent_modes });

    expect(offered.providers.map((provider) => provider.id)).toEqual(expected);
  });

  it("carries each agent the models the bridge listed for the carrier behind it", () => {
    const appServerOffered = creatableCatalog(fourProviders("codex_app_server"));
    const tuiOffered = creatableCatalog(fourProviders("codex"));

    expect(appServerOffered.providers[0].models).toEqual(MODELS);
    expect(appServerOffered.providers[0].efforts).toEqual(EFFORTS);
    expect(appServerOffered.providers[1].models.map((model) => model.id)).toEqual(["gpt-app"]);
    expect(appServerOffered.providers[1].efforts).toEqual(["medium"]);
    expect(tuiOffered.providers[1].models.map((model) => model.id)).toEqual(["gpt-tui"]);
    expect(tuiOffered.providers[1].efforts).toEqual(["low"]);
  });

  it("offers both agents before the bridge has listed a single model", () => {
    const offered = creatableCatalog({});

    expect(offered.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
    expect(offered.providers.every((provider) => provider.models.length === 0)).toBe(true);
  });

  it("never puts the carrier question in front of a person", () => {
    const shown = providerOptionsHtml(creatableCatalog(fourProviders("claude_adk")).providers, "claude_adk");

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
        { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
        { id: "codex", label: "Codex TUI", models: [], efforts: [] },
      ],
    });
    expect(normalized.providers.map((provider) => provider.id)).toEqual([
      "claude_adk",
      "claude",
      "codex_app_server",
      "codex",
    ]);
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

describe("the catalog entry an announced model names", () => {
  it("takes an exact id straight out of the catalog", () => {
    expect(matchCatalogModel(MODELS, "claude-opus-4-8").label).toBe("Claude Opus 4.8");
  });

  it("looks past a date suffix the harness pinned onto the id", () => {
    expect(matchCatalogModel(MODELS, "claude-opus-4-8-20260214").label).toBe("Claude Opus 4.8");
    expect(matchCatalogModel(MODELS, "claude-haiku-4-5-260214").label).toBe("Claude Haiku 4.5");
  });

  it("answers nothing for an id the catalog does not carry, rather than the nearest one", () => {
    expect(matchCatalogModel(MODELS, "claude-opus-4-8-mini")).toBe(null);
    expect(matchCatalogModel(MODELS, "")).toBe(null);
    expect(matchCatalogModel(undefined, "claude-opus-4-8")).toBe(null);
  });
});
