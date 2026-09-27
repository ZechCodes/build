// @vitest-environment jsdom
// A checkout's two faces — what changed, and what is there — as a rail of icons
// down the left edge of the surface. It is the shell's own column (#dir-rail),
// not a row inside the commit list, so it stands at every width and a pane that
// remounts under it never takes it with it.
//
// Here is the markup and the behaviour; paneLayout.test.js holds the CSS half.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  DIRECTORY_TABS,
  SIDEBAR_COLLAPSED_KEY,
  WORKSPACE_TABS,
  directoryRailHtml,
  paintDirectoryRail,
} from "../src/core/directoryRail.js";

const mount = (options = {}) => {
  const host = document.createElement("nav");
  document.body.appendChild(host);
  const onSelect = vi.fn();
  paintDirectoryRail(host, { active: "changes", onSelect, ...options });
  return {
    host,
    onSelect,
    tabs: () => [...host.querySelectorAll("[data-tab]")],
    toggle: () => host.querySelector("[data-sidebar-toggle]"),
  };
};

const press = (element, key) => element.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true }));

describe("the directory rail's markup", () => {
  it("offers what changed before what is there, each as an icon and nothing else", () => {
    expect(DIRECTORY_TABS.map((tab) => tab.id)).toEqual(["changes", "files"]);
    const host = document.createElement("nav");
    host.innerHTML = directoryRailHtml(DIRECTORY_TABS, "changes");
    const [changes, files] = [...host.querySelectorAll("[data-tab]")];
    // An icon-only rail: the words are the tooltip and the accessible name, and
    // no label is written beside the glyph at any width.
    expect(changes.textContent.trim()).toBe("");
    expect(changes.querySelector("svg")).toBeTruthy();
    expect(changes.innerHTML).toContain("git-graph");
    expect(changes.getAttribute("title")).toBe("Changes");
    expect(changes.getAttribute("aria-label")).toBe("Changes");
    expect(files.getAttribute("aria-label")).toBe("Files");
    expect(files.innerHTML).toContain('class="lucide lucide-folder"');
  });

  it("says which face the surface is standing on, to the eye and to the reader", () => {
    const host = document.createElement("nav");
    host.innerHTML = directoryRailHtml(DIRECTORY_TABS, "files");
    const [changes, files] = [...host.querySelectorAll("[data-tab]")];
    expect(files.classList.contains("active")).toBe(true);
    expect(files.getAttribute("aria-selected")).toBe("true");
    expect(changes.getAttribute("aria-selected")).toBe("false");
    // One stop on the rail, not one per tab: the arrows move within it.
    expect(files.getAttribute("tabindex")).toBe("0");
    expect(changes.getAttribute("tabindex")).toBe("-1");
  });
});

