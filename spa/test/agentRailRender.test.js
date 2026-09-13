import { describe, it, expect } from "vitest";
import { railStatusLeadClass, railStatusLeadHtml, railWhoHtml } from "../src/core/agentRailRender.js";

describe("the lead of the pinned status row", () => {
  it("gives the working shape a word of its own to collapse and a slot for the clock", () => {
    expect(railStatusLeadHtml("working")).toContain('class="rail-status-working-word"');
    expect(railStatusLeadHtml("working")).toContain('class="rail-status-text"');
    expect(railStatusLeadHtml("starting")).not.toContain("rail-status-working-word");
    expect(railStatusLeadHtml("starting")).toContain('class="rail-status-text"');
    expect(railStatusLeadHtml("quiet")).toBe("");
  });

  it("wears the class of the shape it is saying", () => {
    expect(railStatusLeadClass("working")).toBe("rail-status-lead rail-status-working");
    expect(railStatusLeadClass("starting")).toBe("rail-status-lead rail-status-starting");
    expect(railStatusLeadClass("quiet")).toBe("rail-status-lead");
  });
});

describe("the head's name slot", () => {
  it("wears the topic, keeps the harness name as the title, and shimmers 'Starting' until there is one", () => {
    const starting = railWhoHtml("Claude Code 1", { text: "Starting", starting: true });
    expect(starting).toContain('class="rail-who rail-who-starting"');
    expect(starting).toContain('title="Claude Code 1"');
    expect(starting).toContain(">Starting<");

    const named = railWhoHtml("Claude Code 1", { text: "Unify <prompt> delivery", starting: false });
    expect(named).toContain('class="rail-who"');
    expect(named).not.toContain("rail-who-starting");
    expect(named).toContain(">Unify &lt;prompt&gt; delivery<");
    expect(named).toContain('title="Claude Code 1"');
  });

  it("falls back to the harness name when no heading is given", () => {
    expect(railWhoHtml("Codex TUI 2", null)).toContain(">Codex TUI 2<");
  });
});
