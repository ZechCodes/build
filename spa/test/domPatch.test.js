// @vitest-environment jsdom
// Writing only the disagreements.
//
// The conversation re-renders on a poll; the patch is what stands between that
// render and the reader's page. Two trees that already agree must come out of
// it byte for byte the same nodes, and a tree that changed one word must lose
// only that word.

import { describe, expect, it } from "vitest";
import { EXPANDED_ATTRIBUTE, KEYED_LIST_ATTRIBUTE, patchElement } from "../src/core/domPatch.js";

const tree = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};

/** Everything the patch moved. */
function mutationsOf(live, next) {
  const observer = new MutationObserver(() => {});
  observer.observe(live, { childList: true, subtree: true, attributes: true, characterData: true });
  try {
    patchElement(live, next);
    return observer.takeRecords();
  } finally {
    observer.disconnect();
  }
}

describe("patching a live tree to say what a rendered one says", () => {
  it("touches nothing when the two already agree", () => {
    const markup = `<p class="a">said <em>this</em></p><p>and that</p>`;
    const live = tree(markup);
    expect(mutationsOf(live, tree(markup))).toEqual([]);
  });

  it("rewrites a word without replacing the element holding it", () => {
    const live = tree(`<p class="a">half</p>`);
    const paragraph = live.querySelector("p");

    const records = mutationsOf(live, tree(`<p class="a">half a thought</p>`));

    expect(live.querySelector("p")).toBe(paragraph);
    expect(paragraph.textContent).toBe("half a thought");
    expect(records.map((record) => record.type)).toEqual(["characterData"]);
  });

  it("moves an attribute without disturbing the children", () => {
    const live = tree(`<div class="one"><span>kept</span></div>`);
    const kept = live.querySelector("span");

    patchElement(live, tree(`<div class="two"><span>kept</span></div>`));

    expect(live.querySelector("div").className).toBe("two");
    expect(live.querySelector("span")).toBe(kept);
  });

  it("drops an attribute the render no longer carries", () => {
    const live = tree(`<div class="one" hidden></div>`);
    patchElement(live, tree(`<div class="one"></div>`));
    expect(live.querySelector("div").hasAttribute("hidden")).toBe(false);
  });

  // A fold the render owns is the other half of that rule: a run of activity
  // draws its rows only while it is open, so a render that shut one has to be
  // able to take `open` back off — nobody else writes it.
  it("shuts the fold whose renderer owns it", () => {
    const live = tree(`<details data-rendered-fold open><summary>a</summary></details>`);
    patchElement(live, tree(`<details data-rendered-fold><summary>a</summary></details>`));
    expect(live.querySelector("details").open).toBe(false);
  });

  it("leaves the fold and the menu the reader opened open", () => {
    const live = tree(`<details open><summary>a</summary></details><div class="splitmenu"></div>`);
    patchElement(live, tree(`<details><summary>a</summary></details><div class="splitmenu" hidden></div>`));
    expect(live.querySelector("details").open).toBe(true);
    expect(live.querySelector(".splitmenu").hasAttribute("hidden")).toBe(false);
  });

  it("leaves the text the reader chose to see in full expanded", () => {
    const live = tree(`<span class="surface-clip" ${EXPANDED_ATTRIBUTE}>a long line</span>`);
    patchElement(live, tree(`<span class="surface-clip">a longer line</span>`));
    expect(live.querySelector("span").hasAttribute(EXPANDED_ATTRIBUTE)).toBe(true);
    expect(live.querySelector("span").textContent).toBe("a longer line");
  });

  it("keeps the aria on expanded text saying what the reader made it, and hands it back on the way in", () => {
    const expanded = tree(`<span class="surface-clip" role="button" aria-expanded="true" ${EXPANDED_ATTRIBUTE}>a long line</span>`);
    patchElement(expanded, tree(`<span class="surface-clip" role="button" aria-expanded="false">a long line</span>`));
    expect(expanded.querySelector("span").getAttribute("aria-expanded")).toBe("true");

    const clipped = tree(`<span class="surface-clip" role="button" aria-expanded="true">a long line</span>`);
    patchElement(clipped, tree(`<span class="surface-clip" role="button" aria-expanded="false">a long line</span>`));
    expect(clipped.querySelector("span").getAttribute("aria-expanded")).toBe("false");
  });

  it("leaves an expansion mark stranded on anything but a clip to the render", () => {
    const live = tree(`<div class="file" ${EXPANDED_ATTRIBUTE} aria-expanded="true">a file</div>`);
    patchElement(live, tree(`<div class="file collapsed">a file</div>`));
    expect(live.querySelector("div").hasAttribute(EXPANDED_ATTRIBUTE)).toBe(false);
    expect(live.querySelector("div").hasAttribute("aria-expanded")).toBe(false);
  });

  it("leaves the children of a list another painter keys alone", () => {
    const live = tree(`<div ${KEYED_LIST_ATTRIBUTE} class="one"><p data-key="a">row</p></div>`);
    const row = live.querySelector("p");

    patchElement(live, tree(`<div ${KEYED_LIST_ATTRIBUTE} class="two"></div>`));

    expect(live.querySelector("div").className).toBe("two");
    expect(live.querySelector("p")).toBe(row);
  });

  it("replaces a node the render made a different kind of thing", () => {
    const live = tree(`<p>a</p>`);
    patchElement(live, tree(`<section>a</section>`));
    expect(live.innerHTML).toBe(`<section>a</section>`);
  });

  it("appends what is new and removes what is gone", () => {
    const live = tree(`<p>one</p><p>two</p>`);
    const first = live.querySelector("p");

    patchElement(live, tree(`<p>one</p><p>two</p><p>three</p>`));
    expect([...live.querySelectorAll("p")].map((p) => p.textContent)).toEqual(["one", "two", "three"]);
    expect(live.querySelector("p")).toBe(first);

    patchElement(live, tree(`<p>one</p>`));
    expect([...live.querySelectorAll("p")].map((p) => p.textContent)).toEqual(["one"]);
  });

  // The render leaves `src` out for bytes it does not hold. That is a statement
  // about the renderer, not about the picture, so a picture already showing
  // keeps showing — and re-fetching a screenshot every 1.6 seconds, at zero
  // height until it lands, is exactly what dragged the reader's scroll.
  it("keeps the bytes an image already holds", () => {
    const live = tree(`<img data-attachment-path="a.png" src="data:image/png;base64,AA" alt="a">`);
    const picture = live.querySelector("img");

    const records = mutationsOf(live, tree(`<img data-attachment-path="a.png" alt="a">`));

    expect(live.querySelector("img")).toBe(picture);
    expect(picture.getAttribute("src")).toBe("data:image/png;base64,AA");
    expect(records).toEqual([]);
  });

  it("does not show one attachment's bytes for another's", () => {
    const live = tree(`<img data-attachment-path="a.png" src="data:image/png;base64,AA" alt="a">`);
    patchElement(live, tree(`<img data-attachment-path="b.png" alt="b">`));
    const picture = live.querySelector("img");
    expect(picture.getAttribute("data-attachment-path")).toBe("b.png");
    expect(picture.hasAttribute("src")).toBe(false);
  });
});

