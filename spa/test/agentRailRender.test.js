import { describe, it, expect } from "vitest";
import { railStatusGitHtml, railStatusLeadClass, railStatusLeadHtml } from "../src/core/agentRailRender.js";

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

describe("the git facts at the row's end", () => {
  it("says only the ones it has, and nothing at all without them", () => {
    expect(railStatusGitHtml({ sync: "↑2", stat: "+4 −1" })).toContain("↑2");
    expect(railStatusGitHtml({ sync: "↑2", stat: "+4 −1" })).toContain("+4 −1");
    expect(railStatusGitHtml({ sync: "", stat: "" })).toBe("");
  });

  it("escapes what the row said", () => {
    expect(railStatusGitHtml({ sync: "<b>", stat: "" })).toContain("&lt;b&gt;");
  });
});
