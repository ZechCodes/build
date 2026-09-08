// @vitest-environment jsdom
// The account fallback used when a coding-agent creation request does not name
// a provider. Visible creation pickers name their Claude Code or Codex provider
// and override it.
//
// The word "headless" stays out of every string a person reads.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defaultHarnessOf, defaultHarnessPanelHtml, mountDefaultHarness } from "../src/core/defaultHarness.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

const panel = () => {
  document.body.innerHTML = defaultHarnessPanelHtml();
  return document.body;
};
const select = () => document.getElementById("defaultharness");
const optionText = (control) => [...control.options].map((option) => option.textContent);
const PROVIDERS = [
  { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
  { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
  { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
  { id: "codex", label: "Codex TUI", models: [], efforts: [] },
  { id: "pi", label: "Pi", models: [], efforts: [] },
];
const CATALOG = { default_provider: "claude_adk", providers: PROVIDERS };

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the harness a settings payload names", () => {
  it("takes the bridge's exact answer when the provider catalog contains it", () => {
    expect(defaultHarnessOf({ default_harness: "claude" }, PROVIDERS)).toBe("claude");
    expect(defaultHarnessOf({ default_harness: "pi" }, PROVIDERS)).toBe("pi");
  });

  it.each([
    ["missing", {}],
    ["malformed", { default_harness: 7 }],
    ["unknown", { default_harness: "gemini" }],
  ])("rejects a %s default instead of selecting another provider", (_kind, settings) => {
    expect(() => defaultHarnessOf(settings, PROVIDERS)).toThrow(/default_harness/i);
  });
});

describe("the fallback-agent panel", () => {
  it("derives its options from the provider catalog, including Pi", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { default_harness: "pi" };
      if (method === "models.list") return CATALOG;
      return {};
    });
    const host = panel();
    await mountDefaultHarness(host, { callRpc });
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(callRpc).toHaveBeenCalledWith("models.list");
    expect(host.querySelectorAll("select")).toHaveLength(1);
    expect(select().value).toBe("pi");
    expect(optionText(select())).toEqual(["Claude Code", "Claude Code TUI", "Codex", "Codex TUI", "Pi"]);
    // Not one visible word about how any of them is carried.
    expect(host.textContent).not.toMatch(/headless/i);
    expect(host.textContent).not.toMatch(/carrier/i);
  });

  it("explains exactly when the fallback applies and when pickers override it", async () => {
    const host = panel();
    await mountDefaultHarness(host, {
      callRpc: vi.fn(async (method) =>
        method === "settings.get" ? { default_harness: "claude_adk" } : CATALOG,
      ),
    });
    await flush();

    expect(host.textContent).toContain(
      "This account fallback is used only when a coding-agent creation request does not name a provider.",
    );
    expect(host.textContent).toContain(
      "Creation pickers send the displayed Claude Code or Codex provider and override this fallback.",
    );
    expect(host.textContent).not.toMatch(/branch|every new/i);
  });

  it("persists Pi on the bridge and repaints from what it answers", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { default_harness: "claude_adk" };
      if (method === "models.list") return CATALOG;
      return { default_harness: "pi" };
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    select().value = "pi";
    select().dispatchEvent(new Event("change"));
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { default_harness: "pi" });
    expect(select().value).toBe("pi");
    expect(select().disabled).toBe(false);
    expect(document.getElementById("harnesssaved").textContent).toContain("Saved");
    expect(document.getElementById("harnesssaved").textContent).toBe(
      "Saved. This fallback applies when a coding-agent creation request does not name a provider.",
    );
  });

  it.each([
    ["missing", {}],
    ["malformed", { default_harness: { id: "pi" } }],
    ["unknown", { default_harness: "gemini" }],
  ])("shows a Settings error for a %s bridge default", async (_kind, settings) => {
    const callRpc = vi.fn(async (method) => (method === "settings.get" ? settings : CATALOG));
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    expect(document.getElementById("harnesserr").textContent).toMatch(/default_harness/i);
    expect(select().options).toHaveLength(0);
    expect(select().value).toBe("");
    expect(select().disabled).toBe(true);
  });

  it.each([
    ["missing providers", {}],
    ["null providers", { providers: null }],
    ["empty providers", { providers: [] }],
  ])("shows a Settings error for a catalog with %s", async (_kind, models) => {
    const callRpc = vi.fn(async (method) =>
      method === "settings.get" ? { default_harness: "pi" } : models,
    );
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    expect(document.getElementById("harnesserr").textContent).toMatch(/models\.list\.providers/i);
    expect(select().options).toHaveLength(0);
    expect(select().value).toBe("");
    expect(select().disabled).toBe(true);
  });

  it.each([
    ["missing", {}],
    ["malformed", { default_harness: 7 }],
    ["unknown", { default_harness: "gemini" }],
  ])("rejects a %s settings.set response and restores the confirmed value", async (_kind, response) => {
    let settingsReads = 0;
    const callRpc = vi.fn(async (method) => {
      if (method === "models.list") return CATALOG;
      if (method === "settings.get") {
        settingsReads += 1;
        return { default_harness: "claude_adk" };
      }
      return response;
    });
    await mountDefaultHarness(panel(), { callRpc });

    select().value = "pi";
    select().dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(document.getElementById("harnesserr").textContent).toMatch(/default_harness/i);
    expect(document.getElementById("harnesssaved").textContent).toBe("");
    expect(settingsReads).toBe(2);
    expect(select().value).toBe("claude_adk");
    expect(select().disabled).toBe(false);
  });

  it("names a refused save and puts the control back on what the bridge holds", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { default_harness: "claude_adk" };
      if (method === "models.list") return CATALOG;
      throw new Error("cannot write the config file");
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    select().value = "claude";
    select().dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(document.getElementById("harnesserr").textContent).toContain("cannot write the config file");
    expect(select().value).toBe("claude_adk");
    expect(select().disabled).toBe(false);
  });

  it("shows both failures when a refused save cannot reload the bridge setting", async () => {
    let settingsReads = 0;
    const callRpc = vi.fn(async (method) => {
      if (method === "models.list") return CATALOG;
      if (method === "settings.get" && settingsReads++ === 0) return { default_harness: "claude_adk" };
      if (method === "settings.get") throw new Error("device went offline");
      throw new Error("cannot write the config file");
    });
    await mountDefaultHarness(panel(), { callRpc });

    select().value = "pi";
    select().dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(document.getElementById("harnesserr").textContent).toContain("cannot write the config file");
    expect(document.getElementById("harnesserr").textContent).toContain("device went offline");
    expect(select().options).toHaveLength(0);
    expect(select().value).toBe("");
    expect(select().disabled).toBe(true);
  });

  it("says the bridge is unreachable instead of offering a choice it cannot keep", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "models.list") return CATALOG;
      throw new Error("device offline");
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    expect(document.getElementById("harnesserr").textContent).toContain("device offline");
    expect(select().options).toHaveLength(0);
    expect(select().value).toBe("");
    expect(select().disabled).toBe(true);
  });
});

