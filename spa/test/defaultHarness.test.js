// @vitest-environment jsdom
// The account's answer to "which agent does a new one start as".
//
// Every harness is an agent of its own, and an agent is locked to the one it
// was created on — its conversation lives there. So the account setting is not
// about how an agent runs any more: it names the harness a NEW agent is created
// on where nobody said otherwise, which is the option the new-agent view leads
// with and the one the bridge falls back to when it has to deliver.
//
// One select, sharing the new-agent view's exact vocabulary, so the two can
// never drift. The word "headless" stays out of every string a person reads.

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

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the harness a settings payload names", () => {
  it("takes the bridge's answer when it names one", () => {
    expect(defaultHarnessOf({ default_harness: "claude" })).toBe("claude");
    expect(defaultHarnessOf({ default_harness: "codex_app_server" })).toBe("codex_app_server");
    expect(defaultHarnessOf({ default_harness: "codex" })).toBe("codex");
  });

  it("reads a bridge that only speaks the old key through it", () => {
    expect(defaultHarnessOf({ claude_mode: "tui" })).toBe("claude");
    expect(defaultHarnessOf({ claude_mode: "headless" })).toBe("claude_adk");
  });

  it("reads silence and nonsense as the default rather than an empty control", () => {
    expect(defaultHarnessOf({})).toBe("claude_adk");
    expect(defaultHarnessOf({ default_harness: "gemini" })).toBe("claude_adk");
    expect(defaultHarnessOf(null)).toBe("claude_adk");
  });
});

describe("the default-agent panel", () => {
  it("is one select, naming all four harnesses — the one place a carrier is chosen", async () => {
    const callRpc = vi.fn(async () => ({ default_harness: "claude_adk", claude_mode: "headless" }));
    const host = panel();
    await mountDefaultHarness(host, { callRpc });
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(host.querySelectorAll("select")).toHaveLength(1);
    expect(select().value).toBe("claude_adk");
    expect(optionText(select())).toEqual(["Claude Code", "Claude Code TUI", "Codex", "Codex TUI"]);
    // Not one visible word about how any of them is carried.
    expect(host.textContent).not.toMatch(/headless/i);
    expect(host.textContent).not.toMatch(/carrier/i);
  });

  // Everywhere an agent is created offers two: Claude Code and Codex. This
  // select is the only place their concrete harnesses appear.
  it("says what choosing Claude Code TUI does, since no other screen can", async () => {
    const host = panel();
    await mountDefaultHarness(host, { callRpc: vi.fn(async () => ({ default_harness: "claude_adk" })) });
    await flush();

    expect(host.textContent).toContain("Claude Code TUI");
    expect(host.textContent).toContain("terminal");
    expect(host.textContent).not.toMatch(/headless/i);
  });

  it("saves the chosen harness on the bridge and repaints from what it answers", async () => {
    const callRpc = vi.fn(async (method) =>
      method === "settings.get" ? { default_harness: "claude_adk" } : { default_harness: "codex_app_server" },
    );
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    select().value = "codex_app_server";
    select().dispatchEvent(new Event("change"));
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { default_harness: "codex_app_server" });
    expect(select().value).toBe("codex_app_server");
    expect(select().disabled).toBe(false);
    expect(document.getElementById("harnesssaved").textContent).toContain("Saved");
    // What it changes and what it does not: the agents already here keep theirs.
    expect(document.getElementById("harnesssaved").textContent).toMatch(/already|new agents/i);
  });

  it("falls back to the old key when the bridge does not know the new one", async () => {
    // An older bridge under a newer client. The two claude harnesses are still
    // sayable there, in the words that bridge speaks.
    const callRpc = vi.fn(async (method, params) => {
      if (method === "settings.get") return { claude_mode: "headless" };
      if (params.default_harness) throw new Error("settings.set: nothing to set");
      return { claude_mode: "tui" };
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    select().value = "claude";
    select().dispatchEvent(new Event("change"));
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { default_harness: "claude" });
    expect(callRpc).toHaveBeenCalledWith("settings.set", { claude_mode: "tui" });
    expect(select().value).toBe("claude");
    expect(document.getElementById("harnesserr").textContent).toBe("");
  });

  it("says the bridge's own refusal for a harness the old key cannot name", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { claude_mode: "headless" };
      throw new Error("settings.set: nothing to set");
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    select().value = "codex_app_server";
    select().dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(callRpc).not.toHaveBeenCalledWith("settings.set", expect.objectContaining({ claude_mode: expect.anything() }));
    expect(document.getElementById("harnesserr").textContent).toContain("nothing to set");
    expect(select().value).toBe("claude_adk");
  });

  it("names a refused save and puts the control back on what the bridge holds", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { default_harness: "claude_adk" };
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

  it("says the bridge is unreachable instead of offering a choice it cannot keep", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("device offline");
    });
    await mountDefaultHarness(panel(), { callRpc });
    await flush();

    expect(document.getElementById("harnesserr").textContent).toContain("device offline");
    expect(select().disabled).toBe(true);
  });
});

// Where the choice actually lives: Account → Settings, beside the other
// account-wide preferences, painted from the same bridge every device reads.
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
  };

  // Each of these re-imports the whole app shell (vi.resetModules), and that
  // transform alone can outrun the default deadline on a loaded machine.
  const SLOW_IMPORT_MS = 30000;

  it("carries the default-agent panel and asks the bridge what the account holds", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude" };
      if (method === "models.list") return { default_provider: "claude", providers: [] };
      return {};
    });

    expect(document.getElementById("defaultharness").value).toBe("claude");
    expect(document.getElementById("root").textContent).not.toMatch(/headless/i);
  }, SLOW_IMPORT_MS);

  // These defaults are spent creating agents, so they offer what every create
  // surface offers — the two agents. Which concrete harness each means is the
  // Default agent panel's question, and it is asked exactly once.
  it("offers the agent defaults the two agents, not the three harnesses", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude_adk" };
      if (method === "models.list") {
        return {
          default_provider: "claude_adk",
          providers: [
            { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
            { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
            { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
            { id: "codex", label: "Codex TUI", models: [], efforts: [] },
          ],
        };
      }
      return {};
    });

    const defaults = document.getElementById("defprovider");
    expect([...defaults.options].map((option) => option.value)).toEqual(["claude_adk", "codex_app_server"]);
    expect([...defaults.options].map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
    // The account's own question is still asked, once, in its own panel.
    expect([...document.getElementById("defaultharness").options].map((option) => option.textContent)).toEqual([
      "Claude Code",
      "Claude Code TUI",
      "Codex",
      "Codex TUI",
    ]);
  }, SLOW_IMPORT_MS);

  it("puts the default agent beside the other agent preferences", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude_adk" };
      if (method === "models.list") return { default_provider: "claude_adk", providers: [] };
      return {};
    });

    const headings = [...document.querySelectorAll("#root .panel h3")].map((h) => h.textContent);
    const at = (word) => headings.findIndex((heading) => heading.includes(word));
    expect(at("Agent defaults")).toBeGreaterThan(-1);
    expect(at("Default agent")).toBe(at("Agent defaults") + 1);
    expect(at("Appearance")).toBe(at("Default agent") + 1);
  }, SLOW_IMPORT_MS);
});
