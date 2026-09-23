/** @vitest-environment jsdom */
// One filter menu, mounted (#44): opening, searching, ticking, the keyboard,
// and the one rule the whole bar is built on — the control is made once and a
// paint never touches what the reader put in it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountFilterMenu } from "../src/core/filterMenuControl.js";
import { uiAddress } from "../src/core/localUiState.js";

const LABELS = [
  { value: "", label: "Any label" },
  { value: "bug", label: "bug" },
  { value: "tracker", label: "tracker" },
  { value: "transport", label: "transport" },
];

const ASSIGNEES = [
  { value: "", label: "Anyone" },
  { value: "none", label: "Unassigned" },
  { value: "agent:a1", label: "issues-spa · Agent 1", group: "issues-spa" },
  { value: "agent:b1", label: "tracker-filters · Agent 1", group: "tracker-filters" },
];

let host, menu, changed;

const mount = (over = {}) => {
  changed = vi.fn();
  menu = mountFilterMenu(host, { name: "label", label: "Labels", multi: true, summary: "count", onChange: changed, ...over });
  return menu;
};

const press = () => host.querySelector(".fmenu-press");
const pressText = () => press().textContent.trim();
const pop = () => host.querySelector(".fmenu-pop");
const search = () => host.querySelector(".fmenu-search");
const rows = () => [...host.querySelectorAll(".fmenu-row")];
const rowValues = () => rows().map((row) => row.dataset.value);
const activeRow = () => host.querySelector(".fmenu-row.is-active");
const key = (name) => search().dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
const type = (text) => {
  search().value = text;
  search().dispatchEvent(new Event("input"));
};

beforeEach(() => {
  document.body.innerHTML = '<div id="bar"></div>';
  host = document.querySelector("#bar");
});

afterEach(() => menu?.dispose());

describe("the press", () => {
  it("says the empty row's words, then what is chosen", () => {
    mount().update(LABELS, []);
    expect(pressText()).toBe("Any label");
    menu.update(LABELS, ["bug"]);
    expect(pressText()).toBe("bug");
    menu.update(LABELS, ["bug", "tracker"]);
    expect(pressText()).toBe("Labels · 2");
  });

  it("opens and shuts the popover, and says which it is", () => {
    mount().update(LABELS, []);
    expect(pop().hidden).toBe(true);
    expect(press().getAttribute("aria-expanded")).toBe("false");
    press().click();
    expect(pop().hidden).toBe(false);
    expect(press().getAttribute("aria-expanded")).toBe("true");
    press().click();
    expect(pop().hidden).toBe(true);
  });

  it("opens on the down arrow, so the keyboard reaches it the way the menu reads", () => {
    mount().update(LABELS, []);
    press().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    expect(pop().hidden).toBe(false);
  });

  it("puts the focus in the search box on opening", () => {
    mount().update(LABELS, []);
    press().click();
    expect(document.activeElement).toBe(search());
  });
});

describe("searching", () => {
  it("changes cached result rows only after the query readback", async () => {
    globalThis.indexedDB = new IDBFactory();
    globalThis.IDBKeyRange = IDBKeyRange;
    mount({ cacheAddress: uiAddress({ view: "filter", kind: "menu", sub: "label" }) }).update(LABELS, []);
    press().click();
    await vi.waitFor(() => expect(pop().hidden).toBe(false));
    const before = rowValues();
    type("tr");
    expect(rowValues()).toEqual(before);
    await vi.waitFor(() => expect(rowValues()).toEqual(["tracker", "transport"]));
  });
  it("leaves what the query ranks, best first", () => {
    mount().update(LABELS, []);
    press().click();
    expect(rowValues()).toEqual(["bug", "tracker", "transport"]);
    type("tr");
    expect(rowValues()).toEqual(["tracker", "transport"]);
  });

  it("says so when nothing matches, rather than showing an empty box", () => {
    mount().update(LABELS, []);
    press().click();
    type("zzz");
    expect(rows()).toHaveLength(0);
    expect(host.querySelector(".fmenu-none").hidden).toBe(false);
  });

  it("keeps the row that survives a keystroke as the same element", () => {
    mount().update(LABELS, []);
    press().click();
    const kept = rows().find((row) => row.dataset.value === "tracker");
    type("tr");
    expect(rows().find((row) => row.dataset.value === "tracker")).toBe(kept);
  });

  it("draws the workspace headings an assignee menu groups by", () => {
    mount({ name: "assignee", label: "Assignee", summary: "first" }).update(ASSIGNEES, []);
    press().click();
    expect([...pop().querySelectorAll(".fmenu-group")].map((one) => one.textContent))
      .toEqual(["issues-spa", "tracker-filters"]);
  });
});

