import { expect } from "vitest";

// Both cold opens and workspace switches use the same visible headroom.
// Chromium's integer scrollTop can round a fractional layout by one pixel.
export function expectNewHeadroom(offset, detail = "") {
  expect(Math.abs(offset - 12), detail).toBeLessThanOrEqual(1);
}
