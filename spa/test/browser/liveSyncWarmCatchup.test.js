import { expect, it } from "vitest";
import { runLiveSyncReplay } from "./liveSyncReplayHarness.mjs";

it("keeps receiving tips while reopening retained threads and drains recovery", async () => {
  const result = await runLiveSyncReplay({ warmCatchup: true, holdThreadMs: 300, holdReleaseAfterRPC: true,
    frames: 36, intervalMs: 200, tasksPerProject: 2, retainedMessages: 10 });
  expect(result.warmBoundary.pendingTransactions).toBeGreaterThan(0);
  expect(result.ownRepairHolds.length).toBeGreaterThan(0);
  expect(result.ownRepairHolds.every((hold) => hold.end > hold.start)).toBe(true);
  expect(result.progressDuringRecovery).toBe(true);
  expect(result.progressAfterReleaseDuringProducer).toBe(true);
  expect(result.realPaintSchedulerTurns).toHaveLength(6);
  expect(Math.max(...result.realPaintSchedulerTurns)).toBeLessThanOrEqual(256);
  expect(result.peakCatchupPending).toBeLessThanOrEqual(16);
  expect(result.rowGets).toBeLessThan(4096);
  expect(result.finalPassRunning).toBe(false);
  expect(result.finalCatchups).toEqual({ running: 0, pending: 0 });
  expect(result.finalWork).toEqual({ running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
  expect(result.pendingAppliers).toBe(0);
  expect(result.pendingTransactions).toBe(0);
  expect(result.exactFinalMessages).toBe(true);
  expect(result.exactFinalStates).toBe(true);
  expect(result.readMarkerConstraints).toBe(true);
}, 180_000);

it("commits the mounted conversation ahead of a retained nonmounted body backlog", async () => {
  const result = await runLiveSyncReplay({ warmCatchup: true, warmTipMode: "mixed", holdThreadMs: 0,
    initialBodyBacklog: 64, boardMembership: 204, frames: 24, intervalMs: 200,
    tasksPerProject: 2, retainedMessages: 10 });
  expect(result.backlogCommitted).toBe(64);
  expect(result.exactBacklogStates).toBe(true);
  expect(result.firstOwnCommitBacklogCompleted).toBeLessThan(64);
  expect(result.progressAfterReleaseDuringProducer).toBe(true);
  expect(result.peakWork_pending).toBeLessThanOrEqual(128);
  expect(result.finalWork).toEqual({ running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
  expect(result.finalPassRunning).toBe(false);
  expect(result.finalCatchups).toEqual({ running: 0, pending: 0 });
  expect(result.exactFinalMessages).toBe(true);
  expect(result.exactFinalStates).toBe(true);
}, 180_000);
