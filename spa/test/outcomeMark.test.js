import { describe, it, expect } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { outcomeMarkHtml } from "../src/core/outcomeMark.js";

const okMarkHtml = `<span class="thread-activity-outcome " data-outcome="ok"
    role="img" aria-label="The tool answered">✓</span>`;

const errorMarkHtml = `<span class="thread-activity-outcome blocked" data-outcome="error"
    role="img" aria-label="The tool reported an error">✕</span>`;

const unansweredMarkHtml = `<span class="thread-activity-outcome " data-outcome="unanswered"
    role="img" aria-label="No answer arrived">⊘</span>`;

describe("the one glyph table", () => {
  it("renders the tool-outcome marks byte-identically to the markup lifted out of thread.js", () => {
    expect(outcomeMarkHtml("ok", "The tool answered")).toBe(okMarkHtml);
    expect(outcomeMarkHtml("error", "The tool reported an error")).toBe(errorMarkHtml);
    expect(outcomeMarkHtml("unanswered", "No answer arrived")).toBe(unansweredMarkHtml);
  });

  it("renders no mark at all for a name it does not know", () => {
    expect(outcomeMarkHtml("nonesuch", "x")).toBe("");
  });

  it("takes its label from the caller, because a mark means different things to different callers", () => {
    expect(outcomeMarkHtml("ok", "The subagent finished")).toContain('aria-label="The subagent finished"');
    expect(outcomeMarkHtml("ok", "The subagent finished")).toContain(">✓<");
  });

  it("escapes a label carrying markup", () => {
    expect(outcomeMarkHtml("ok", '"><script>')).toContain('aria-label="&quot;&gt;&lt;script&gt;"');
  });

  it("owns no vocabulary of its own — no tool tokens, no state tokens, no labels", () => {
    const source = coreSourceOf("outcomeMark.js");
    expect(source).not.toContain("The tool answered");
    expect(source).not.toContain("in_progress");
    expect(source).not.toContain("checklist");
  });
});
