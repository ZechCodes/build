import { writeFile } from "node:fs/promises";
import { runLiveSyncReplay } from "./liveSyncReplayHarness.mjs";

const output = process.argv[2] || "/tmp/858-smoke.json";
const profiled = process.argv.includes("--cpu");
const sensitivity = process.argv.includes("--sensitivity");
const smoke = process.argv.includes("--smoke");
const revision = process.argv.includes("--current") ? undefined : "83d5c9fd";
const result = await runLiveSyncReplay({ revision, frames: smoke ? 12 : sensitivity ? 36 : 120,
  intervalMs: 375, ablateCanvas: process.argv.includes("--canvas-off"), ablateInbox: process.argv.includes("--inbox-off"), cacheCanvasRect: process.argv.includes("--cache-canvas-rect"), boardMembership: process.argv.includes("--board204") ? 204 : 6, cpuProfilePath: profiled ? output.replace(/\.json$/, ".cpuprofile") : undefined });
await writeFile(output, JSON.stringify(result, null, 2));
console.log(JSON.stringify({ output, revision: revision || "current", profiled, seedRecords: result.seedRecords,
  inboxRows: result.seedInboxRows, visibleInboxRows: result.seedVisibleInboxRows,
  canvases: result.seedCanvasCount, animatingCanvases: result.animatingCanvasCount,
  exactFinalMessages: result.exactFinalMessages, exactFinalStates: result.exactFinalStates,
  manualRenderP95Ms: result.renderP95Ms, realPaintP95Ms: result.realPaintP95Ms,
  realPaintSamples: result.realPaintLatencies.length, uiCounters: result.ui.counters, uiTimings: result.ui.timings }));
