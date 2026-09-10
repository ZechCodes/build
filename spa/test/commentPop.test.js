// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { hasCommentPop, hideCommentPop, openCommentComposer, outsideTapAction, showCommentPop } from "../src/commentPop.js";

describe("outsideTapAction", () => {
  it("discards on an outside tap when the draft is empty, regardless of armed state", () => {
    expect(outsideTapAction(false, false)).toBe("discard");
    expect(outsideTapAction(false, true)).toBe("discard");
  });

  it("arms (does not discard) on the FIRST outside tap when the draft has text", () => {
    expect(outsideTapAction(true, false)).toBe("arm");
  });

  it("discards on the SECOND outside tap (already armed) when the draft has text", () => {
    expect(outsideTapAction(true, true)).toBe("discard");
  });
});

// Two ways a comment starts, and they are not the same gesture.
//
// A reader who PRESSED something has already said what they want, so the field
// is what they get — a button that opens a field is a second press for nothing.
// A reader who has just finished dragging out a SELECTION has said nothing yet,
// and a textarea taking focus the instant they let go would collapse the very
// selection the comment is about. That one keeps its button.
describe("how a comment composer opens", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    hideCommentPop();
  });

  const rect = { top: 10, bottom: 30, left: 10 };

  it("gives a reader who pressed the field itself", () => {
    openCommentComposer(rect, () => {});
    expect(document.querySelector(".cp-input")).toBeTruthy();
    expect(document.querySelector(".cp-add")).toBeNull();
  });

  it("takes what they typed", () => {
    const added = [];
    openCommentComposer(rect, (comment) => added.push(comment));
    document.querySelector(".cp-input").value = "rename this";
    document.querySelector(".cp-save").click();
    expect(added).toEqual(["rename this"]);
    expect(hasCommentPop()).toBe(false);
  });

  it("submits nothing when nothing was typed", () => {
    const added = [];
    openCommentComposer(rect, (comment) => added.push(comment));
    document.querySelector(".cp-save").click();
    expect(added).toEqual([]);
  });

  it("offers a selection the button first, so the selection survives", () => {
    showCommentPop(rect, () => {});
    expect(document.querySelector(".cp-add")).toBeTruthy();
    expect(document.querySelector(".cp-input")).toBeNull();
  });
});
