// @vitest-environment jsdom
// A split menu in sections, and a split menu driven from the keyboard.
//
// The conversation head's ⋮ (#124) holds two different kinds of row — what
// the agent opened, and the conversation's settings — so the menu learned
// groups: each headed by what it holds, a setting's rows a radio set. And a
// menu a screen reader can tell apart is a menu the keyboard has to reach.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { groupedMenuButtonMarkup, menuButtonMarkup, mountMenuIfChanged, mountSplitMenu } from "../src/core/splitButton.js";
import { motionBeat } from "./motionRecorder.js";

const SHELLS = { id: "shells", label: "Shells", description: "1 running" };
const GROUPS = [
  { id: "show", label: "Show", options: [SHELLS, { id: "checklist", label: "Tasks", description: "1/3 completed" }] },
  {
    id: "detail",
    label: "Detail",
    options: [
      { id: "detail:all", label: "All", description: "Everything", selected: true },
      { id: "detail:agent", label: "Agent only", description: "Less", selected: false },
    ],
  },
];

const keydown = (target, key) => {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
};

describe("groupedMenuButtonMarkup", () => {
  const mounted = () => {
    document.body.innerHTML = groupedMenuButtonMarkup("⋮", GROUPS, { title: "Conversation menu", icon: true });
    return document.body;
  };

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("writes one labelled group per section, in order, with its rows inside", () => {
    const host = mounted();
    const groups = [...host.querySelectorAll('.splitmenu > [role="group"]')];

    expect(host.querySelector(".splitmenu").getAttribute("role")).toBe("menu");
    expect(host.querySelector(".splitmenu").getAttribute("aria-label")).toBe("Conversation menu");
    expect(groups.map((group) => group.getAttribute("aria-label"))).toEqual(["Show", "Detail"]);
    expect(groups.map((group) => group.dataset.group)).toEqual(["show", "detail"]);
    expect(groups.map((group) => [...group.querySelectorAll(".mi")].map((row) => row.dataset.action))).toEqual([
      ["shells", "checklist"],
      ["detail:all", "detail:agent"],
    ]);
  });

  it("heads each group with its name, once for the eye and once for the reader", () => {
    const host = mounted();
    const titles = [...host.querySelectorAll(".menu-group-title")];

    expect(titles.map((title) => title.textContent)).toEqual(["Show", "Detail"]);
    // The group carries the name (aria-label), so the visible heading is not
    // read a second time.
    expect(titles.every((title) => title.getAttribute("aria-hidden") === "true")).toBe(true);
  });

  it("gives a row that carries an answer radio semantics, and the rest plain item semantics", () => {
    const host = mounted();
    const row = (action) => host.querySelector(`.mi[data-action="${action}"]`);

    expect(row("shells").getAttribute("role")).toBe("menuitem");
    expect(row("shells").hasAttribute("aria-checked")).toBe(false);
    expect(row("detail:all").getAttribute("role")).toBe("menuitemradio");
    expect(row("detail:all").getAttribute("aria-checked")).toBe("true");
    expect(row("detail:all").classList.contains("on")).toBe(true);
    expect(row("detail:agent").getAttribute("aria-checked")).toBe("false");
    expect(row("detail:agent").classList.contains("on")).toBe(false);
    expect([...host.querySelectorAll(".mi")].every((each) => each.getAttribute("tabindex") === "-1")).toBe(true);
  });

  it("says the opener holds a menu, and that it is shut", () => {
    const host = mounted();
    const caret = host.querySelector(".caret");

    expect(caret.getAttribute("aria-haspopup")).toBe("menu");
    expect(caret.getAttribute("aria-expanded")).toBe("false");
  });

  it("escapes a group's label", () => {
    const html = groupedMenuButtonMarkup("⋮", [{ id: "x", label: "<b>Show</b>", options: [SHELLS] }]);
    expect(html).not.toContain("<b>Show</b>");
    expect(html).toContain("&lt;b&gt;Show&lt;/b&gt;");
  });
});

describe("a flat menu's rows", () => {
  it("are plain items unless they carry an answer", () => {
    document.body.innerHTML = menuButtonMarkup("Model", [
      { id: "opus", label: "Opus", description: "", selected: true },
      { id: "sonnet", label: "Sonnet", description: "", selected: false },
    ]);
    const rows = [...document.querySelectorAll(".mi")];
    expect(rows.map((row) => row.getAttribute("role"))).toEqual(["menuitemradio", "menuitemradio"]);
    expect(rows.map((row) => row.getAttribute("aria-checked"))).toEqual(["true", "false"]);
    expect(document.querySelector(".splitmenu").getAttribute("role")).toBe("menu");

    document.body.innerHTML = menuButtonMarkup("Ask", [SHELLS]);
    expect(document.querySelector(".mi").getAttribute("role")).toBe("menuitem");
    document.body.innerHTML = "";
  });
});