describe("patching a canvas being painted into", () => {
  it("leaves a canvas the size it was drawn at", () => {
    const live = tree(`<canvas class="rail-glyph"></canvas>`);
    const face = live.querySelector("canvas");
    face.width = 64;
    face.height = 64;

    const records = mutationsOf(live, tree(`<canvas class="rail-glyph"></canvas>`));

    expect(live.querySelector("canvas")).toBe(face);
    expect(face.getAttribute("width")).toBe("64");
    expect(face.getAttribute("height")).toBe("64");
    expect(records).toEqual([]);
  });

  it("takes the size the render names", () => {
    const live = tree(`<canvas width="64" height="64"></canvas>`);
    patchElement(live, tree(`<canvas width="32" height="32"></canvas>`));
    expect(live.querySelector("canvas").getAttribute("width")).toBe("32");
  });
});

// A checkbox the reader pressed carries its state as a PROPERTY, and the render
// declares it as an ATTRIBUTE. Comparing attributes alone, the two agree — so a
// render saying "unticked" wrote nothing and the box stayed ticked on screen.
// That is the whole of why Clear left every box in the diff still ticked.
describe("a box the render owns", () => {
  const box = (checked) => tree(`<input type="checkbox" class="fselect-box"${checked ? " checked" : ""}/>`);

  it("unticks one the render no longer says is ticked", () => {
    const live = box(false);
    live.querySelector("input").checked = true; // the reader pressed it
    patchElement(live, box(false));
    expect(live.querySelector("input").checked).toBe(false);
  });

  it("ticks one the render says is ticked", () => {
    const live = box(false);
    patchElement(live, box(true));
    expect(live.querySelector("input").checked).toBe(true);
  });

  it("leaves a box that already agrees completely alone", () => {
    const live = box(true);
    live.querySelector("input").checked = true;
    expect(mutationsOf(live, box(true))).toEqual([]);
    expect(live.querySelector("input").checked).toBe(true);
  });

  it("says the same thing about a radio", () => {
    const live = tree('<input type="radio" name="r"/>');
    live.querySelector("input").checked = true;
    patchElement(live, tree('<input type="radio" name="r"/>'));
    expect(live.querySelector("input").checked).toBe(false);
  });

  it("never reaches into a box that is not a box", () => {
    const live = tree('<input type="text" value="typed"/>');
    live.querySelector("input").value = "half a sentence";
    patchElement(live, tree('<input type="text" value="typed"/>'));
    expect(live.querySelector("input").value, "what is being typed is the reader's").toBe("half a sentence");
  });
});
