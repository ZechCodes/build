// The composer's action row. The reported defect: Send floated mid-row because
// the row reserved 150px of horizontal padding to dodge the New-issue FAB, and
// the paperclip sat orphaned at the far left. Actions belong together, at the
// right, and the FAB is cleared vertically so nothing has to move sideways.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { composerHtml, composerPartIds, sendControlHtml } from "../src/core/composer.js";

const styles = ["../src/styles.css", "../src/styles/shell.css"]
  .map((path) => readFileSync(fileURLToPath(new URL(path, import.meta.url)), "utf8"))
  .join("\n");
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

  it("keeps contextual rows above the bordered input and omits visual shortcut and menu arrows", () => {
    const html = markup({ modelMenu: true });
    expect(html.indexOf("composer-context")).toBeLessThan(html.indexOf('class="composer attachable"'));
    expect(html.indexOf("composer-tray")).toBeLessThan(html.indexOf('class="composer attachable"'));
    expect(html).not.toContain("composer-shortcut");
  });

  it("renders send as an icon-only arrow", () => {
    const html = markup();
    const button = html.slice(html.indexOf('class="btn primary composer-send"'));
    expect(button.slice(0, 400)).toContain("lucide-arrow-right");
    expect(button.slice(0, 400)).toContain('aria-label="Send message"');
    expect(button.slice(0, 400)).not.toContain(">Send<");
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

  it("lets the rail footer overlay the transcript and reserves its measured height in the scroller", () => {
    expect(styles).toMatch(/\.rail-footer\s*\{[^}]*position:absolute/);
    expect(styles).toMatch(/\.rail-body\s*\{[^}]*--rail-composer-clearance/);
  });

  it("keeps the toolbar reachable when a short viewport constrains the floating footer", () => {
    expect(styles).toMatch(/\.rail-footer\s*\{[^}]*display:flex[^}]*flex-direction:column/);
    expect(styles).toMatch(/\.rail-composer\s*\{[^}]*min-height:0[^}]*display:flex[^}]*flex-direction:column/);
    expect(styles).toMatch(/\.rail-composer \.thread-composer\s*\{[^}]*min-height:0[^}]*display:flex/);
    expect(styles).toMatch(/\.rail-composer \.composer\s*\{[^}]*min-height:0[^}]*display:flex/);
    expect(styles).toMatch(/\.rail-composer \.composer-bar\s*\{[^}]*flex:none/);
    expect(styles).toMatch(/\.rail-composer \.composer textarea\s*\{[^}]*flex:1 1 auto[^}]*overflow-y:auto/);
  });
});

// The action changes with the turn and draft: stop for an interruptible active
// turn with no draft, otherwise send.
describe("the send control", () => {
  it("is the plain button, unchanged, where there is nothing to interrupt", () => {
    const html = markup();
    expect(html).toContain('class="btn primary composer-send" id="s"');
    expect(html).not.toContain("splitbtn");
    expect(html).not.toContain("splitmenu");
  });

  it("is a stop square when a turn can be stopped and the draft is empty", () => {
    const html = markup({ canInterrupt: true });
    expect(html).toContain('data-action="stop"');
    expect(html).toContain('aria-label="Stop agent"');
    expect(html).toContain("lucide-square");
  });

  // The default press is the one it always was: queued, and delivered at the
  // agent's next step. Stopping the turn is the alternative you reach for.
  it("shows send while a working agent has a draft", () => {
    const html = sendControlHtml({ sendId: "s", canInterrupt: true, hasDraft: true });
    expect(html).toContain('data-action="send"');
    expect(html).toContain("lucide-arrow-right");
  });

  // The poll swaps the control in place — the composer around it is never
  // rebuilt, because that would take the draft and the focus with it — so the
  // container it swaps into is named off the input, like the tray.
  it("sits in a container the poll can re-render into", () => {
    const parts = composerPartIds("i");
    expect(markup()).toContain(`id="${parts.sendControl}"`);
    expect(sendControlHtml({ sendId: "s", canInterrupt: false })).toContain('id="s"');
    expect(sendControlHtml({ sendId: "s", canInterrupt: true })).toContain('data-action="stop"');
  });
});
