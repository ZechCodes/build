// @vitest-environment jsdom
// The chooser a freshly minted worktree opens on: pick what runs here, and the
// tab becomes that.

import { describe, it, expect } from "vitest";
import { chooserTabHtml, mountChooserTab, NEW_TAB_KINDS } from "../src/core/surfaceTabs.js";

const mount = (onChoose) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  mountChooserTab(host, { onChoose });
  return { host, cards: () => [...host.querySelectorAll(".chooser-card")] };
};

describe("chooserTabHtml", () => {
  it("offers every new-tab kind as a card carrying its id", () => {
    const html = chooserTabHtml();
    for (const kind of NEW_TAB_KINDS) {
      expect(html).toContain(`data-kind="${kind.id}"`);
      expect(html).toContain(kind.label);
    }
  });

  it("escapes option strings", () => {
    const html = chooserTabHtml([{ id: "x", label: "<b>x</b>", description: '<img src=x>' }]);
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<img");
  });
});

describe("mountChooserTab", () => {
  it("reports the chosen kind", async () => {
    const picked = [];
    const ui = mount(async (kind) => picked.push(kind));
    ui.cards().find((card) => card.dataset.kind === "codex").click();
    await Promise.resolve();
    expect(picked).toEqual(["codex"]);
  });

  it("ignores a second pick while the first is in flight — one worktree, one session", async () => {
    let settle;
    const gate = new Promise((resolve) => (settle = resolve));
    const picked = [];
    const ui = mount((kind) => {
      picked.push(kind);
      return gate;
    });
    ui.cards()[0].click();
    ui.cards()[1].click();
    expect(picked).toEqual(["shell"]);
    expect(ui.host.classList.contains("chooser-busy")).toBe(true);
    settle();
  });

  it("re-arms when the pick fails, so another kind can be tried", async () => {
    const ui = mount(() => Promise.reject(new Error("terminal limit reached")));
    ui.cards()[0].click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(ui.host.classList.contains("chooser-busy")).toBe(false);
    const picked = [];
    mountChooserTab(ui.host, { onChoose: (kind) => picked.push(kind) });
    ui.cards()[1].click();
    expect(picked).toEqual(["claude"]);
  });
});
