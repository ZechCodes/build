import { expect, it } from "vitest";
import { paintedRowMatches, paintOrderMatches } from "./browser/liveSyncPaintOracle.mjs";

it("rechecks connection, sequence and exact expected body at the paint frame", () => {
  const row = { isConnected: true, dataset: { sequence: "1102" }, textContent: "frame 1" };
  row.querySelector = () => ({ textContent: row.textContent });
  expect(paintedRowMatches(row, 1102, "frame 1")).toBe(true);
  expect(paintedRowMatches(row, 1102, "frame 6")).toBe(false);
  row.isConnected = false;
  expect(paintedRowMatches(row, 1102, "frame 1")).toBe(false);
  row.isConnected = true;
  row.dataset.sequence = "1101";
  expect(paintedRowMatches(row, 1102, "frame 1")).toBe(false);
  row.dataset.sequence = "1102";
  row.textContent = "stale previous frame";
  expect(paintedRowMatches(row, 1102, "frame 1")).toBe(false);
  row.textContent = "frame 10";
  expect(paintedRowMatches(row, 1102, "frame 1")).toBe(false);
});

it("requires all tips once in increasing order and rejects DOM cursor regression", () => {
  expect(paintOrderMatches([1101, 1102], [1101, 1102], [1101, 1102])).toBe(true);
  expect(paintOrderMatches([1101, 1102], [1101], [1101])).toBe(false);
  expect(paintOrderMatches([1101, 1102], [1102, 1101], [1102, 1102])).toBe(false);
  expect(paintOrderMatches([1101, 1102], [1101, 1102], [1103, 1102])).toBe(false);
});