describe("the directory rail", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("stands the faces up as a vertical tablist", () => {
    const { host, tabs } = mount();
    const list = host.querySelector("[role='tablist']");
    expect(list.getAttribute("aria-orientation")).toBe("vertical");
    // The tablist owns the tabs and nothing else: the sidebar toggle beside it
    // is a button of the rail, not a cell of the list.
    expect([...list.children]).toEqual(tabs());
  });

  it("reports the face that was pressed", () => {
    const { onSelect, tabs } = mount();
    tabs()[1].click();
    expect(onSelect).toHaveBeenCalledWith("files");
  });

  it("walks the rail with the arrows, wrapping at either end", () => {
    const { onSelect, tabs } = mount();
    tabs()[0].focus();
    press(tabs()[0], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("files");
    expect(document.activeElement).toBe(tabs()[1]);
    press(tabs()[1], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
    press(tabs()[0], "ArrowUp");
    expect(onSelect).toHaveBeenLastCalledWith("files");
  });

  it("jumps to either end of the rail", () => {
    const { onSelect, tabs } = mount();
    tabs()[0].focus();
    press(tabs()[0], "End");
    expect(onSelect).toHaveBeenLastCalledWith("files");
    press(tabs()[1], "Home");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
  });

  it("leaves a key it does not answer for to the page", () => {
    const { onSelect, tabs } = mount();
    press(tabs()[0], "ArrowRight");
    expect(onSelect).not.toHaveBeenCalled();
  });

  // A press changes the route, and the surface answers by painting the rail
  // again for the new face. That paint rewrites the cells, so the one the
  // keyboard was standing on is gone; the keyboard has to be handed the cell
  // that took its place, or the second arrow lands on nothing (measured in the
  // browser: ArrowDown worked once, ArrowUp then did nothing).
  it("keeps the keyboard on the rail when a press repaints it", () => {
    const host = document.createElement("nav");
    document.body.appendChild(host);
    const onSelect = vi.fn((tab) => paintDirectoryRail(host, { active: tab, onSelect }));
    paintDirectoryRail(host, { active: "changes", onSelect });
    host.querySelector("[data-tab=changes]").focus();
    press(document.activeElement, "ArrowDown");
    expect(document.activeElement).toBe(host.querySelector("[data-tab=files]"));
    press(document.activeElement, "ArrowUp");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
    expect(document.activeElement).toBe(host.querySelector("[data-tab=changes]"));
  });

  it("repaints in place, with only the faces the directory has", () => {
    const { host, onSelect } = mount();
    paintDirectoryRail(host, { tabs: DIRECTORY_TABS.filter((tab) => tab.id === "files"), active: "files", onSelect });
    const tabs = [...host.querySelectorAll("[data-tab]")];
    expect(tabs.map((tab) => tab.dataset.tab)).toEqual(["files"]);
    tabs[0].click();
    expect(onSelect).toHaveBeenCalledWith("files");
  });
});

// The list column beside the rail — the file tree, the commit rail — folds away
// from one control at the rail's foot, so the detail can take the whole width.
// One state for the rail, whichever face is showing, kept in this browser.
describe("the sidebar toggle", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    localStorage.clear();
  });

  it("stands at the foot of the rail, a button outside the tablist, reachable by Tab", () => {
    const { host, toggle } = mount();
    const button = toggle();
    expect(host.lastElementChild).toBe(button);
    expect(button.tagName).toBe("BUTTON");
    expect(button.getAttribute("type")).toBe("button");
    expect(button.hasAttribute("role")).toBe(false);
    expect(button.hasAttribute("tabindex")).toBe(false);
    expect(button.closest("[role='tablist']")).toBeNull();
    // Icon-only, like the faces above it: the words are its name and tooltip.
    expect(button.textContent.trim()).toBe("");
    expect(button.querySelector("svg")).toBeTruthy();
  });

  it("starts expanded, and says what pressing it does", () => {
    const { host, toggle } = mount();
    expect(host.dataset.sidebar).toBe("expanded");
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(toggle().getAttribute("aria-label")).toBe("Collapse sidebar");
    expect(toggle().getAttribute("title")).toBe("Collapse sidebar");
    expect(toggle().innerHTML).toContain("panel-left-close");
  });

  it("collapses and expands the sidebar, keeping the control's words in step", () => {
    const { host, toggle } = mount();
    toggle().click();
    expect(host.dataset.sidebar).toBe("collapsed");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(toggle().getAttribute("aria-label")).toBe("Expand sidebar");
    expect(toggle().getAttribute("title")).toBe("Expand sidebar");
    expect(toggle().innerHTML).toContain("panel-left-open");
    toggle().click();
    expect(host.dataset.sidebar).toBe("expanded");
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(toggle().getAttribute("aria-label")).toBe("Collapse sidebar");
  });

  it("keeps the keyboard on the control it pressed", () => {
    const { toggle } = mount();
    toggle().focus();
    toggle().click();
    expect(document.activeElement).toBe(toggle());
  });

  it("changes no face when it is pressed", () => {
    const { onSelect, toggle } = mount();
    toggle().click();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("is not in the arrow ring: the arrows walk the faces and never land on it", () => {
    const { onSelect, tabs, toggle } = mount({ active: "files" });
    tabs()[1].focus();
    press(tabs()[1], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
    expect(document.activeElement).toBe(tabs()[0]);
    press(tabs()[0], "End");
    expect(document.activeElement).toBe(tabs()[1]);
    onSelect.mockClear();
    toggle().focus();
    press(toggle(), "ArrowUp");
    press(toggle(), "Home");
    expect(onSelect).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(toggle());
  });

  it("remembers the choice in this browser, under one key", () => {
    const { toggle } = mount();
    toggle().click();
    expect(localStorage.getItem(SIDEBAR_COLLAPSED_KEY)).toBe("true");
    toggle().click();
    expect(localStorage.getItem(SIDEBAR_COLLAPSED_KEY)).toBe("false");
    expect(localStorage.length).toBe(1);
  });

  it("paints the remembered choice, on a new rail and on a repaint for the other face", () => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, "true");
    const { host, onSelect, toggle } = mount();
    expect(host.dataset.sidebar).toBe("collapsed");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    toggle().click();
    // One state per rail, not per face: switching faces repaints the rail and
    // the choice stands.
    paintDirectoryRail(host, { active: "files", onSelect });
    expect(host.dataset.sidebar).toBe("expanded");
    expect(toggle().getAttribute("aria-label")).toBe("Collapse sidebar");
    toggle().click();
    paintDirectoryRail(host, { active: "changes", onSelect });
    expect(host.dataset.sidebar).toBe("collapsed");
    expect(toggle().getAttribute("aria-label")).toBe("Expand sidebar");
  });

  it("keeps the keyboard on the control across a repaint of the rail", () => {
    const { host, onSelect, toggle } = mount();
    toggle().focus();
    paintDirectoryRail(host, { active: "files", onSelect });
    expect(document.activeElement).toBe(toggle());
  });

  it("paints both faces and an expanded toggle when the browser denies the store itself", () => {
    // A denied origin throws on READING localStorage, before there is an
    // object whose getItem could be guarded (SecurityError).
    const own = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new DOMException("The operation is insecure.", "SecurityError");
      },
    });
    try {
      const { host, tabs, toggle } = mount();
      expect(tabs().map((tab) => tab.dataset.tab)).toEqual(["changes", "files"]);
      expect(host.dataset.sidebar).toBe("expanded");
      expect(toggle().getAttribute("aria-expanded")).toBe("true");
      toggle().click();
      expect(host.dataset.sidebar).toBe("collapsed");
    } finally {
      if (own) Object.defineProperty(globalThis, "localStorage", own);
      else delete globalThis.localStorage;
    }
  });

  it("works for the mount when this browser keeps nothing", () => {
    const blocked = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    const { host, toggle } = mount({ storage: blocked });
    expect(host.dataset.sidebar).toBe("expanded");
    toggle().click();
    expect(host.dataset.sidebar).toBe("collapsed");
  });
});