describe("driving a split menu from the keyboard", () => {
  let onChoose;
  let container;
  let caret;
  let menu;
  const rows = () => [...menu.querySelectorAll(".mi")];
  const focused = () => document.activeElement?.dataset.action;

  beforeEach(() => {
    onChoose = vi.fn();
    document.body.innerHTML = "";
    container = document.createElement("div");
    container.innerHTML = groupedMenuButtonMarkup("⋮", GROUPS, { title: "Conversation menu", icon: true });
    document.body.appendChild(container);
    mountSplitMenu(container, { onChoose });
    caret = container.querySelector(".caret");
    menu = container.querySelector(".splitmenu");
    caret.focus();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("opens on ArrowDown with the first row focused, and on ArrowUp with the last", () => {
    expect(keydown(caret, "ArrowDown").defaultPrevented).toBe(true);

    expect(menu.hidden).toBe(false);
    expect(caret.getAttribute("aria-expanded")).toBe("true");
    expect(focused()).toBe("shells");

    keydown(menu, "Escape");
    keydown(caret, "ArrowUp");
    expect(focused()).toBe("detail:agent");
  });

  it("walks the rows with the arrows, wrapping at both ends, and jumps with Home and End", () => {
    keydown(caret, "ArrowDown");

    keydown(menu, "ArrowDown");
    expect(focused()).toBe("checklist");
    keydown(menu, "ArrowDown");
    keydown(menu, "ArrowDown");
    expect(focused()).toBe("detail:agent");
    keydown(menu, "ArrowDown");
    expect(focused()).toBe("shells");
    keydown(menu, "ArrowUp");
    expect(focused()).toBe("detail:agent");
    keydown(menu, "Home");
    expect(focused()).toBe("shells");
    keydown(menu, "End");
    expect(focused()).toBe("detail:agent");
  });

  it("chooses the focused row on Enter, shuts, and hands focus back to the opener", async () => {
    keydown(caret, "ArrowDown");
    keydown(menu, "ArrowDown");

    const event = keydown(menu, "Enter");
    await motionBeat();

    expect(event.defaultPrevented).toBe(true);
    expect(onChoose).toHaveBeenCalledWith("checklist");
    expect(menu.hidden).toBe(true);
    expect(caret.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(caret);
  });

  it("chooses on Space as well", () => {
    keydown(caret, "ArrowDown");

    keydown(menu, " ");

    expect(onChoose).toHaveBeenCalledWith("shells");
  });

  it("shuts on Escape without choosing, focus back on the opener", async () => {
    keydown(caret, "ArrowDown");

    keydown(menu, "Escape");
    await motionBeat();

    expect(onChoose).not.toHaveBeenCalled();
    expect(menu.hidden).toBe(true);
    expect(document.activeElement).toBe(caret);
  });

  it("shuts on Escape pressed on the opener while it is open", async () => {
    caret.click();
    expect(menu.hidden).toBe(false);

    keydown(caret, "Escape");
    await motionBeat();

    expect(menu.hidden).toBe(true);
  });

  it("shuts when focus leaves it for somewhere outside", async () => {
    const elsewhere = document.createElement("button");
    document.body.appendChild(elsewhere);
    keydown(caret, "ArrowDown");

    elsewhere.focus();
    await motionBeat();

    expect(menu.hidden).toBe(true);
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("stays open while focus moves between its own rows and its opener", async () => {
    keydown(caret, "ArrowDown");

    rows()[1].focus();
    caret.focus();
    await motionBeat();

    expect(menu.hidden).toBe(false);
  });

  it("keeps the opener's expanded state in step with a pointer too", async () => {
    caret.click();
    expect(caret.getAttribute("aria-expanded")).toBe("true");

    rows()[0].click();
    await motionBeat();

    expect(caret.getAttribute("aria-expanded")).toBe("false");
    expect(onChoose).toHaveBeenCalledWith("shells");
  });
});

describe("mountMenuIfChanged", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  const markupAt = (level) => groupedMenuButtonMarkup("⋮", [
    { id: "detail", label: "Detail", options: [
      { id: "detail:all", label: "All", description: "", selected: level === "all" },
      { id: "detail:agent", label: "Agent only", description: "", selected: level === "agent" },
    ] },
  ], { title: "Conversation menu", icon: true });

  it("hands focus to the new opener when the old one had it", () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    mountMenuIfChanged(container, markupAt("all"), { onChoose: () => {} });
    container.querySelector(".caret").focus();

    mountMenuIfChanged(container, markupAt("agent"), { onChoose: () => {} });

    expect(document.activeElement).toBe(container.querySelector(".caret"));
    expect(container.querySelector('.mi[data-action="detail:agent"]').getAttribute("aria-checked")).toBe("true");
  });

  it("leaves focus alone when it was somewhere else", () => {
    const container = document.createElement("div");
    const elsewhere = document.createElement("button");
    document.body.append(container, elsewhere);
    mountMenuIfChanged(container, markupAt("all"), { onChoose: () => {} });
    elsewhere.focus();

    mountMenuIfChanged(container, markupAt("agent"), { onChoose: () => {} });

    expect(document.activeElement).toBe(elsewhere);
  });
});
