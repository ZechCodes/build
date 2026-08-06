import { describe, it, expect } from "vitest";
import { tabShellHtml, newTabMenuHtml, surfaceMenuHtml } from "../src/core/tabshell.js";

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

const RIGHT_TABS = [{ id: "inbox", glyph: "▤", label: "Inbox" }];
const MENU_ITEMS = [
  { id: "archive", label: "Archive", description: "Retired issues and worktrees" },
  { id: "settings", label: "Project settings", description: "Name, path, base branch" },
];

// The right cluster is the same on every project surface: icon tabs then the ⋯.
describe("tabShellHtml right cluster", () => {
  it("trails the row with icon tabs and the ⋯ button", () => {
    const html = tabShellHtml({ tabs, active: "plan", newTabOptions: [], rightTabs: RIGHT_TABS, menu: MENU_ITEMS });
    expect(html).toContain('<div class="tabs-right">');
    expect(html).toContain('data-tab="inbox"');
    expect(html).toContain('aria-label="Inbox"');
    expect(html).toContain("▤");
    expect(html).toContain('class="t tmenu" data-menu="1"');
    // The cluster comes after every ordinary tab, and the glyph is the whole
    // cell — the label lives in the tooltip.
    expect(html.indexOf('data-tab="files"')).toBeLessThan(html.indexOf("tabs-right"));
    expect(html).not.toContain(">Inbox<");
  });

  it("marks a selected icon tab active exactly like an ordinary tab", () => {
    const html = tabShellHtml({ tabs, active: "inbox", newTabOptions: [], rightTabs: RIGHT_TABS });
    expect(html).toContain('class="t ticon active" data-tab="inbox"');
    expect((html.match(/ active"/g) || []).length).toBe(1);
  });

  it("omits the cluster when the surface passes neither icon tabs nor a menu", () => {
    const html = tabShellHtml({ tabs, active: "plan", newTabOptions: [] });
    expect(html).not.toContain("tabs-right");
    expect(html).not.toContain("tmenu");
  });

  it("escapes every icon-tab string", () => {
    const html = tabShellHtml({
      tabs,
      active: "plan",
      newTabOptions: [],
      rightTabs: [{ id: '"><img src=x>', glyph: "<b>g</b>", label: "<i>l</i>" }],
    });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>g</b>");
    expect(html).not.toContain("<i>l</i>");
    expect(html).toContain("&lt;img");
  });
});

describe("surfaceMenuHtml", () => {
  it("renders one button per action, each carrying its action id", () => {
    const html = surfaceMenuHtml(MENU_ITEMS);
    expect(html).toContain('<button class="mi" data-action="archive"');
    expect(html).toContain('data-action="settings"');
    expect(html).toContain("Project settings");
    expect(html).toContain("Retired issues and worktrees");
  });

  it("escapes every item string", () => {
    const html = surfaceMenuHtml([{ id: '"><img src=x>', label: "<b>x</b>", description: "<i>d</i>" }]);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<i>d</i>");
    expect(html).toContain("&lt;img");
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
