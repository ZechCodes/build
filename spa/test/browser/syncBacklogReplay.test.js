import { expect, it } from "vitest";
import { runPushReplay } from "./pushReplayHarness.mjs";

it("coalesces board revision demand while preserving every mixed replay message", async () => {
  const result = await runPushReplay({ revision: process.env.SYNC_REPLAY_REVISION, frames: 240, intervalMs: 1, stallMs: 500, mixed: true, drainMs: 120_000 });
  // Fixed-fixture work budgets leave ample room for host scheduling delays.
  expect(result.rowGets).toBeLessThanOrEqual(2_048);
  expect(result.schedulerStarts).toBeGreaterThan(0);
  expect(result.renderSchedulerTurns).toHaveLength(40);
  expect(Math.max(...result.renderSchedulerTurns)).toBeLessThanOrEqual(128);
  expect(result.drainSchedulerTurns).toBeLessThanOrEqual(128);
  expect(result.deliveryDeadlineReached).not.toBe(true);
  expect(result.pendingTransactions).toBe(0);
  expect(result.pendingAppliers).toBe(0);
  expect(result.exactFinalMessages, JSON.stringify(result.messageMismatch)).toBe(true);
  expect(result.exactFinalStates).toBe(true);
  expect(result.peakWork_pending).toBeLessThanOrEqual(64);
  expect(result.finalNotificationTabs).toEqual(Array.from({ length: 8 }, (_, index) => [{ term_id: `tab-${232 + index}` }]));
  expect(result.retiredAddresses).toBe(0);
  expect(result.finalResetThread.thread_id).toBe("reset-180");
  expect(result.finalResetThread.items).toEqual([]);
  expect(result.peakWork_backgroundPending).toBeLessThanOrEqual(64);
  expect(result.finalWork).toEqual({ running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
}, 240_000);