describe("ticking", () => {
  it("adds and removes, saying the whole selection each time", () => {
    mount().update(LABELS, []);
    press().click();
    rows()[0].click();
    expect(changed).toHaveBeenLastCalledWith(["bug"]);
    rows()[1].click();
    expect(changed).toHaveBeenLastCalledWith(["bug", "tracker"]);
    rows()[0].click();
    expect(changed).toHaveBeenLastCalledWith(["tracker"]);
  });

  it("marks what is ticked where a screen reader can read it", () => {
    mount().update(LABELS, []);
    press().click();
    rows()[0].click();
    expect(rows()[0].getAttribute("aria-selected")).toBe("true");
    expect(rows()[1].getAttribute("aria-selected")).toBe("false");
    expect(host.querySelector(".fmenu-rows").getAttribute("aria-multiselectable")).toBe("true");
  });

  // Ticking three labels should be one opening, not three.
  it("stays open through a multi menu's ticks and shuts after a single one's", () => {
    mount().update(LABELS, []);
    press().click();
    rows()[0].click();
    expect(pop().hidden).toBe(false);

    menu.dispose();
    host.innerHTML = "";
    mount({ name: "state", label: "State", multi: false }).update(
      [{ value: "", label: "Open and closed" }, { value: "open", label: "Open" }], [],
    );
    press().click();
    rows()[1].click();
    expect(changed).toHaveBeenLastCalledWith(["open"]);
    expect(pop().hidden).toBe(true);
  });

  it("clears from inside, and shuts", () => {
    mount().update(LABELS, ["bug", "tracker"]);
    press().click();
    host.querySelector(".fmenu-clear").click();
    expect(changed).toHaveBeenLastCalledWith([]);
    expect(pressText()).toBe("Any label");
    expect(pop().hidden).toBe(true);
  });

  it("offers no Clear while there is nothing to clear", () => {
    mount().update(LABELS, []);
    expect(host.querySelector(".fmenu-clear").disabled).toBe(true);
    menu.update(LABELS, ["bug"]);
    expect(host.querySelector(".fmenu-clear").disabled).toBe(false);
  });
});

describe("the keyboard", () => {
  // The first row is walked to the moment the menu opens, so Enter answers
  // without a step — which is what makes typing and choosing one gesture.
  it("walks the rows from the first one, stepping over the headings", () => {
    mount({ name: "assignee", label: "Assignee" }).update(ASSIGNEES, []);
    press().click();
    expect(activeRow().dataset.value).toBe("none");
    key("ArrowDown");
    expect(activeRow().dataset.value).toBe("agent:a1");
    key("ArrowDown");
    expect(activeRow().dataset.value).toBe("agent:b1");
    key("ArrowUp");
    expect(activeRow().dataset.value).toBe("agent:a1");
  });

  it("walks back to the best match after a search", () => {
    mount().update(LABELS, []);
    press().click();
    key("ArrowDown");
    type("tr");
    expect(activeRow().dataset.value).toBe("tracker");
  });

  it("names the walked row for a screen reader, without taking the focus", () => {
    mount().update(LABELS, []);
    press().click();
    key("ArrowDown");
    expect(search().getAttribute("aria-activedescendant")).toBe(activeRow().id);
    expect(document.activeElement).toBe(search());
  });

  it("toggles the walked row on enter", () => {
    mount().update(LABELS, []);
    press().click();
    key("Enter");
    expect(changed).toHaveBeenLastCalledWith(["bug"]);
    key("ArrowDown");
    key("Enter");
    expect(changed).toHaveBeenLastCalledWith(["bug", "tracker"]);
    key("Enter");
    expect(changed).toHaveBeenLastCalledWith(["bug"]);
  });

  it("shuts on escape and gives the press back its focus", () => {
    mount().update(LABELS, []);
    press().click();
    key("Escape");
    expect(pop().hidden).toBe(true);
    expect(document.activeElement).toBe(press());
  });

  it("lets tab out rather than trapping the reader", () => {
    mount().update(LABELS, []);
    press().click();
    const event = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    search().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(pop().hidden).toBe(true);
  });
});

describe("mounted once", () => {
  // The whole reason this control can exist: what the reader put into it lives
  // in the DOM, and `update` is told what is on offer — never what is open,
  // typed or walked.
  it("keeps the popover open, the query typed and the row walked across ten updates", () => {
    mount().update(LABELS, []);
    press().click();
    type("tr");
    key("ArrowDown");
    const walked = activeRow();
    const theSearch = search();
    const thePress = press();
    for (let i = 0; i < 10; i += 1) menu.update([...LABELS, { value: `new-${i}`, label: `new-${i}` }], []);
    expect(press()).toBe(thePress);
    expect(search()).toBe(theSearch);
    expect(search().value).toBe("tr");
    expect(pop().hidden).toBe(false);
    expect(activeRow()).toBe(walked);
    expect(document.activeElement).toBe(theSearch);
  });

  it("shows a newly offered row without re-making the ones already there", () => {
    mount().update(LABELS, []);
    press().click();
    const kept = rows()[0];
    menu.update([...LABELS, { value: "perf", label: "perf" }], []);
    expect(rowValues()).toEqual(["bug", "tracker", "transport", "perf"]);
    expect(rows()[0]).toBe(kept);
  });

  it("shuts when the reader presses somewhere else", () => {
    mount().update(LABELS, []);
    press().click();
    document.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(pop().hidden).toBe(true);
  });
});
