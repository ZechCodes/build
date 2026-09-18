// @vitest-environment jsdom
// The device's project agent: the harness, model and reasoning effort a
// project's agent starts on, held by the machine that runs it.
//
// It is persistent, so no browser is asked at first use — the panel reads what
// the bridge holds and writes back to it, beside the fallback agent.

import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  mountProjectAgentSetting,
  projectAgentChoiceOf,
  projectAgentPanelHtml,
} from "../src/core/projectAgentSetting.js";

const flush = () => new Promise((done) => setTimeout(done, 0));

const CLAUDE_MODELS = [
  { id: "claude-opus-5", label: "Opus 5", supports_effort: true, efforts: ["low", "high"] },
  { id: "claude-haiku-4-5", label: "Haiku 4.5", supports_effort: false, efforts: [] },
];
const CODEX_MODELS = [
  { id: "gpt-5.6-sol", label: "GPT-5.6-Sol", supports_effort: true, efforts: ["medium", "high"] },
];
const CATALOG = {
  default_provider: "claude_adk",
  providers: [
    { id: "claude_adk", label: "Claude Code", models: CLAUDE_MODELS, efforts: ["low", "medium", "high"] },
    { id: "codex", label: "Codex TUI", models: CODEX_MODELS, efforts: ["medium", "high"] },
  ],
};

const panel = () => {
  document.body.innerHTML = projectAgentPanelHtml();
  return document.body;
};
const harness = () => document.getElementById("projectagentharness");
const model = () => document.getElementById("projectagentmodel");
const effort = () => document.getElementById("projectagenteffort");
const values = (control) => [...control.options].map((option) => option.value);
const change = async (control, value) => {
  control.value = value;
  control.dispatchEvent(new Event("change"));
  await flush();
  await flush();
};

/** A bridge holding `settings`, answering every set with the settings it would
 *  hold afterwards — which is what the real one answers with. */
const bridgeHolding = (settings) => {
  let held = settings;
  const calls = [];
  const call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "models.list") return CATALOG;
    if (method === "settings.set") {
      const patch = params.project_agent;
      const next = patch === null ? {} : { ...held.project_agent };
      for (const [key, value] of Object.entries(patch || {})) {
        if (value === null) delete next[key];
        else next[key] = value;
      }
      held = { ...held, project_agent: next };
    }
    return held;
  });
  return { call, calls };
};

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("what a project agent starts on, as the bridge states it", () => {
  it("is the device's own words", () => {
    expect(
      projectAgentChoiceOf({
        default_harness: "claude_adk",
        project_agent: { provider: "codex", model: "gpt-5.6-sol", effort: "high" },
      }),
    ).toEqual({ provider: "codex", model: "gpt-5.6-sol", effort: "high" });
  });

  it("falls back to the device's default harness where it names none", () => {
    expect(projectAgentChoiceOf({ default_harness: "claude_adk", project_agent: {} })).toEqual({
      provider: "claude_adk",
      model: "",
      effort: "",
    });
    expect(projectAgentChoiceOf({ default_harness: "claude_adk" })).toEqual({
      provider: "claude_adk",
      model: "",
      effort: "",
    });
  });

  it("reads a malformed answer as no preference rather than throwing", () => {
    expect(projectAgentChoiceOf(null)).toEqual({ provider: "", model: "", effort: "" });
    expect(projectAgentChoiceOf({ project_agent: { provider: 7, model: [] } })).toEqual({
      provider: "",
      model: "",
      effort: "",
    });
  });
});

