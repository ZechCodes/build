// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { panelHeadHtml } = await import("../src/core/agentRail.js");

describe("agent conversation headers", () => {
  for (const mode of ["chat", "tui"]) {
    it.each([
      { provider: "claude_adk", model: "claude-opus-5", label: "Opus" },
      { provider: "codex_app_server", active_model: "gpt-6-astra", label: "Astra" },
      { provider: "pi", model: "claude-sonnet-4-6", label: "Sonnet" },
    ])(`keeps identity and controls without a model label in ${mode}: $provider`, (agent) => {
      const host = document.createElement("div");
      host.innerHTML = panelHeadHtml("Build", mode, {
        agent,
        provider: agent.provider,
        heading: { text: "Bridge roll and #564" },
        watch: { watching: true, watchers: 1, pending: false },
      });

      expect(host.querySelector(".rail-who-name").textContent).toBe("Build");
      expect(host.querySelector(".rail-who-work").textContent).toBe("Bridge roll and #564");
      expect(host.querySelector(".rail-harness-icon")).not.toBeNull();
      expect(host.querySelector(".rail-watch")).not.toBeNull();
      expect(host.querySelector(".pinbtn")).not.toBeNull();
      expect(host.querySelector(".agent-model-badge")).toBeNull();
      expect(host.textContent).not.toContain(agent.label);
    });
  }
});
