import { writeFile } from "node:fs/promises";
import { runPushReplay } from "./pushReplayHarness.mjs";

const revision = process.argv[2] === "current" ? undefined : process.argv[2];
const result = await runPushReplay({ revision, intervalMs: 17300 / 60, stallMs: 2000 });
const report = { revision: revision || "current", recipe: { frames: 60, stateItems: 60, inlineMessageTips: 60, coveredEmptyTips: 60, projects: 6, agents: 82,
  intervalMs: 17300 / 60, writeStallMs: 2000, rpcLatencyMs: 15 }, ...result };
const json = JSON.stringify(report, null, 2);
if (process.argv[3]) await writeFile(process.argv[3], `${json}\n`);
console.log(json);
