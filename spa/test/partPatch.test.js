/** @vitest-environment jsdom */
// A page drawn as named parts repaints only the parts that changed (#153).

import { beforeEach, describe, expect, it } from "vitest";
import { patchParts } from "../src/core/partPatch.js";

let container, held;
const names = () => [...container.children].map((child) => child.dataset.part);

beforeEach(() => {
  document.body.innerHTML = '<div id="page"></div>';
  container = document.querySelector("#page");
  held = new Map();
});

describe("patchParts", () => {
  it("paints every part the first time, in order", () => {
    const painted = patchParts(container, held, [
      { name: "a", html: '<p data-part="a">A</p>' },
      { name: "b", html: '<p data-part="b">B</p>' },
    ]);
    expect(painted).toEqual(["a", "b"]);
    expect(names()).toEqual(["a", "b"]);
  });

  it("keeps the nodes of a part whose HTML did not change", () => {
    patchParts(container, held, [{ name: "a", html: '<p data-part="a">A</p>' }, { name: "b", html: '<p data-part="b">B</p>' }]);
    const [a, b] = container.children;
    const painted = patchParts(container, held, [{ name: "a", html: '<p data-part="a">A</p>' }, { name: "b", html: '<p data-part="b">B2</p>' }]);
    expect(painted).toEqual(["b"]);
    expect(container.children[0]).toBe(a);
    expect(container.children[1]).not.toBe(b);
    expect(container.children[1].textContent).toBe("B2");
  });

  it("keys a part by what needs new nodes, not by all of its HTML", () => {
    patchParts(container, held, [{ name: "box", html: '<textarea data-part="box">one</textarea>', key: "plain" }]);
    const box = container.firstElementChild;
    expect(patchParts(container, held, [{ name: "box", html: '<textarea data-part="box">two</textarea>', key: "plain" }])).toEqual([]);
    expect(container.firstElementChild).toBe(box);
    expect(patchParts(container, held, [{ name: "box", html: '<div data-part="box"></div>', key: "attachable" }])).toEqual(["box"]);
    expect(container.firstElementChild).not.toBe(box);
  });

  it("adds a new part in its place and takes out one no longer listed", () => {
    patchParts(container, held, [{ name: "a", html: '<p data-part="a"></p>' }, { name: "c", html: '<p data-part="c"></p>' }]);
    const a = container.children[0];
    patchParts(container, held, [
      { name: "a", html: '<p data-part="a"></p>' },
      { name: "b", html: '<p data-part="b"></p>' },
      { name: "c", html: '<p data-part="c"></p>' },
    ]);
    expect(names()).toEqual(["a", "b", "c"]);
    expect(container.children[0]).toBe(a);
    const changed = patchParts(container, held, [{ name: "a", html: '<p data-part="a"></p>' }, { name: "c", html: '<p data-part="c"></p>' }]);
    expect(names()).toEqual(["a", "c"]);
    // A part taken out is a change: whatever it carried is gone from the page.
    expect(changed).toEqual(["b"]);
  });

  it("moves a kept part that is out of place", () => {
    patchParts(container, held, [{ name: "a", html: '<p data-part="a"></p>' }, { name: "b", html: '<p data-part="b"></p>' }]);
    patchParts(container, held, [{ name: "b", html: '<p data-part="b"></p>' }, { name: "a", html: '<p data-part="a"></p>' }]);
    expect(names()).toEqual(["b", "a"]);
  });

  it("holds a part with no HTML, and fills it later in its place", () => {
    patchParts(container, held, [
      { name: "a", html: '<p data-part="a"></p>' }, { name: "empty", html: "" }, { name: "z", html: '<p data-part="z"></p>' },
    ]);
    expect(names()).toEqual(["a", "z"]);
    patchParts(container, held, [
      { name: "a", html: '<p data-part="a"></p>' }, { name: "empty", html: '<p data-part="empty"></p>' }, { name: "z", html: '<p data-part="z"></p>' },
    ]);
    expect(names()).toEqual(["a", "empty", "z"]);
  });

  it("places parts after a given node, leaving what precedes it alone", () => {
    container.innerHTML = '<p data-part="before"></p><main data-part="main"></main>';
    patchParts(container, held, [{ name: "rail", html: '<aside data-part="rail"></aside>' }], { after: container.children[1] });
    expect(names()).toEqual(["before", "main", "rail"]);
  });
});
