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

  it("closes on a pointerdown outside without choosing anything", async () => {
    const shell = mount();
    click(shell.plus());
    await tick(); // the outside-close listener arms on the next tick
    document.body.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
    expect(menu()).toBeNull();
    expect(shell.picked).toEqual([]);
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
