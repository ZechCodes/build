import { describe, it, expect } from "vitest";
import { hasDraftValues } from "../src/core/sheetDismiss.js";

describe("hasDraftValues", () => {
  it("is false for no fields", () => {
    expect(hasDraftValues([])).toBe(false);
  });

  it("is false when every field is blank or whitespace-only", () => {
    expect(hasDraftValues(["", "   ", "\n\t"])).toBe(false);
  });

  it("is true when any field has non-whitespace content", () => {
    expect(hasDraftValues(["", "a goal"])).toBe(true);
    expect(hasDraftValues(["  trimmed  "])).toBe(true);
  });
});
