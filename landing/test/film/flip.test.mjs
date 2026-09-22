import { describe, it } from "node:test";
import assert from "node:assert/strict";
import gsap from "gsap";
import { flip } from "../../src/film/overlays.js";

// A stand-in for the label the film flips: text and a class list, no DOM.
function label(text) {
  const classes = new Set();
  return {
    textContent: text,
    classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)), contains: (name) => classes.has(name) },
  };
}

describe("a flip on the timeline", () => {
  it("hands back the text the previous flip left when scrubbed backward (act 4)", () => {
    const status = label("Working");
    const tl = gsap.timeline({ paused: true });
    flip(tl, status, 45, { text: "Waiting", className: "waiting" });
    flip(tl, status, 75, { text: "Working", className: "waiting", off: true });
    tl.time(60);
    assert.deepEqual([status.textContent, status.classList.contains("waiting")], ["Waiting", true]);
    tl.time(80);
    assert.deepEqual([status.textContent, status.classList.contains("waiting")], ["Working", false]);
    tl.time(60);
    assert.deepEqual([status.textContent, status.classList.contains("waiting")], ["Waiting", true]);
    tl.time(0);
    assert.deepEqual([status.textContent, status.classList.contains("waiting")], ["Working", false]);
  });

  it("restores Staged, not the markup's label, between the stage and the commit (act 6)", () => {
    const tree = label("Working tree");
    const tl = gsap.timeline({ paused: true });
    flip(tl, tree, 59, { text: "Staged" });
    flip(tl, tree, 79, { text: "Working tree" });
    tl.time(70);
    assert.equal(tree.textContent, "Staged");
    tl.time(90);
    assert.equal(tree.textContent, "Working tree");
    tl.time(70);
    assert.equal(tree.textContent, "Staged");
    tl.time(50);
    assert.equal(tree.textContent, "Working tree");
  });

  it("survives a jump straight over both flips and back", () => {
    const tree = label("Working tree");
    const tl = gsap.timeline({ paused: true });
    flip(tl, tree, 59, { text: "Staged" });
    flip(tl, tree, 79, { text: "Working tree" });
    tl.time(100);
    assert.equal(tree.textContent, "Working tree");
    tl.time(0);
    assert.equal(tree.textContent, "Working tree");
    tl.time(70);
    assert.equal(tree.textContent, "Staged");
  });
});