// The account fallback lives in Settings beside the other agent preferences.
describe("the Settings page", () => {
  const renderWith = async (call) => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    // The devices panel talks HTTP, not the bridge. Nothing here is about it,
    // and a real request from jsdom hangs until the test's own deadline.
    globalThis.fetch = vi.fn(async () => {
      throw new Error("no network in tests");
    });
    const { App } = await import("../src/app.js");
    const { renderSettings } = await import("../src/views/settings.js");
    App.call = vi.fn(call);
    await renderSettings();
    await flush();
    return App;
  };

  // Each of these re-imports the whole app shell (vi.resetModules), and that
  // transform alone can outrun the default deadline on a loaded machine.
  const SLOW_IMPORT_MS = 30000;

  it("carries the default-agent panel and asks the bridge what the account holds", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return {
        projects_dir: "/p",
        default_harness: "claude",
        agent_modes: { claude: "tui", codex: "headless" },
      };
      if (method === "models.list") return CATALOG;
      return {};
    });

    expect(document.getElementById("defaultharness").value).toBe("claude");
    expect(document.getElementById("agentmode-claude").value).toBe("tui");
    expect(document.getElementById("agentmode-codex").value).toBe("headless");
  }, SLOW_IMPORT_MS);

  // Visible creation pickers send Claude Code or Codex and therefore override
  // the separate fallback selector.
  it("offers the agent defaults two agents, not every default harness", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude_adk" };
      if (method === "models.list") return CATALOG;
      return {};
    });

    const defaults = document.getElementById("defprovider");
    expect([...defaults.options].map((option) => option.value)).toEqual(["claude_adk", "codex"]);
    expect([...defaults.options].map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
    // The account fallback still exposes every provider from the catalog.
    expect([...document.getElementById("defaultharness").options].map((option) => option.textContent)).toEqual([
      "Claude Code",
      "Claude Code TUI",
      "Codex",
      "Codex TUI",
      "Pi",
    ]);
  }, SLOW_IMPORT_MS);

  it("puts the fallback agent beside the other agent preferences", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude_adk" };
      if (method === "models.list") return CATALOG;
      return {};
    });

    const headings = [...document.querySelectorAll("#root .panel h3")].map((h) => h.textContent);
    const at = (word) => headings.findIndex((heading) => heading.includes(word));
    expect(at("Agent defaults")).toBeGreaterThan(-1);
    expect(at("Agent modes")).toBe(at("Agent defaults") + 1);
    expect(at("Fallback agent")).toBe(at("Agent modes") + 1);
    expect(at("Work isolation")).toBe(at("Fallback agent") + 1);
    expect(at("Appearance")).toBe(at("Work isolation") + 1);
  }, SLOW_IMPORT_MS);

  it("updates same-page creation defaults after a mode save even when catalog refresh fails", async () => {
    let current = {
      projects_dir: "/p",
      default_harness: "claude_adk",
      agent_modes: { claude: "headless", codex: "tui" },
    };
    let catalogReads = 0;
    const App = await renderWith(async (method, params) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return current;
      if (method === "settings.set") {
        current = { ...current, agent_modes: { ...current.agent_modes, ...params.agent_modes } };
        return current;
      }
      if (method === "models.list" && catalogReads++ < 2) return CATALOG;
      if (method === "models.list") throw new Error("catalog refresh failed");
      return {};
    });

    const claudeMode = document.getElementById("agentmode-claude");
    claudeMode.value = "tui";
    claudeMode.dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(App.modelCatalog.agent_modes).toEqual({ claude: "tui", codex: "tui" });
    expect([...document.getElementById("defprovider").options].map(({ value }) => value)).toEqual(["claude", "codex"]);
    expect(claudeMode.disabled).toBe(false);
    expect(document.querySelector('[data-agent-mode-status="claude"]').textContent).toBe("Saved.");
  }, SLOW_IMPORT_MS);
});