describe("the project-agent panel", () => {
  it("paints the device's choice, its harness's models, and that model's efforts", async () => {
    const bridge = bridgeHolding({
      default_harness: "claude_adk",
      project_agent: { provider: "codex", model: "gpt-5.6-sol", effort: "high" },
    });
    await mountProjectAgentSetting(panel(), { callRpc: bridge.call });
    await flush();

    expect(harness().value).toBe("codex");
    expect(values(harness())).toEqual(["claude_adk", "codex"]);
    // The model list is that harness's catalog and no other's.
    expect(values(model())).toEqual(["", "gpt-5.6-sol"]);
    expect(model().value).toBe("gpt-5.6-sol");
    expect(values(effort())).toEqual(["", "medium", "high"]);
    expect(effort().value).toBe("high");
  });

  it("stands on the device's fallback harness while the choice names none", async () => {
    const bridge = bridgeHolding({ default_harness: "claude_adk", project_agent: {} });
    await mountProjectAgentSetting(panel(), { callRpc: bridge.call });
    await flush();

    expect(harness().value).toBe("claude_adk");
    expect(values(model())).toEqual(["", "claude-opus-5", "claude-haiku-4-5"]);
    expect(model().value).toBe("");
  });

  it("offers no effort for a model that takes none", async () => {
    const bridge = bridgeHolding({
      default_harness: "claude_adk",
      project_agent: { model: "claude-haiku-4-5" },
    });
    await mountProjectAgentSetting(panel(), { callRpc: bridge.call });
    await flush();

    expect(effort().disabled).toBe(true);
    expect(effort().value).toBe("");
  });

  // A model belongs to its harness and an effort to its model, so moving the
  // one above drops what was chosen under it.
  it("saves each change on the bridge and repaints from the answer", async () => {
    const bridge = bridgeHolding({
      default_harness: "claude_adk",
      project_agent: { provider: "codex", model: "gpt-5.6-sol", effort: "high" },
    });
    await mountProjectAgentSetting(panel(), { callRpc: bridge.call });
    await flush();

    await change(harness(), "claude_adk");
    expect(bridge.calls.at(-1)).toEqual({
      method: "settings.set",
      params: { project_agent: { provider: "claude_adk", model: null, effort: null } },
    });
    expect(harness().value).toBe("claude_adk");
    expect(model().value).toBe("");
    expect(document.getElementById("projectagentsaved").textContent).toContain("Saved");

    await change(model(), "claude-opus-5");
    expect(bridge.calls.at(-1).params).toEqual({
      project_agent: { model: "claude-opus-5", effort: null },
    });
    expect(values(effort())).toEqual(["", "low", "high"]);

    await change(effort(), "high");
    expect(bridge.calls.at(-1).params).toEqual({ project_agent: { effort: "high" } });
    expect(effort().value).toBe("high");
  });

  it("clears a word the reader empties rather than saving the empty string", async () => {
    const bridge = bridgeHolding({
      default_harness: "claude_adk",
      project_agent: { provider: "claude_adk", model: "claude-opus-5", effort: "high" },
    });
    await mountProjectAgentSetting(panel(), { callRpc: bridge.call });
    await flush();

    await change(model(), "");
    expect(bridge.calls.at(-1).params).toEqual({ project_agent: { model: null, effort: null } });
    expect(model().value).toBe("");
  });

  it("names a refused save and puts the controls back on what the device holds", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "models.list") return CATALOG;
      if (method === "settings.get") {
        return { default_harness: "claude_adk", project_agent: { provider: "codex" } };
      }
      throw new Error("cannot write the config file");
    });
    await mountProjectAgentSetting(panel(), { callRpc });
    await flush();

    await change(harness(), "claude_adk");

    expect(document.getElementById("projectagenterr").textContent).toContain("cannot write the config file");
    expect(document.getElementById("projectagentsaved").textContent).toBe("");
    expect(harness().value).toBe("codex");
    expect(harness().disabled).toBe(false);
  });

  it("offers nothing it cannot keep when the machine will not answer", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("device offline");
    });
    await mountProjectAgentSetting(panel(), { callRpc });
    await flush();

    expect(document.getElementById("projectagenterr").textContent).toContain("device offline");
    for (const control of [harness(), model(), effort()]) {
      expect(control.options).toHaveLength(0);
      expect(control.disabled).toBe(true);
    }
  });

  it("refuses a harness the catalog does not carry instead of showing another", async () => {
    const callRpc = vi.fn(async (method) =>
      method === "models.list" ? CATALOG : { default_harness: "gemini", project_agent: {} },
    );
    await mountProjectAgentSetting(panel(), { callRpc });
    await flush();

    expect(document.getElementById("projectagenterr").textContent).toMatch(/models\.list\.providers/i);
    expect(harness().options).toHaveLength(0);
    expect(harness().disabled).toBe(true);
  });
});