// #174: "Tasks moved to the rail as an icon along with the settings. Making the
// left rail the workspace navigation." A workspace's rail is Changes, Files and
// Tasks, with Settings at its foot above the sidebar toggle.
describe("the workspace's rail", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    localStorage.clear();
  });

  const mountWorkspace = (options = {}) => {
    const onOpen = vi.fn();
    const mounted = mount({ tabs: WORKSPACE_TABS, settings: { onOpen }, ...options });
    return { ...mounted, onOpen, settings: () => mounted.host.querySelector("[data-rail-settings]") };
  };

  it("lists Changes, Files and Tasks, Tasks as an icon wearing a count bubble", () => {
    expect(WORKSPACE_TABS.map((tab) => tab.id)).toEqual(["changes", "files", "tasks"]);
    const { tabs } = mountWorkspace();
    const tasks = tabs()[2];
    expect(tasks.getAttribute("aria-label")).toBe("Tasks");
    expect(tasks.innerHTML).toContain("lucide-square-check");
    expect(tasks.innerHTML).not.toContain("circle");
    expect(tasks.querySelector(".badge.dirtab-count")).not.toBeNull();
    // Changes and Files carry no count.
    expect(tabs()[0].querySelector(".badge")).toBeNull();
  });

  it("stands Settings at the foot, outside the tablist, above the sidebar toggle", () => {
    const { host, tabs, settings, toggle } = mountWorkspace();
    expect([...host.querySelector("[role='tablist']").children]).toEqual(tabs());
    expect(settings().getAttribute("aria-label")).toBe("Workspace settings");
    expect(settings().innerHTML).toContain("lucide-settings");
    expect([...host.children].slice(1)).toEqual([settings(), toggle()]);
  });

  it("opens the settings from its cog, and changes no face doing it", () => {
    const { onOpen, onSelect, settings } = mountWorkspace();
    settings().click();
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("walks Changes, Files and Tasks with the arrows, and never lands on Settings", () => {
    const { onSelect, tabs } = mountWorkspace();
    tabs()[1].focus();
    press(tabs()[1], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("tasks");
    press(tabs()[2], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
  });

  // A bridge that carries no tasks hides the Tasks face; the arrows walk the
  // faces that are drawn.
  it("skips a hidden face", () => {
    const { onSelect, tabs } = mountWorkspace();
    tabs()[2].hidden = true;
    tabs()[1].focus();
    press(tabs()[1], "ArrowDown");
    expect(onSelect).toHaveBeenLastCalledWith("changes");
    press(tabs()[0], "End");
    expect(onSelect).toHaveBeenLastCalledWith("files");
  });

  it("marks Tasks as the face the workspace is standing on", () => {
    const { tabs } = mountWorkspace({ active: "tasks" });
    expect(tabs().map((tab) => tab.getAttribute("aria-selected"))).toEqual(["false", "false", "true"]);
  });

  it("is drawn with no Settings where the surface names none, as the branch checkout does", () => {
    const { host } = mount();
    expect(host.querySelector("[data-rail-settings]")).toBeNull();
  });

  // A CSS fact jsdom cannot see: `.dirtab` sets display, which beats the UA's
  // [hidden] rule, so a hidden face needs a rule of its own.
  it("hides a hidden face by its own rule, because the cell sets display", () => {
    const css = readFileSync(resolve("src/styles/shell.css"), "utf8");
    expect(css).toMatch(/\.dirtab \{[^}]*display:flex/);
    expect(css).toMatch(/\.dirtab\[hidden\] \{[^}]*display:none/);
    expect(css).toMatch(/\.dirtab-count:empty \{[^}]*display:none/);
  });
});
