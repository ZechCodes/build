import { describe, it, expect } from "vitest";
import { tabShellHtml, newTabMenuHtml } from "../src/core/tabshell.js";

const tabs = [
  { id: "plan", label: "Plan" },
  { id: "diff", label: "Diff" },
  { id: "files", label: "Files" },
  { id: "term-3", label: "Terminal 1", closable: true },
];

const NEW_TAB_OPTIONS = [
  { id: "shell", label: "Terminal", description: "your shell here" },
  { id: "claude", label: "Claude Code", description: "an interactive session" },
];

describe("tabShellHtml", () => {
  it("renders every tab, marks the active one, and omits the + when no options", () => {
    const html = tabShellHtml({ tabs, active: "diff", newTabOptions: [] });
    expect(html).toContain('data-tab="plan"');
    expect(html).toContain('data-tab="diff"');
    expect(html).toContain('class="t active" data-tab="diff"');
    expect(html).not.toContain("tplus");
  });

  it("renders the + affordance when new-tab options are present", () => {
    const html = tabShellHtml({ tabs, active: "plan", newTabOptions: NEW_TAB_OPTIONS });
    expect(html).toContain('class="t tplus" data-newtab="1"');
    // The + only opens the menu; the choices live in it, not in the row.
    expect(html).not.toContain("Claude Code");
  });

  it("renders a leading back chevron only when back is passed, with its title escaped", () => {
    const bare = tabShellHtml({ tabs, active: "plan", newTabOptions: [] });
    expect(bare).not.toContain("tback");
    const html = tabShellHtml({
      tabs,
      active: "plan",
      newTabOptions: [],
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
    const html = tabShellHtml({ tabs, active: "plan", newTabOptions: [], back: {} });
    expect(html).toContain('aria-label="Back"');
  });

  it("marks only closable tabs with a × carrying the tab id", () => {
    const html = tabShellHtml({ tabs, active: "plan", newTabOptions: NEW_TAB_OPTIONS });
    expect(html).toContain('<span class="tx" data-close="term-3"');
    // Static tabs get no closer.
    expect(html).not.toContain('data-close="plan"');
  });

  it("escapes tab ids and labels (a malicious terminal ordinal renders inert)", () => {
    const html = tabShellHtml({
      tabs: [{ id: '"><img src=x>', label: "<b>x</b>", closable: true }],
      active: "plan",
      newTabOptions: [],
    });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;img");
  });

  it("only one tab is active at a time", () => {
    const html = tabShellHtml({ tabs, active: "files", newTabOptions: [] });
    expect((html.match(/ active"/g) || []).length).toBe(1);
  });
});

describe("newTabMenuHtml", () => {
  it("renders one item per option, each carrying its kind", () => {
    const html = newTabMenuHtml(NEW_TAB_OPTIONS);
    expect(html).toContain('data-kind="shell"');
    expect(html).toContain('data-kind="claude"');
    expect(html).toContain("Claude Code");
    expect(html).toContain("an interactive session");
  });

  it("escapes every option string", () => {
    const html = newTabMenuHtml([{ id: '"><img src=x>', label: "<b>x</b>", description: "<i>d</i>" }]);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<i>d</i>");
    expect(html).toContain("&lt;img");
  });
});
