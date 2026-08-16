import { describe, it, expect } from "vitest";
import { splitButtonMarkup, createSingleFlight } from "../src/core/splitButton.js";

const MERGE = { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: "commit, merge into main", busyLabel: "merging…" };
const KEEP = { id: "merge_keep", menuLabel: "Merge & keep worktree", description: "merge into main, keep the worktree", busyLabel: "merging…" };

describe("splitButtonMarkup", () => {
  it("makes options[0] the primary button, labeled by its label", () => {
    const html = splitButtonMarkup([MERGE, KEEP]);
    expect(html).toContain('class="splitbtn"');
    expect(html).toMatch(/<button class="btn primary"[^>]*data-action="merge_prune"[^>]*>Merge<\/button>/);
    expect(html).toContain('class="btn primary caret"');
    expect(html).toContain('class="splitmenu"');
    expect(html).toContain("hidden");
  });

  it("lists every option in the menu with its menuLabel and description", () => {
    const html = splitButtonMarkup([MERGE, KEEP]);
    // menuLabel wins over label for the menu item text
    expect(html).toContain('<span class="mt">Merge &amp; clean up</span>');
    expect(html).toContain('<span class="mt">Merge &amp; keep worktree</span>');
    expect(html).toContain('<span class="md">merge into main, keep the worktree</span>');
    // each menu item carries data-action
    expect(html).toContain('class="mi" data-action="merge_prune"');
    expect(html).toContain('class="mi" data-action="merge_keep"');
  });

  it("renders a single option as a plain button with no caret or menu", () => {
    const html = splitButtonMarkup([{ id: "abandon_delete", label: "Abandon", description: "d", busyLabel: "abandoning…" }]);
    expect(html).toContain('data-action="abandon_delete"');
    expect(html).toContain("Abandon");
    expect(html).not.toContain("caret");
    expect(html).not.toContain("splitmenu");
  });

  it("marks a danger option's MENU item, and never the button chrome", () => {
    // A primary painted with the destructive palette is two color systems on
    // one control — accent fill under red text. The cost is said in the
    // confirmation the button opens; the menu names which option carries it.
    const html = splitButtonMarkup([
      { id: "push", label: "Push", description: "push to the upstream", busyLabel: "…" },
      { id: "force_push", menuLabel: "Force push", description: "overwrite remote history", busyLabel: "…", danger: true },
    ]);
    expect(html).not.toContain("btn primary danger");
    expect(html).toContain('class="mi danger" data-action="force_push"');
    expect(html).toContain('class="mi" data-action="push"');
  });

  it("keeps a lone danger option's button plain — no menu to mark, nothing on the chrome", () => {
    const html = splitButtonMarkup([{ id: "x", label: "Delete", description: "d", busyLabel: "…", danger: true }]);
    expect(html).toContain('<button class="btn primary" data-action="x">Delete</button>');
    expect(html).not.toContain("danger");
  });

  it("paints the toolbar variant in the shared mini button, caret included", () => {
    const html = splitButtonMarkup([MERGE, KEEP], { variant: "mini" });
    expect(html).toMatch(/<button class="btn mini"[^>]*data-action="merge_prune"[^>]*>Merge<\/button>/);
    expect(html).toContain('class="btn mini caret"');
    expect(html).not.toContain("primary");
  });

  it("refuses a variant that is not part of the vocabulary", () => {
    expect(() => splitButtonMarkup([MERGE], { variant: "chunky" })).toThrow(/unknown split button variant: chunky/);
  });

  it("escapes labels, menuLabels, and descriptions", () => {
    const html = splitButtonMarkup([
      { id: "a", label: "<b>x</b>", description: "<script>alert(1)</script>", busyLabel: "…" },
      { id: "b", menuLabel: "<img src=x>", description: "plain", busyLabel: "…" },
    ]);
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img src=x>");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&lt;img src=x&gt;");
  });
});

describe("createSingleFlight", () => {
  it("begin() arms and returns true when idle", () => {
    const flight = createSingleFlight();
    expect(flight.begin()).toBe(true);
  });

  it("active() reflects the latch: false when idle, true in flight, false after end()", () => {
    const flight = createSingleFlight();
    expect(flight.active()).toBe(false);
    flight.begin();
    expect(flight.active()).toBe(true);
    flight.end();
    expect(flight.active()).toBe(false);
  });

  it("begin() returns false while already in flight", () => {
    const flight = createSingleFlight();
    flight.begin();
    expect(flight.begin()).toBe(false);
    expect(flight.begin()).toBe(false);
  });

  it("end() re-arms so begin() succeeds again", () => {
    const flight = createSingleFlight();
    flight.begin();
    flight.end();
    expect(flight.begin()).toBe(true);
  });

  it("instances are independent", () => {
    const a = createSingleFlight();
    const b = createSingleFlight();
    a.begin();
    expect(b.begin()).toBe(true);
  });
});
