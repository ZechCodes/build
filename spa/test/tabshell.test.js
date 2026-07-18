import { describe, it, expect } from "vitest";
import { tabShellHtml } from "../src/core/tabshell.js";

const tabs = [
  { id: "plan", label: "Plan" },
  { id: "diff", label: "Diff" },
  { id: "files", label: "Files" },
  { id: "term-3", label: "Terminal 1", closable: true },
];

describe("tabShellHtml", () => {
  it("renders every tab, marks the active one, and omits the + when no handler", () => {
    const html = tabShellHtml({ tabs, active: "diff", hasNewTerminal: false });
    expect(html).toContain('data-tab="plan"');
    expect(html).toContain('data-tab="diff"');
    expect(html).toContain('class="t active" data-tab="diff"');
    expect(html).not.toContain("tplus");
  });

  it("renders the + affordance when a new-terminal handler is present", () => {
    const html = tabShellHtml({ tabs, active: "plan", hasNewTerminal: true });
    expect(html).toContain('class="t tplus" data-newterm="1"');
  });

  it("renders a leading back chevron only when back is passed, with its title escaped", () => {
    const bare = tabShellHtml({ tabs, active: "plan", hasNewTerminal: false });
    expect(bare).not.toContain("tback");
    const html = tabShellHtml({
      tabs,
      active: "plan",
      hasNewTerminal: false,
      back: { title: 'Back to <b>"proj"</b>' },
    });
    // The chevron is the FIRST cell in the row.
    expect(html).toMatch(/<div class="tabs"><div class="t tback" data-back="1"/);
    expect(html).not.toContain("<b>");
    expect(html).toContain("&quot;proj&quot;");
    // It carries an escaped aria-label so the chevron is reachable to assistive tech.
    expect(html).toContain('aria-label="Back to &lt;b&gt;&quot;proj&quot;&lt;/b&gt;"');
  });

  it("falls back the back chevron's aria-label to 'Back' when no title is given", () => {
    const html = tabShellHtml({ tabs, active: "plan", hasNewTerminal: false, back: {} });
    expect(html).toContain('aria-label="Back"');
  });

  it("marks only closable tabs with a × carrying the tab id", () => {
    const html = tabShellHtml({ tabs, active: "plan", hasNewTerminal: true });
    expect(html).toContain('<span class="tx" data-close="term-3"');
    // Static tabs get no closer.
    expect(html).not.toContain('data-close="plan"');
  });

  it("escapes tab ids and labels (a malicious terminal ordinal renders inert)", () => {
    const html = tabShellHtml({
      tabs: [{ id: '"><img src=x>', label: "<b>x</b>", closable: true }],
      active: "plan",
      hasNewTerminal: false,
    });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;img");
  });

  it("only one tab is active at a time", () => {
    const html = tabShellHtml({ tabs, active: "files", hasNewTerminal: false });
    expect((html.match(/ active"/g) || []).length).toBe(1);
  });
});
