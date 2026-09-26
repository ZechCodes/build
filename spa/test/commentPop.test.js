// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { hasCommentPop, hideCommentPop, openCommentComposer, outsideTapAction, showCommentPop, suspendCommentPop } from "../src/commentPop.js";

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

// A surface kept while another shows (views/workspaceChanges.js) takes its
// draft with it: off the page, nothing of it actionable, back when it shows.
describe("a draft put away with its surface", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    hideCommentPop();
  });

  const rect = { top: 10, bottom: 30, left: 10 };
  const press = (target) => target.dispatchEvent(new Event("pointerdown", { bubbles: true }));

  it("leaves the page disarmed, and comes back with its text to the same callback", async () => {
    const added = [];
    openCommentComposer(rect, (comment) => added.push(comment));
    document.querySelector(".cp-input").value = "keep this comment";
    // The outside tap is listened for from the next turn on.
    await new Promise((resolve) => setTimeout(resolve, 0));
    press(document.body);
    expect(document.querySelector(".comment-pop.cp-armed")).not.toBeNull();

    const restore = suspendCommentPop();
    expect(document.querySelector(".comment-pop")).toBeNull();
    expect(hasCommentPop()).toBe(false);
    // Taps on whatever shows now reach nothing of it.
    press(document.body);
    press(document.body);

    restore();
    expect(document.querySelector(".comment-pop.cp-armed")).toBeNull();
    expect(document.querySelector(".cp-input").value).toBe("keep this comment");
    // Its outside tap is back: the first arms, the second discards.
    press(document.body);
    expect(document.querySelector(".comment-pop.cp-armed")).not.toBeNull();
    document.querySelector(".cp-save").click();
    expect(added).toEqual(["keep this comment"]);
    expect(hasCommentPop()).toBe(false);
  });

  it("comes back over a popover opened since, closing it", () => {
    openCommentComposer(rect, () => {});
    document.querySelector(".cp-input").value = "first";
    const restore = suspendCommentPop();
    openCommentComposer(rect, () => {});
    restore();
    expect(document.querySelectorAll(".comment-pop")).toHaveLength(1);
    expect(document.querySelector(".cp-input").value).toBe("first");
  });

  it("closes a bare Comment button, which holds nothing to keep", () => {
    showCommentPop(rect, () => {});
    expect(suspendCommentPop()).toBeNull();
    expect(document.querySelector(".comment-pop")).toBeNull();
    expect(suspendCommentPop()).toBeNull();
  });

  it("only suspends the popover owned by the pane being hidden", () => {
    const repository = Symbol("repository");
    const assets = Symbol("assets");
    openCommentComposer(rect, () => {}, assets);
    document.querySelector(".cp-input").value = "assets draft";
    expect(suspendCommentPop(repository)).toBeNull();
    expect(hasCommentPop(assets)).toBe(true);
    hideCommentPop(repository);
    expect(document.querySelector(".cp-input").value).toBe("assets draft");
  });
});
