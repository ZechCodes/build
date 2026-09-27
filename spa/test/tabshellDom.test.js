// @vitest-environment jsdom
// DOM wiring for the tab row's `+`: it opens a menu of tab kinds (terminal,
// claude, codex) instead of silently minting a shell. The menu is mounted on
// document.body rather than inside the row, because the row scrolls
// (overflow-x) and would clip it.

import { describe, it, expect, beforeEach } from "vitest";
import { mountTabShell } from "../src/core/tabshell.js";

const TABS = [
  { id: "changes", label: "Changes" },
  { id: "term-1", label: "Terminal 1", closable: true },
];

const OPTIONS = [
  { id: "shell", label: "Terminal", description: "your shell" },
  { id: "claude", label: "Claude Code", description: "claude, interactive" },
  { id: "codex", label: "Codex", description: "codex, interactive" },
];

function mount(handlers = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const picked = [];
  const controller = mountTabShell(host, {
    tabs: TABS,
    active: "changes",
    newTabOptions: OPTIONS,
    onNewTab: (kind) => picked.push(kind),
    ...handlers,
  });
  return { host, controller, picked, plus: () => host.querySelector(".tplus") };
}

const menu = () => document.querySelector(".tabmenu");
const click = (element) => element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  document.body.innerHTML = "";
});

// A dropdown is how a control offers a CHOICE. With one thing to open, the `+`
// is that one thing: a menu of one item asks a question with a single answer.
describe("a + with one kind", () => {
  it("opens it directly instead of a one-item menu", () => {
    const shell = mount({ newTabOptions: [{ id: "shell", label: "Terminal", description: "your shell" }] });
    click(shell.plus());
    expect(menu()).toBeNull();
    expect(shell.picked).toEqual(["shell"]);
  });
});

describe("the + tab menu", () => {
  it("opens on click with one item per kind", () => {
    const shell = mount();
    expect(menu()).toBeNull();
    click(shell.plus());
    expect(menu()).not.toBeNull();
    expect([...menu().querySelectorAll(".mi")].map((mi) => mi.dataset.kind)).toEqual([
      "shell",
      "claude",
      "codex",
    ]);
  });

  it("reports the chosen kind and closes", () => {
    const shell = mount();
    click(shell.plus());
    click(menu().querySelector('[data-kind="codex"]'));
    expect(shell.picked).toEqual(["codex"]);
    expect(menu()).toBeNull();
  });

  it("closes on a pointerdown outside without choosing anything", () => {
    const shell = mount();
    click(shell.plus());
    document.body.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
    expect(menu()).toBeNull();
    expect(shell.picked).toEqual([]);
  });

  // A real pointer emits pointerdown BEFORE click. The outside-close listener is
  // live from the moment the menu opens (no arming tick — a fast trusted click
  // beats one), so the `+`'s own pointerdown must fall through to the `+`, or the
  // second click would close-then-reopen instead of toggling shut.
  it("toggles closed on a full pointer sequence over the +", () => {
    const shell = mount();
    click(shell.plus());
    shell.plus().dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
    expect(menu()).not.toBeNull();
    click(shell.plus());
    expect(menu()).toBeNull();
  });

  it("closes on Escape without choosing anything", () => {
    const shell = mount();
    click(shell.plus());
    document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(menu()).toBeNull();
    expect(shell.picked).toEqual([]);
  });

  it("closes when the row repaints, so no menu outlives its +", () => {
    const shell = mount();
    click(shell.plus());
    expect(menu()).not.toBeNull();
    shell.controller.setTabs([{ id: "changes", label: "Changes" }]);
    expect(menu()).toBeNull();
  });

  it("toggles closed when the + is clicked again", () => {
    const shell = mount();
    click(shell.plus());
    click(shell.plus());
    expect(menu()).toBeNull();
  });

  it("omits the + entirely when no handler is given", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    mountTabShell(host, { tabs: TABS, active: "changes", newTabOptions: OPTIONS });
    expect(host.querySelector(".tplus")).toBeNull();
  });

  it("still selects and closes tabs", () => {
    const selected = [];
    const closed = [];
    const shell = mount({ onSelect: (id) => selected.push(id), onClose: (id) => closed.push(id) });
    click(shell.host.querySelector('[data-tab="term-1"]'));
    expect(selected).toEqual(["term-1"]);
    click(shell.host.querySelector(".tx"));
    expect(closed).toEqual(["term-1"]);
  });
});

