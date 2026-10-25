import { writeFile } from "node:fs/promises";
import { summarizeLiveSyncDelivery } from "./liveSyncDelivery.mjs";
import { runLiveSyncReplay } from "./liveSyncReplayHarness.mjs";

const output = process.argv[2] || "/tmp/858-smoke.json";
const profiled = process.argv.includes("--cpu");
const sensitivity = process.argv.includes("--sensitivity");
const smoke = process.argv.includes("--smoke");
const revision = process.argv.find((arg) => arg.startsWith("--revision="))?.slice("--revision=".length)
  || (process.argv.includes("--current") ? undefined : "83d5c9fd");
const result = await runLiveSyncReplay({ revision, frames: process.argv.includes("--sustained") ? 360 : smoke ? 12 : sensitivity ? 36 : 120,
  historyRepair: process.argv.includes("--history-repair"), representativeRate: process.argv.includes("--representative-rate"), resetFrame: process.argv.includes("--sustained") ? -1 : 180, removalFrame: process.argv.includes("--sustained") ? -1 : 120, warmCatchup: process.argv.includes("--warm-catchup"), explicitThreadPriority: process.argv.includes("--explicit-thread-priority"), holdThreadMs: process.argv.includes("--no-rpc-hold") ? 0 : 5000, initialBodyBacklog: process.argv.includes("--body-backlog64") ? 64 : 0, warmTipMode: process.argv.includes("--mixed-warm-tips") ? "mixed" : "empty", intervalMs: 375, ablateCanvas: process.argv.includes("--canvas-off"), ablateInbox: process.argv.includes("--inbox-off"), cacheCanvasRect: process.argv.includes("--cache-canvas-rect"), boardMembership: process.argv.includes("--board204") ? 204 : 6, cpuProfilePath: profiled ? output.replace(/\.json$/, ".cpuprofile") : undefined });
result.delivery = summarizeLiveSyncDelivery(result);
await writeFile(output, JSON.stringify(result, null, 2));
console.log(JSON.stringify({ output, revision: revision || "current", profiled, seedRecords: result.seedRecords,
  inboxRows: result.seedInboxRows, visibleInboxRows: result.seedVisibleInboxRows,
  canvases: result.seedCanvasCount, animatingCanvases: result.animatingCanvasCount,
  exactFinalMessages: result.exactFinalMessages, exactFinalStates: result.exactFinalStates,
  cacheP95Ms: result.cacheP95Ms, realPaintP95Ms: result.realPaintP95Ms,
  realPaintSamples: result.realPaintLatencies.length, uiCounters: result.ui.counters, uiTimings: result.ui.timings }));

if (!result.exactMonotonicPaint || !result.historicalRepairSettled || !result.delivery.complete || !result.exactFinalMessages || !result.exactFinalStates || (process.argv.includes("--warm-catchup") && result.finalPassRunning)) process.exitCode = 1;
