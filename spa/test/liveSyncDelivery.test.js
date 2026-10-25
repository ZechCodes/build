import { expect, it } from "vitest";
import { summarizeLiveSyncDelivery, unionIntervals } from "./browser/liveSyncDelivery.mjs";

const capture = (count) => ({ framesReceived: count * 6, ownRepairHolds: [], ui: { stages: Array.from({ length: count }, (_, index) => {
  const sequence = 1101 + index, receipt = index * 100;
  return ["receipt", "admission", "workerStart", "cacheCommit", "mountedPaint"].map((stage, offset) => ({ stage, entityId: "run-0", sequences: [sequence], at: receipt + (offset === 4 ? index + 1 : 0) }));
}).flat() } });

it("unions overlapping own repair waits and clips them to each tip's lifetime", () => {
  const result = capture(1);
  for (const event of result.ui.stages) event.at = event.stage === "mountedPaint" ? 100 : event.at;
  result.ownRepairHolds = [{ start: -20, end: 20 }, { start: 10, end: 40 }, { start: 60, end: 120 }];
  const summary = summarizeLiveSyncDelivery(result);
  expect(summary.measuredOwnRepairHoldUnion).toEqual([{ start: -20, end: 40 }, { start: 60, end: 120 }]);
  expect(summary.raw.maxMs).toBe(100);
  expect(summary.holdAdjusted.maxMs).toBe(20);
  expect(summary.lateUnheld.samples).toBe(0);
});

it("uses nearest rank and reports missing stages without censoring expected tips", () => {
  const result = capture(26);
  const summary = summarizeLiveSyncDelivery(result);
  expect(summary.raw.p95Ms).toBe(25);
  expect(summary.complete).toBe(true);
  result.ui.stages = result.ui.stages.filter((event) => !(event.stage === "workerStart" && event.sequences[0] === 1126));
  expect(summarizeLiveSyncDelivery(result).complete).toBe(false);
  expect(summarizeLiveSyncDelivery(result).completeness.workerStart).toBe(25);
});

it("does not subtract an unrecorded hold and counts late tips only after hold end", () => {
  const result = capture(2);
  result.ownRepairHolds = [{ start: -10, end: 50 }];
  expect(summarizeLiveSyncDelivery(result).lateUnheld.samples).toBe(1);
  delete result.ownRepairHolds;
  expect(summarizeLiveSyncDelivery(result).holdAdjusted).toBe(null);
  expect(() => unionIntervals([{ start: 2, end: 1 }])).toThrow("Invalid measured hold interval");
});


it("recognizes coalesced covering goals without inventing causal stage intervals", () => {
  const result = capture(2);
  result.ui.stages = result.ui.stages.filter((event) => !(["admission", "workerStart"].includes(event.stage) && event.sequences[0] === 1101));
  const summary = summarizeLiveSyncDelivery(result);
  expect(summary.complete).toBe(true);
  expect(summary.completeness.workerStart).toBe(2);
  expect(summary.unorderedStageIntervals.workerToCommit).toBe(1);
  expect(summary.stageLatencies.workerToCommit.samples).toBe(1);
});