// The right cluster: icon tabs pinned to the row's end plus a ⋯ menu. One
// implementation here, mounted by every project surface.
const INBOX_ICON = '<svg class="lucide lucide-inbox"><circle cx="1" cy="1" r="1" /></svg>';
const RIGHT_TABS = [{ id: "inbox", icon: INBOX_ICON, label: "Inbox" }];
const MENU_ITEMS = [
  { id: "archive", label: "Archive", description: "Retired tasks and worktrees" },
  { id: "settings", label: "Project settings", description: "Name, path, base branch" },
];

function mountWithCluster(handlers = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const selected = [];
  const picked = [];
  const controller = mountTabShell(host, {
    tabs: TABS,
    active: "changes",
    rightTabs: RIGHT_TABS,
    menu: MENU_ITEMS,
    onSelect: (id) => selected.push(id),
    onMenuPick: (id) => picked.push(id),
    ...handlers,
  });
  return { host, controller, selected, picked, dots: () => host.querySelector(".tmenu") };
}

describe("the ⋯ surface menu", () => {
  it("opens on click with one button per action", () => {
    const shell = mountWithCluster();
    expect(menu()).toBeNull();
    click(shell.dots());
    expect(menu()).not.toBeNull();
    const items = [...menu().querySelectorAll(".mi")];
    expect(items.map((item) => item.tagName)).toEqual(["BUTTON", "BUTTON"]);
    expect(items.map((item) => item.dataset.action)).toEqual(["archive", "settings"]);
  });

  it("reports the chosen action and closes", () => {
    const shell = mountWithCluster();
    click(shell.dots());
    click(menu().querySelector('[data-action="archive"]'));
    expect(shell.picked).toEqual(["archive"]);
    expect(menu()).toBeNull();
  });

  it("closes on Escape and on a pointerdown outside, choosing nothing", () => {
    const shell = mountWithCluster();
    click(shell.dots());
    document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(menu()).toBeNull();
    click(shell.dots());
    document.body.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
    expect(menu()).toBeNull();
    expect(shell.picked).toEqual([]);
  });

  it("closes when the row repaints", () => {
    const shell = mountWithCluster();
    click(shell.dots());
    shell.controller.setActive("term-1");
    expect(menu()).toBeNull();
  });

  it("carries the active state for the tab state it opened", () => {
    const shell = mountWithCluster();
    shell.controller.setActive("archive");
    expect(shell.dots().classList.contains("active")).toBe(true);
    shell.controller.setActive("changes");
    expect(shell.dots().classList.contains("active")).toBe(false);
  });

  it("is absent when the surface passes no menu", () => {
    const shell = mountWithCluster({ menu: [], onMenuPick: undefined });
    expect(shell.dots()).toBeNull();
  });
});

describe("icon tabs in the right cluster", () => {
  it("select through the same onSelect as any other tab", () => {
    const shell = mountWithCluster();
    click(shell.host.querySelector('.ticon[data-tab="inbox"]'));
    expect(shell.selected).toEqual(["inbox"]);
  });

  it("show the active state when the surface selects them", () => {
    const shell = mountWithCluster();
    shell.controller.setActive("inbox");
    expect(shell.host.querySelector('[data-tab="inbox"]').classList.contains("active")).toBe(true);
    expect(shell.host.querySelector('[data-tab="changes"]').classList.contains("active")).toBe(false);
  });

  // A recognisable icon carries the cell; the label is what a screen reader and
  // a hover both get, so nothing about the tab is lost by dropping the text.
  it("draw a real SVG, with the label as tooltip and accessible name", () => {
    const shell = mountWithCluster();
    const cell = shell.host.querySelector('.ticon[data-tab="inbox"]');
    expect(cell.querySelector("svg")).toBeTruthy();
    expect(cell.textContent.trim()).toBe("");
    expect(cell.getAttribute("title")).toBe("Inbox");
    expect(cell.getAttribute("aria-label")).toBe("Inbox");
    expect(shell.dots().querySelector("svg")).toBeTruthy();
    expect(shell.dots().textContent.trim()).toBe("");
  });
});
