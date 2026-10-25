import { expect, it } from "vitest";
import { runPushReplay } from "./pushReplayHarness.mjs";

it("coalesces board revision demand while preserving every mixed replay message", async () => {
  const result = await runPushReplay({ revision: process.env.SYNC_REPLAY_REVISION, frames: 240, intervalMs: 1, stallMs: 500, mixed: true });
  expect(result.exactFinalMessages, JSON.stringify(result.messageMismatch)).toBe(true);
  expect(result.exactFinalStates).toBe(true);
  expect(result.peakWork_pending).toBeLessThanOrEqual(64);
  expect(result.finalNotificationTabs).toEqual(Array.from({ length: 8 }, (_, index) => [{ term_id: `tab-${232 + index}` }]));
  expect(result.retiredAddresses).toBe(0);
  expect(result.finalResetThread.thread_id).toBe("reset-180");
  expect(result.finalResetThread.items).toEqual([]);
  expect(result.peakWork_backgroundPending).toBeLessThanOrEqual(64);
  expect(result.finalWork).toEqual({ running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
}, 60_000);
