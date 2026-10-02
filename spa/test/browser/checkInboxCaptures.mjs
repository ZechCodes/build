// The inbox review captures are a separate, sequential gate: each script owns
// several Chromium/Vite lifetimes. Keep that work out of the default unit/layout
// suite, and keep generated images out of the tracked design directories.
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const output = process.argv[2] ? resolve(process.argv[2]) : await mkdtemp(join(tmpdir(), "build-inbox-captures-"));
await mkdir(output, { recursive: true });
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("BRIDGE_")));
console.log(`Inbox capture artifacts: ${output}`);
for (const name of ["captureInboxBadgeSum", "captureTaskUnread", "captureWatching"]) {
  const script = fileURLToPath(new URL(`./${name}.mjs`, import.meta.url));
  const result = spawnSync(process.execPath, [script, join(output, name)], { env, stdio: "inherit" });
  const code = result.status ?? 1;
  console.log(`${name}: exit ${code}`);
  if (result.error) console.error(result.error.message);
  if (code !== 0) {
    process.exitCode = code;
    break;
  }
}
