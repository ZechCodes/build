// @vitest-environment jsdom
// The box grows to its text and shrinks back, writing its own height only
// when that height moves (#138).
//
// jsdom lays nothing out, so a textarea here is given the one piece of layout
// the growing reads: its scrollHeight. It wraps 30 characters to a 20px line
// inside 16px of padding, and like a browser's it reports at least the height
// the box already stands at — a box held at 132px says 132 however little text
// is left in it, which is why a shrink can only be found by measuring the text
// somewhere the old height does not hold it.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { autoGrow } from "../src/core/composer.js";

const LINE = 20;
const PADDING = 16;
const PER_LINE = 30;

const linesOf = (text) => text.split("\n").reduce((lines, line) => lines + Math.max(1, Math.ceil(line.length / PER_LINE)), 0);

const boxHeight = (textarea) => {
  const set = Number.parseFloat(textarea.style.height);
  return Number.isFinite(set) ? set : textarea.rows * LINE + PADDING;
};

let described = null;

beforeEach(() => {
  described = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "scrollHeight")
    || Object.getOwnPropertyDescriptor(Element.prototype, "scrollHeight");
  Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", {
    configurable: true,
    get() { return Math.max(boxHeight(this), linesOf(this.value) * LINE + PADDING); },
  });
  document.body.innerHTML = '<textarea id="box" rows="1" style="width:330px"></textarea>';
});

afterEach(() => {
  if (described) Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", described);
  else delete HTMLTextAreaElement.prototype.scrollHeight;
  document.body.innerHTML = "";
});

const box = () => document.getElementById("box");

/** Put `text` in the box the way an edit does: the value moves, then the
 *  input event says so. */
const edit = (text) => {
  box().value = text;
  box().dispatchEvent(new Event("input", { bubbles: true }));
};

const heightWrites = () => {
  const writes = [];
  const observer = new MutationObserver((batch) => writes.push(...batch));
  observer.observe(box(), { attributes: true, attributeFilter: ["style"] });
  return () => {
    writes.push(...observer.takeRecords());
    observer.disconnect();
    return writes.length;
  };
};

describe("a box that grows to its text", () => {
  it("shrinks when a selection is replaced by as many characters on one line", () => {
    autoGrow(box());
    edit("a\nb\nc\nd\ne\nf");
    expect(box().style.height).toBe("136px");

    // Select all, paste eleven characters over eleven.
    edit("abcdefghijk");

    expect(box().style.height).toBe("36px");
  });

  it("shrinks when longer text takes out the line breaks it replaces", () => {
    autoGrow(box());
    edit("one\ntwo\nthree\nfour");
    expect(box().style.height).toBe("96px");

    edit("one, two, three and four");

    expect(box().style.height).toBe("36px");
  });

  it("grows as a line wraps and shrinks as the text is taken back", () => {
    autoGrow(box());
    edit("x".repeat(29));
    expect(box().style.height).toBe("36px");
    edit("x".repeat(31));
    expect(box().style.height).toBe("56px");
    edit("x".repeat(12));
    expect(box().style.height).toBe("36px");
  });

  it("writes the box's height only when a key moves it", () => {
    autoGrow(box());
    const writes = heightWrites();

    let text = "";
    for (const letter of "a sentence that runs past its line") {
      text += letter;
      edit(text);
    }
    for (let i = 0; i < 8; i++) {
      text = text.slice(0, -1);
      edit(text);
    }

    // Once as it wrapped onto a second line, once as the backspaces took it
    // back to one: nothing for the other forty-one keys.
    expect(writes()).toBe(2);
  });

  it("stands as many rows tall as the box asks for with little text in it", () => {
    document.body.innerHTML = '<textarea id="box" rows="3" style="width:330px"></textarea>';
    autoGrow(box());
    edit("hi");
    expect(box().style.height).toBe("76px");
  });
});
