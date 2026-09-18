import { describe, it, expect } from "vitest";
import { confirmModalHtml } from "../src/core/confirm.js";

describe("confirmModalHtml", () => {
  it("renders a dialog with title, confirm and cancel buttons", () => {
    const html = confirmModalHtml({ title: "Merge this run?" });
    expect(html).toContain('class="modal"');
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain("<h3>Merge this run?</h3>");
    expect(html).toContain("data-confirm-ok");
    expect(html).toContain("data-confirm-cancel");
    expect(html).toMatch(/<button class="btn" data-confirm-cancel>Cancel<\/button>/);
    expect(html).toMatch(/<button class="btn primary" data-confirm-ok>Confirm<\/button>/);
  });

  it("uses custom confirm and cancel labels", () => {
    const html = confirmModalHtml({ title: "t", confirmLabel: "Merge", cancelLabel: "Keep reviewing" });
    expect(html).toMatch(/data-confirm-ok>Merge<\/button>/);
    expect(html).toMatch(/data-confirm-cancel>Keep reviewing<\/button>/);
  });

  it("uses defaults when optional values are explicitly undefined", () => {
    const html = confirmModalHtml({ title: "t", actions: undefined, warnings: undefined, confirmLabel: undefined });
    expect(html).toContain("data-confirm-ok>Confirm</button>");
    expect(html).not.toContain("<ol");
    expect(html).not.toContain("<ul");
  });

  it("renders the intro sub line only when given", () => {
    expect(confirmModalHtml({ title: "t", intro: "This will:" })).toContain('<div class="sub">This will:</div>');
    expect(confirmModalHtml({ title: "t" })).not.toContain('class="sub"');
  });

  it("renders the action steps as an ordered list, in order", () => {
    const html = confirmModalHtml({ title: "t", actions: ["Commit changes", "Merge into main", "Delete branch"] });
    expect(html).toContain('<ol class="confirm-steps">');
    const commit = html.indexOf("Commit changes");
    const merge = html.indexOf("Merge into main");
    const del = html.indexOf("Delete branch");
    expect(commit).toBeGreaterThan(-1);
    expect(merge).toBeGreaterThan(commit);
    expect(del).toBeGreaterThan(merge);
    expect(html.match(/<li>/g)).toHaveLength(3);
  });

  // What a destructive verb is about to cost — the bridge's own `finish.warnings`
  // — is read BEFORE the outline of what will happen, because it is the part
  // that changes the answer.
  it("renders the warnings above the steps, and nothing when there are none", () => {
    const html = confirmModalHtml({
      title: "t",
      warnings: ["build/login has 3 commits that origin/build/login does not"],
      actions: ["Delete branch build/login"],
    });
    expect(html).toContain('<ul class="confirm-warnings">');
    expect(html).toContain("3 commits");
    expect(html.indexOf("confirm-warnings")).toBeLessThan(html.indexOf("confirm-steps"));
    expect(confirmModalHtml({ title: "t", warnings: [] })).not.toContain("confirm-warnings");
    expect(confirmModalHtml({ title: "t" })).not.toContain("confirm-warnings");
  });

  it("escapes what the warnings say", () => {
    expect(confirmModalHtml({ title: "t", warnings: ["<script>alert(1)</script>"] })).not.toContain("<script>");
  });

  it("omits the ordered list when actions is empty", () => {
    expect(confirmModalHtml({ title: "t" })).not.toContain("<ol");
    expect(confirmModalHtml({ title: "t", actions: [] })).not.toContain("<ol");
  });

  it("adds the danger class to the ok button only when danger", () => {
    expect(confirmModalHtml({ title: "t", danger: true })).toContain('class="btn primary danger" data-confirm-ok');
    expect(confirmModalHtml({ title: "t" })).not.toContain("danger");
  });

  it("escapes title, intro, actions, and labels", () => {
    const html = confirmModalHtml({
      title: "<b>x</b>",
      intro: "<i>y</i>",
      actions: ["<script>alert(1)</script>"],
      confirmLabel: '<img src="x">',
      cancelLabel: "a'b",
    });
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<i>y</i>");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain('<img src="x">');
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(html).toContain("&lt;i&gt;y&lt;/i&gt;");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&lt;img src=&quot;x&quot;&gt;");
    expect(html).toContain("a&#39;b");
  });
});
