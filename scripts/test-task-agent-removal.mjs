#!/usr/bin/env node
// Generate a trace through real AppState agent.remove and subscriptions, then
// replay its wire responses/events through mounted production SPA components.
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const tracePath = join(mkdtempSync(join(tmpdir(), "build-task-agent-removal-")), "trace.json");
const env = { ...process.env, BUILD_TASK_AGENT_REMOVAL_TRACE: tracePath };
console.log(`Agent removal trace: ${tracePath}`);

function run(command, args, directory) {
  const result = spawnSync(command, args, { cwd: join(root, directory), env, stdio: "inherit" });
  const code = result.status ?? 1;
  if (result.error) console.error(result.error.message);
  console.log(`${command} ${args.join(" ")}: exit ${code}`);
  if (code !== 0) process.exit(code);
}

run("cargo", ["test", "remote_agent_removal_invalidates_task_identities", "--", "--nocapture"], "bridge");
run("npx", ["vitest", "run", "test/browser/taskIdentityRemovalMounted.test.js"], "spa");
