import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { runPushReplay } from "./pushReplayHarness.mjs";

const revision = process.argv[2] === "current" ? undefined : process.argv[2];
const recipe = { frames: 1200, intervalMs: 250, stallMs: 0, mixed: true, drainMs: 120000 };
const result = await runPushReplay({ revision, ...recipe });
const inputMs = result.elapsedMs - result.drainMs;
const samples = result.backlogSamples.filter((sample) => sample.at >= inputMs - 120000 && sample.at <= inputMs);
const first = samples[0], last = samples.at(-1);
const pendingSlope = (field) => first && last ? (last[field] - first[field]) / ((last.at - first.at) / 1000) : null;
const report = { revision: revision || "current", recipe, inputMs, framesPerSecond: recipe.frames / (inputMs / 1000),
  itemsPerSecond: (recipe.frames * 3 + 10) / (inputMs / 1000),
  finalTwoMinuteBodySlope: pendingSlope("pending"), finalTwoMinuteBackgroundSlope: pendingSlope("backgroundPending"), ...result };
if (process.argv[4]) {
  const baseline = JSON.parse(await readFile(process.argv[4], "utf8"));
  report.rowGetReduction = 1 - result.rowGets / baseline.rowGets;
}
const json = JSON.stringify(report, null, 2);
if (process.argv[3]) await writeFile(process.argv[3], `${json}\n`);
console.log(json);
if (process.argv[4]) {
  assert(report.framesPerSecond >= 3.5);
  assert(report.itemsPerSecond >= 8);
  assert(result.renderP95Ms <= 1000);
  assert(result.renderMaxMs <= 3000);
  assert(result.peakWork_pending <= 64);
  assert(report.finalTwoMinuteBodySlope <= 0);
  assert(report.finalTwoMinuteBackgroundSlope <= 0);
  assert(result.drainMs <= 5000);
  assert(report.rowGetReduction >= .8);
  assert(result.exactFinalMessages && result.exactFinalStates);
  assert.equal(result.retiredAddresses, 0);
  assert.equal(result.finalResetThread.thread_id, "reset-1140");
  assert.deepEqual(result.finalResetThread.items, []);
  assert.deepEqual(result.finalWork, { running: 0, pending: 0, backgroundRunning: 0, backgroundPending: 0 });
}
