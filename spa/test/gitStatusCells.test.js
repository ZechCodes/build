import { describe, expect, it } from "vitest";
import { gitStatusCells, gitStatusPlan } from "../src/core/gitStatusCells.js";

const keysOf = (cells) => cells.map((cell) => cell.key);
const textOf = (cells) => cells.map((cell) => cell.char).join("");

describe("the characters the git status is made of", () => {
  it("is one cell per glyph and per digit, four sections left to right", () => {
    const cells = gitStatusCells({ ahead: 2, behind: 1, insertions: 12, deletions: 4 });

    expect(textOf(cells)).toBe("↑2↓1+12−4");
    expect(keysOf(cells)).toEqual([
      "ahead:glyph",
      "ahead:d0",
      "behind:glyph",
      "behind:d0",
      "insertions:glyph",
      "insertions:d1",
      "insertions:d0",
      "deletions:glyph",
      "deletions:d0",
    ]);
  });

  it("names the section and the slot each cell belongs to, so the sheet can space them", () => {
    const [glyph, digit] = gitStatusCells({ ahead: 2 });

    expect(glyph).toEqual({ key: "ahead:glyph", char: "↑", section: "ahead", slot: "glyph" });
    expect(digit).toEqual({ key: "ahead:d0", char: "2", section: "ahead", slot: "digit" });
  });

  it("says nothing about a section with nothing to report", () => {
    expect(gitStatusCells({ ahead: 0, behind: 0, insertions: 0, deletions: 0 })).toEqual([]);
    expect(gitStatusCells(null)).toEqual([]);
    expect(textOf(gitStatusCells({ ahead: 0, behind: 3, insertions: 0, deletions: 1 }))).toBe("↓3−1");
  });

  it("reads a diffstat an older bridge sent ready-made as a string", () => {
    expect(textOf(gitStatusCells("+4 −1"))).toBe("+4−1");
    expect(textOf(gitStatusCells("+4 -1"))).toBe("+4−1");
  });

  it("keys digits from the right, so the units column stays the units column", () => {
    expect(keysOf(gitStatusCells({ ahead: 10 }))).toEqual(["ahead:glyph", "ahead:d1", "ahead:d0"]);
  });
});

describe("the plan for moving from one status to the next", () => {
  const cellsOf = gitStatusCells;

  it("cascades every character in when there was no status at all, right to left", () => {
    const plan = gitStatusPlan([], cellsOf({ ahead: 2 }));

    expect(plan.enters).toEqual(["ahead:d0", "ahead:glyph"]);
    expect(plan.exits).toEqual([]);
    expect(plan.rolls).toEqual([]);
  });

  it("rolls a digit that changed, and moves nothing else", () => {
    const plan = gitStatusPlan(cellsOf({ ahead: 1 }), cellsOf({ ahead: 7 }));

    expect(plan.rolls).toEqual([{ key: "ahead:d0", from: "1", to: "7" }]);
    expect(plan.enters).toEqual([]);
    expect(plan.exits).toEqual([]);
  });

  it("cascades a new section in and leaves the sections beside it standing", () => {
    const plan = gitStatusPlan(cellsOf({ insertions: 4 }), cellsOf({ behind: 2, insertions: 4 }));

    expect(plan.enters).toEqual(["behind:d0", "behind:glyph"]);
    expect(plan.exits).toEqual([]);
    expect(plan.rolls).toEqual([]);
  });

  it("cascades a section that is gone out left to right", () => {
    const plan = gitStatusPlan(cellsOf({ ahead: 12, insertions: 4 }), cellsOf({ insertions: 4 }));

    expect(plan.exits).toEqual(["ahead:glyph", "ahead:d1", "ahead:d0"]);
    expect(plan.enters).toEqual([]);
  });

  it("treats a count falling to zero as the section leaving, not a roll to nothing", () => {
    const plan = gitStatusPlan(cellsOf({ behind: 1 }), cellsOf({ behind: 0 }));

    expect(plan.exits).toEqual(["behind:glyph", "behind:d0"]);
    expect(plan.rolls).toEqual([]);
  });

  it("grows a number by rolling the columns it kept and cascading the new one in", () => {
    const plan = gitStatusPlan(cellsOf({ ahead: 9 }), cellsOf({ ahead: 10 }));

    expect(plan.rolls).toEqual([{ key: "ahead:d0", from: "9", to: "0" }]);
    expect(plan.enters).toEqual(["ahead:d1"]);
    expect(plan.exits).toEqual([]);
  });

  it("has nothing to do when the status says what it already said", () => {
    const plan = gitStatusPlan(cellsOf({ ahead: 2 }), cellsOf({ ahead: 2 }));

    expect(plan).toEqual({ cells: cellsOf({ ahead: 2 }), enters: [], exits: [], rolls: [] });
  });
});
