// The composer's action row. The reported defect: Send floated mid-row because
// the row reserved 150px of horizontal padding to dodge the New-issue FAB, and
// the paperclip sat orphaned at the far left. Actions belong together, at the
// right, and the FAB is cleared vertically so nothing has to move sideways.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { composerHtml, composerPartIds, sendControlHtml } from "../src/core/composer.js";

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

// A message to a working agent can be handed over two ways — queued for its
// next step, or after stopping the turn it is running — so where both are
// possible the send is a split button. Where only one is, it is the button it
// has always been: a menu of one is not a choice.
describe("the send control", () => {
  it("is the plain button, unchanged, where there is nothing to interrupt", () => {
    const html = markup();
    expect(html).toContain('<button class="btn primary composer-send" id="s">');
    expect(html).not.toContain("splitbtn");
    expect(html).not.toContain("splitmenu");
  });

  it("is a split button offering the interrupting send where the turn can be stopped", () => {
    const html = markup({ canInterrupt: true });
    expect(html).toContain('class="splitbtn"');
    expect(html).toContain('<span class="mt">Interrupt &amp; send</span>');
    expect(html).toContain("Stop what the agent is doing now and hand it this message");
  });

  // The default press is the one it always was: queued, and delivered at the
  // agent's next step. Stopping the turn is the alternative you reach for.
  it("keeps the ordinary send as the default press, still addressable by id", () => {
    const html = markup({ canInterrupt: true });
    const primary = html.slice(html.indexOf('class="splitbtn"'), html.indexOf('class="btn primary caret"'));
    expect(primary).toContain('id="s"');
    expect(primary).toContain('data-action="send"');
    expect(primary).toContain(">Send<");
    expect(primary).not.toContain("interrupt");
  });

  // The poll swaps the control in place — the composer around it is never
  // rebuilt, because that would take the draft and the focus with it — so the
  // container it swaps into is named off the input, like the tray.
  it("sits in a container the poll can re-render into", () => {
    const parts = composerPartIds("i");
    expect(markup()).toContain(`id="${parts.sendControl}"`);
    expect(sendControlHtml({ sendId: "s", canInterrupt: false })).toContain('id="s"');
    expect(sendControlHtml({ sendId: "s", canInterrupt: true })).toContain("splitmenu");
  });
});
