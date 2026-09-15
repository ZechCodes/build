// @vitest-environment jsdom
// A checkout's two faces — what changed, and what is there — as a rail of icons
// down the left edge of the surface. It is the shell's own column (#dir-rail),
// not a row inside the commit list, so it stands at every width and a pane that
// remounts under it never takes it with it.
//
// Here is the markup and the behaviour; paneLayout.test.js holds the CSS half.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { DIRECTORY_TABS, directoryRailHtml, paintDirectoryRail } from "../src/core/directoryRail.js";

const mount = (options = {}) => {
  const host = document.createElement("nav");
  document.body.appendChild(host);
  const onSelect = vi.fn();
  paintDirectoryRail(host, { active: "changes", onSelect, ...options });
  return { host, onSelect, tabs: () => [...host.querySelectorAll("[data-tab]")] };
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
    expect(changes.getAttribute("title")).toBe("Changes");
    expect(changes.getAttribute("aria-label")).toBe("Changes");
    expect(files.getAttribute("aria-label")).toBe("Files");
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

  it("stands the host up as a vertical tablist", () => {
    const { host } = mount();
    expect(host.getAttribute("role")).toBe("tablist");
    expect(host.getAttribute("aria-orientation")).toBe("vertical");
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
