import { expect, it } from "vitest";
import { runPushReplay } from "./pushReplayHarness.mjs";

it("bounds push IndexedDB work under concurrent multi-project agents while the open conversation renders", async () => {
  const result = await runPushReplay();
  expect(result.peakAppliers).toBeLessThanOrEqual(2);
  expect(result.peakTransactions).toBeLessThanOrEqual(8);
  expect(result.pendingTransactions).toBe(0);
  expect(result.pendingAppliers).toBe(0);
  expect(result.sequences).toEqual(Array.from({ length: 6 }, () => Array.from({ length: 10 }, (_, index) => index + 1)));
  expect(result.finalFrames).toEqual([54, 55, 56, 57, 58, 59]);
  expect(result.rendered).toBe("10");
  expect(result.renderedText).toContain("frame 54");
  expect(result.cacheLatencies).toHaveLength(10);
  expect(result.cacheMaxMs).toBeLessThan(2000);
}, 60_000);
