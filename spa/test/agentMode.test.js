// @vitest-environment jsdom
// The account's answer to "which program does Claude Code open".
//
// It is one choice for the account, not a question at every start: the two
// programs are the same agent on the same transcripts, and asking per start
// would put two of them side by side with nothing but a carrier name to tell
// them apart. So the choice lives on the bridge (settings.get / settings.set)
// and is made here, on Account → Settings.
//
// What the controls may say is the whole point of the panel: "Claude Code" and
// "Claude Code TUI". How either one is carried is never a person's word.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { agentModePanelHtml, mountAgentMode, CLAUDE_MODES, CODEX_MODES } from "../src/core/agentMode.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

const panel = () => {
  document.body.innerHTML = agentModePanelHtml();
  return document.body;
};
const claude = () => document.getElementById("claudemode");
const codex = () => document.getElementById("codexmode");
const optionText = (select) => [...select.options].map((option) => option.textContent);

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the modes on offer", () => {
  it("names the two programs Claude Code, and Codex's one program Codex", () => {
    expect(CLAUDE_MODES.map((mode) => mode.label)).toEqual(["Claude Code", "Claude Code TUI"]);
    expect(CODEX_MODES.map((mode) => mode.label)).toEqual(["Codex TUI"]);
  });
});

describe("the mode panel", () => {
  it("shows what the account holds, saying nothing about how it is carried", async () => {
    const callRpc = vi.fn(async () => ({ claude_mode: "headless", codex_mode: "tui" }));
    const host = panel();
    await mountAgentMode(host, { callRpc });
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(claude().value).toBe("headless");
    expect(optionText(claude())).toEqual(["Claude Code", "Claude Code TUI"]);
    // Not one visible word about carriers, on the control or around it.
    expect(host.textContent).not.toMatch(/headless/i);
    expect(host.textContent).not.toMatch(/carrier/i);
  });

  it("wires Codex's field like Claude's and locks it to the one mode it has", async () => {
    const callRpc = vi.fn(async () => ({ claude_mode: "tui", codex_mode: "tui" }));
    await mountAgentMode(panel(), { callRpc });
    await flush();

    expect(codex().value).toBe("tui");
    expect(optionText(codex())).toEqual(["Codex TUI"]);
    expect(codex().disabled).toBe(true);
    // A disabled control with no reason reads as a bug, so the panel says why.
    expect(document.getElementById("codexmodenote").textContent).toContain("one");
  });

  it("saves the chosen mode on the bridge and repaints from what it answers", async () => {
    const callRpc = vi.fn(async (method) =>
      method === "settings.get"
        ? { claude_mode: "headless", codex_mode: "tui" }
        : { claude_mode: "tui", codex_mode: "tui" },
    );
    await mountAgentMode(panel(), { callRpc });
    await flush();

    claude().value = "tui";
    claude().dispatchEvent(new Event("change"));
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { claude_mode: "tui" });
    expect(claude().value).toBe("tui");
    expect(claude().disabled).toBe(false);
    expect(document.getElementById("modesaved").textContent).toContain("Saved");
    // What it changes and what it does not: a running agent keeps its program.
    expect(document.getElementById("modesaved").textContent).toMatch(/already running|new agents/i);
  });

  it("names a refused save and puts the control back on what the bridge holds", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { claude_mode: "headless", codex_mode: "tui" };
      throw new Error("cannot write the config file");
    });
    await mountAgentMode(panel(), { callRpc });
    await flush();

    claude().value = "tui";
    claude().dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(document.getElementById("modeerr").textContent).toContain("cannot write the config file");
    expect(claude().value).toBe("headless");
    expect(claude().disabled).toBe(false);
  });

  it("reads an older bridge's silence as the default rather than an empty control", async () => {
    const callRpc = vi.fn(async () => ({ projects_dir: "/Users/z/Projects" }));
    await mountAgentMode(panel(), { callRpc });
    await flush();

    expect(claude().value).toBe("headless");
    expect(claude().selectedOptions[0].textContent).toBe("Claude Code");
  });

  it("says the bridge is unreachable instead of offering a choice it cannot keep", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("device offline");
    });
    await mountAgentMode(panel(), { callRpc });
    await flush();

    expect(document.getElementById("modeerr").textContent).toContain("device offline");
    expect(claude().disabled).toBe(true);
  });
});

// Where the choice actually lives: Account → Settings, beside the other
// account-wide preferences, painted from the same bridge every device reads.
describe("the Settings page", () => {
  it("carries the mode panel and asks the bridge what the account holds", async () => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    const { App } = await import("../src/app.js");
    const { renderSettings } = await import("../src/views/settings.js");
    App.call = vi.fn(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "settings.get") return { projects_dir: "/p", claude_mode: "tui", codex_mode: "tui" };
      if (method === "models.list") return { default_provider: "claude", providers: [] };
      return {};
    });

    await renderSettings();
    await flush();

    expect(document.getElementById("claudemode").value).toBe("tui");
    expect(document.getElementById("codexmode").disabled).toBe(true);
    expect(document.getElementById("root").textContent).not.toMatch(/headless/i);
  });
});
