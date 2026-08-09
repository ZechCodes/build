// The composer's action row. The reported defect: Send floated mid-row because
// the row reserved 150px of horizontal padding to dodge the New-issue FAB, and
// the paperclip sat orphaned at the far left. Actions belong together, at the
// right, and the FAB is cleared vertically so nothing has to move sideways.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { composerHtml } from "../src/core/composer.js";

const styles = readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8");
const markup = (over = {}) =>
  composerHtml({ inputId: "i", sendId: "s", hintId: "h", placeholder: "p", attachable: true, ...over });

describe("the composer action row", () => {
  it("groups the attach and send controls together", () => {
    const html = markup();
    const actions = html.match(/<div class="composer-actions">[\s\S]*?<\/div>/);
    expect(actions, "the actions share one container").not.toBeNull();
    expect(actions[0]).toContain('id="iattach"');
    expect(actions[0]).toContain('id="s"');
  });

  it("keeps the send label addressable so a busy state cannot wipe its icon", () => {
    expect(markup()).toContain('class="composer-send-label"');
  });

  it("trails the word with the arrow, the direction a send travels", () => {
    const html = markup();
    const button = html.slice(html.indexOf('class="btn primary composer-send"'));
    const label = button.indexOf("composer-send-label");
    const icon = button.indexOf("<svg");
    expect(label).toBeGreaterThan(-1);
    expect(icon).toBeGreaterThan(label);
    expect(button.slice(icon, icon + 200)).toContain("lucide-arrow-right");
  });

  it("still renders a plain box where there is no upload path", () => {
    const html = markup({ attachable: false });
    expect(html).not.toContain("composer-attach");
    expect(html).toContain('id="s"');
  });

  it("clears the create button vertically, never by stranding the actions", () => {
    expect(styles).not.toMatch(/\.composer-bar\s*\{[^}]*padding-right:\s*var\(--fab-clear\)/);
    expect(styles).toMatch(/\.composer-actions\s*\{[^}]*margin-left:\s*auto/);
  });
});
