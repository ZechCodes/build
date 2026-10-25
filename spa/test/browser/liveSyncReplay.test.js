import { expect, it } from "vitest";
import { runLiveSyncReplay } from "./liveSyncReplayHarness.mjs";

it("profiles mounted inbox, actual rail canvases and conversation against real cache pushes", async () => {
  const result = await runLiveSyncReplay({ frames: 24, intervalMs: 50, tasksPerProject: 2, retainedMessages: 10, resetFrame: 18, removalFrame: 12 });
  expect(result.seedCanvasCount).toBe(1);
  expect(result.animatingCanvasCount).toBeGreaterThan(0);
  expect(result.seedVisibleInboxRows).toBe(4);
  expect(result.ui.timings.inboxDraw.count).toBeGreaterThan(0);
  expect(result.ui.timings.canvasPaint.count).toBeGreaterThan(0);
  expect(result.ui.timings.canvasGeometry?.count || 0).toBeLessThanOrEqual(2);
  expect(result.realPaintLatencies).toHaveLength(4);
  expect(result.realPaintSchedulerTurns).toHaveLength(4);
  expect(Math.max(...result.realPaintSchedulerTurns)).toBeLessThanOrEqual(128);
  for (let sequence = 1101; sequence <= 1104; sequence++) {
    for (const stage of ["receipt", "admission", "workerStart", "mountedPaint"]) {
      expect(result.ui.stages.some((event) => event.stage === stage && event.entityId === "run-0" && event.sequences.includes(sequence)), `${stage} ${sequence}`).toBe(true);
    }
    expect(result.ui.stages.some((event) => event.stage === "cacheCommit" && event.sequences.some((covered) => covered >= sequence))).toBe(true);
  }
  expect(result.noopProbe.directGets).toBeLessThan(128);
  expect(result.rowGets).toBeLessThan(1500);
  expect(result.warmBoundary.pendingTransactions).toBe(0);
  expect(result.warmBoundary.passRunning).toBe(false);
  expect(result.measurementBoundary.pendingTransactions).toBe(0);
  expect(result.measurementBoundary.passRunning).toBe(false);
  expect(result.finalWork).toEqual({ running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
  expect(result.pendingTransactions).toBe(0);
  expect(result.pendingAppliers).toBe(0);
  expect(result.peakWork_pending).toBeLessThanOrEqual(64);
  expect(result.drainSchedulerTurns).toBeLessThanOrEqual(128);
  expect(result.exactFinalMessages).toBe(true);
  expect(result.exactFinalStates).toBe(true);
  expect(result.readMarkerConstraints).toBe(true);
  expect(result.retiredAddresses).toBe(0);
  expect(result.finalResetThread.thread_id).toBe("reset-18");
  expect(result.finalResetThread.items).toEqual([]);
  expect(result.finalNotificationTabs).toEqual(Array.from({ length: 8 }, (_, index) => [{ term_id: `tab-${16 + index}` }]));
}, 90_000);
