import { describe, it, expect } from "vitest";
import { railStatusLeadClass, railStatusLeadHtml } from "../src/core/agentRailRender.js";

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
