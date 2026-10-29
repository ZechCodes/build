/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountFilterMenu } from "../src/core/filterMenuControl.js";

const VALUES = ["AgentA", "AgentA\0suffix", "AgentA\uFFFDsuffix", "AgentA\0suffix-more"];
const offers = (name) => VALUES.map((value) => ({
  value: name === "assignee" ? `agent:${value}` : value,
  label: `${value} <img src=x onerror=alert(1)> & "quoted"`,
  group: "Workspace <one>",
}));

let host, menu;
beforeEach(() => {
  document.body.innerHTML = '<div id="host"></div>';
  host = document.querySelector("#host");
});
afterEach(() => menu?.dispose());

const rows = () => [...host.querySelectorAll('[role="option"]')];
const key = (value) => host.querySelector(".fmenu-search").dispatchEvent(
  new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true }),
);
const start = (name, options) => {
  const changed = vi.fn();
  menu = mountFilterMenu(host, { name, label: name, multi: true, onChange: changed });
  menu.update(options, []);
  host.querySelector(".fmenu-press").click();
  return changed;
};

describe.each(["label", "assignee"])("opaque %s choices", (name) => {
  it.each(VALUES.map((value, index) => [JSON.stringify(value), index]))(
    "click and Enter dispatch the same original value for %s",
    (_name, index) => {
      const options = offers(name);
      const changed = start(name, options);
      expect(rows()[index].querySelector(".fmenu-label").textContent)
        .toBe(options[index].label.replaceAll("\0", ""));
      expect(host.querySelector("img")).toBeNull();

      rows()[index].click();
      expect(changed).toHaveBeenLastCalledWith([options[index].value]);
      expect(rows().map((row) => row.getAttribute("aria-selected")))
        .toEqual(options.map((_option, at) => String(at === index)));

      menu.update(options, []);
      for (let at = 0; at < index; at += 1) key("ArrowDown");
      key("Enter");
      expect(changed).toHaveBeenLastCalledWith([options[index].value]);
    },
  );

  it("keeps retained rows tied to their current choice after updates and reordering", () => {
    const options = offers(name);
    const changed = start(name, options);
    const retainedNul = rows()[1];
    const reordered = [options[3], options[2], options[0], { ...options[1], label: "Renamed <choice>" }];
    menu.update(reordered, []);
    expect(rows()[3]).toBe(retainedNul);
    expect(retainedNul.querySelector(".fmenu-label").textContent).toBe("Renamed <choice>");
    expect(menu.isOpen()).toBe(true);

    retainedNul.click();
    expect(changed).toHaveBeenLastCalledWith([options[1].value]);
    menu.update(reordered, []);
    for (let at = 0; at < 3; at += 1) key("ArrowDown");
    key("Enter");
    expect(changed).toHaveBeenLastCalledWith([options[1].value]);

    menu.update([options[2], options[0]], []);
    rows()[0].click();
    expect(changed).toHaveBeenLastCalledWith([options[2].value]);
  });
});
