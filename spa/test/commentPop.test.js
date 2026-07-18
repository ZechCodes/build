import { describe, it, expect } from "vitest";
import { outsideTapAction } from "../src/commentPop.js";

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
