import { describe, it, expect } from "vitest";
import {
  catalogForProvider,
  genericProviderId,
  modelOptionsHtml,
  effortOptionsHtml,
  effortSupported,
  modelParams,
  normalizeModelCatalog,
  providerLabel,
  providerOptionsHtml,
  DEFAULT_START_PROVIDER,
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

describe("the agents a start can name", () => {
  it("offers one card per agent a person knows, not one per carrier", () => {
    expect(STARTABLE_PROVIDERS.map((provider) => provider.id)).toEqual(["claude", "codex"]);
    expect(STARTABLE_PROVIDERS.map((provider) => provider.label)).toEqual(["Claude Code", "Codex"]);
    // Which program "Claude Code" opens is the account's answer, so a start
    // names the agent and the bridge resolves the rest.
    expect(DEFAULT_START_PROVIDER).toBe("claude");
  });

  it("calls both claude carriers Claude Code — nothing user-facing tells them apart", () => {
    expect(providerLabel("claude")).toBe("Claude Code");
    expect(providerLabel("claude_adk")).toBe("Claude Code");
    expect(providerLabel("codex")).toBe("Codex");
  });

  it("resolves a carrier to the agent a start names", () => {
    // The bridge records which program a session opened; a start names the
    // agent and lets the account setting choose the program again.
    expect(genericProviderId("claude_adk")).toBe("claude");
    expect(genericProviderId("claude")).toBe("claude");
    expect(genericProviderId("codex")).toBe("codex");
    expect(genericProviderId("")).toBe("");
    expect(genericProviderId(null)).toBe("");
  });

  it("says what an unnamed or unknown provider is, rather than nothing", () => {
    expect(providerLabel("")).toBe("Agent");
    expect(providerLabel("gemini")).toBe("gemini");
  });

  it("never says how a carrier runs", () => {
    const shown = [
      ...STARTABLE_PROVIDERS.map((provider) => provider.label),
      providerLabel("claude"),
      providerLabel("claude_adk"),
      providerLabel("codex"),
    ].join(" ");
    expect(shown).not.toMatch(/headless/i);
  });
});

describe("provider catalog", () => {
  it("renders providers and resolves a provider-specific model catalog", () => {
    expect(providerOptionsHtml(CATALOG.providers, "codex")).toContain('value="codex" selected');
    expect(catalogForProvider(CATALOG, "codex").models[0].id).toBe("gpt-5.6-sol");
  });

  it("keeps one Claude Code in the catalog, however many carriers the bridge lists", () => {
    // The bridge serves a catalog per carrier — an entity persisted on either
    // one needs its models under its own id — but a person picking an agent
    // must not be shown the same name twice.
    const normalized = normalizeModelCatalog({
      default_provider: "claude",
      providers: [
        { id: "claude", label: "Claude Code", models: MODELS, efforts: EFFORTS },
        { id: "codex", label: "Codex", models: [], efforts: [] },
        { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
      ],
    });
    expect(normalized.providers.map((provider) => provider.id)).toEqual(["claude", "codex"]);
    expect(normalized.default_provider).toBe("claude");
  });

  it("still offers Claude Code when the only carrier the bridge lists is the other one", () => {
    // Folding is by name, not by id: whichever carrier arrives first keeps the
    // name, so no catalog can leave a person with no way to pick Claude Code.
    const normalized = normalizeModelCatalog({
      default_provider: "claude_adk",
      providers: [
        { id: "claude_adk", label: "Claude Code", models: MODELS, efforts: EFFORTS },
        { id: "codex", label: "Codex", models: [], efforts: [] },
      ],
    });
    expect(normalized.providers.map((provider) => provider.id)).toEqual(["claude_adk", "codex"]);
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
